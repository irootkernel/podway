//! Daemon-owned composition of validated workspaces, SQLite state, metadata, and schedulers.
//!
//! The Store remains the durable authority. This module only admits a workspace after Git and the
//! existing Store binding agree, then keeps registry data as non-authoritative metadata.

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use podway_config::{
    ConfigError, DEFAULT_WORKSPACE_CONFIG_YAML_V1, WorkspaceConfigV1, parse_workspace_config_v1,
};
use podway_core::{DomainResult, SessionAggregateV1, UnixMillis, WorkspaceId};
use podway_git::{
    GitResolveErrorV1, GitResolverContractV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorkspaceLayoutErrorV1, WorkspaceLayoutInitializerV1, WorktreeSelectorV1,
};
use podway_protocol::Rfc3339MillisV1;
use podway_service::ServiceRuntimePathsV1;
use podway_store::{
    AdmitOutcomeV1, AdmitRequestV1, CancelOutcomeV1, CanonicalRequestDigestV1, ClaimTokenV1,
    ClaimedJobV1, IdempotencyKeyV1, IdempotentExecutionV1, JobIdV1, JobListQueryV1, JobViewV1,
    PersistedResponseContextV1, RecoveryReportV1, RevisionAttemptItemPreconditionsV1, RevisionV1,
    SqliteStoreOptionsV1, SqliteStoreV1, StateTransitionV1, StoreContractV1, StoreErrorV1,
    StoreIdempotencyReadContractV1, StoreInvariantV1, StoreReadContractV1,
    StoreUnavailableReasonV1, StoreValueErrorV1, TerminalReceiptV1, TerminalResultV1,
    ValidatedWorkspaceRootV1, WorkerIdV1, WorkspaceBindingV1, WorkspaceViewV1,
};

use crate::{
    DaemonCompositionErrorV1,
    execution::{PreparedWorkspaceResetAllV1, ResetStoreInspectionV1, ValidatedUnavailableStoreV1},
    observability::ObservabilityEmitterV1,
    registry::{RegistryErrorV1, RegistryStoreV1, WorkspaceRegistryEntryV1, WorkspaceRegistryV1},
    scheduler::{WorkspaceSchedulerKeyV1, WorkspaceSchedulerRegistryV1, WorkspaceSchedulerV1},
    workspace::{
        ResetMaintenanceFilesystemTokenV1, ResetMarkerV1, ResetWorkspaceResolutionV1,
        ResolvedWorkspaceV1, SqliteWorkspaceBindingInspectorV1, ValidatedRuntimeDirectoryErrorV1,
        ValidatedRuntimeDirectoryV1, WorkspaceBindingInspectionErrorV1, WorkspaceGitObservationV1,
        WorkspaceResolutionErrorV1, WorkspaceResolverV1,
    },
};

/// Reset-all crash boundaries available only through an explicitly injected test seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetAllCrashBoundaryV1 {
    MarkerCreated,
    OldDatabaseDeleted,
    NewTargetDatabaseCreated,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ResetAllCrashInjectionV1(Option<ResetAllCrashBoundaryV1>);

impl ResetAllCrashInjectionV1 {
    fn abort_at(self, boundary: ResetAllCrashBoundaryV1) {
        if self.0 == Some(boundary) {
            std::process::abort();
        }
    }
}

/// The caller-supplied clock values used by the Store and metadata-only registry update.
///
/// Keeping the values explicit makes the daemon's durable writes deterministic and avoids making a
/// filesystem-resolution operation own a second time source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceRuntimeObservationV1 {
    store_now: UnixMillis,
    registry_seen_at: Rfc3339MillisV1,
}

impl WorkspaceRuntimeObservationV1 {
    pub fn new(store_now: UnixMillis, registry_seen_at: Rfc3339MillisV1) -> Self {
        Self {
            store_now,
            registry_seen_at,
        }
    }

    pub const fn store_now(&self) -> UnixMillis {
        self.store_now
    }

    pub fn registry_seen_at(&self) -> &Rfc3339MillisV1 {
        &self.registry_seen_at
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DatabaseFileIdentityV1 {
    device: u64,
    inode: u64,
}

#[cfg(not(unix))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct DatabaseFileIdentityV1 {
    length: u64,
    modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ValidatedDatabaseFileV1 {
    path: PathBuf,
    identity: DatabaseFileIdentityV1,
}
/// The daemon-owned Store handle shared by all clones of one scheduler context.
///
/// The underlying Store is consumed during maintenance and never recreated in this slot. Once
/// closed, every Store operation fails closed and repeated close attempts return the cached result.
pub(crate) struct WorkspaceStoreSlotV1 {
    state: Mutex<WorkspaceStoreStateV1>,
    startup_recovery_report: RecoveryReportV1,
    token: WorkspaceStoreSlotTokenV1,
    #[cfg(test)]
    close_failpoint: Option<WorkspaceStoreCloseFailpointV1>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct WorkspaceStoreSlotTokenV1(u64);

impl WorkspaceStoreSlotTokenV1 {
    fn issue() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let token = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .expect("workspace Store-slot token space must not be exhausted");
        Self(token)
    }
}

enum WorkspaceStoreStateV1 {
    Open(Box<SqliteStoreV1>),
    Closed(Result<(), StoreErrorV1>),
}

#[cfg(test)]
struct WorkspaceStoreCloseFailpointV1 {
    error: StoreErrorV1,
    close_count: Arc<AtomicU64>,
}
/// Public read-only observation facade for one scheduler-owned Store generation.
///
/// The upstream read trait inherits mutation methods, so this facade implements those methods as
/// fail-closed errors and never carries the manager's mutation-capable Store Arc.
#[derive(Clone)]
pub struct WorkspaceStoreReadFacadeV1 {
    slot: Arc<WorkspaceStoreSlotV1>,
}

impl WorkspaceStoreReadFacadeV1 {
    fn new(slot: Arc<WorkspaceStoreSlotV1>) -> Self {
        Self { slot }
    }

    pub fn startup_recovery_report(&self) -> &RecoveryReportV1 {
        self.slot.startup_recovery_report()
    }

    pub fn read_workspace_view(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
    ) -> Result<WorkspaceViewV1, StoreErrorV1> {
        self.slot.read_workspace_view(identity)
    }
}

impl StoreReadContractV1 for WorkspaceStoreReadFacadeV1 {
    fn read_session_aggregate(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
    ) -> Result<Option<SessionAggregateV1>, StoreErrorV1> {
        self.slot.read_session_aggregate(identity)
    }

    fn read_job(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        job: &JobIdV1,
    ) -> Result<Option<JobViewV1>, StoreErrorV1> {
        self.slot.read_job(identity, job)
    }

    fn list_jobs(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        query: JobListQueryV1,
    ) -> Result<Vec<JobViewV1>, StoreErrorV1> {
        self.slot.list_jobs(identity, query)
    }
}

impl StoreIdempotencyReadContractV1 for WorkspaceStoreReadFacadeV1 {
    fn read_idempotent_outcome(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
        request_digest: &CanonicalRequestDigestV1,
    ) -> Result<Option<AdmitOutcomeV1>, StoreErrorV1> {
        self.slot
            .read_idempotent_outcome(identity, idempotency_key, request_digest)
    }

    fn read_idempotent_execution(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<Option<IdempotentExecutionV1>, StoreErrorV1> {
        self.slot
            .read_idempotent_execution(identity, idempotency_key)
    }
}
impl StoreContractV1 for WorkspaceStoreReadFacadeV1 {
    fn admit(
        &self,
        _identity: &podway_store::DurableWorktreeIdentityV1,
        _request: AdmitRequestV1,
    ) -> Result<AdmitOutcomeV1, StoreErrorV1> {
        Err(read_only_store_mutation_error())
    }

    fn claim_next(
        &self,
        _identity: &podway_store::DurableWorktreeIdentityV1,
        _worker: WorkerIdV1,
        _now: podway_store::EpochMillisV1,
    ) -> Result<Option<ClaimedJobV1>, StoreErrorV1> {
        Err(read_only_store_mutation_error())
    }

    fn cancel_before_claim(
        &self,
        _identity: &podway_store::DurableWorktreeIdentityV1,
        _job: JobIdV1,
        _expected_job_revision: RevisionV1,
        _now: podway_store::EpochMillisV1,
    ) -> Result<CancelOutcomeV1, StoreErrorV1> {
        Err(read_only_store_mutation_error())
    }

    fn commit_terminal(
        &self,
        _claim: ClaimTokenV1,
        _expected_workspace_revision: RevisionV1,
        _transition: Option<StateTransitionV1>,
        _result: TerminalResultV1,
        _now: podway_store::EpochMillisV1,
    ) -> Result<TerminalReceiptV1, StoreErrorV1> {
        Err(read_only_store_mutation_error())
    }

    fn read_workspace_view(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
    ) -> Result<WorkspaceViewV1, StoreErrorV1> {
        self.slot.read_workspace_view(identity)
    }
}

fn read_only_store_mutation_error() -> StoreErrorV1 {
    StoreErrorV1::StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1::Recovery,
    }
}

impl WorkspaceStoreSlotV1 {
    fn new(store: SqliteStoreV1) -> Self {
        Self {
            startup_recovery_report: store.startup_recovery_report().clone(),
            state: Mutex::new(WorkspaceStoreStateV1::Open(Box::new(store))),
            token: WorkspaceStoreSlotTokenV1::issue(),
            #[cfg(test)]
            close_failpoint: None,
        }
    }
    /// Returns an inert Store slot for reset preparation when no prior generation can be read
    /// safely. The reset preparation path passes this slot as no idempotency authority, so it
    /// cannot observe or manufacture a durable outcome.
    pub(crate) fn unavailable_for_reset_preparation() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(WorkspaceStoreStateV1::Closed(Err(
                StoreErrorV1::StorageUnavailableV1 {
                    reason: StoreUnavailableReasonV1::Recovery,
                },
            ))),
            startup_recovery_report: RecoveryReportV1::new(0, UnixMillis::new(0)),
            token: WorkspaceStoreSlotTokenV1::issue(),
            #[cfg(test)]
            close_failpoint: None,
        })
    }

    /// Returns recovery metadata captured while this slot was opened.
    pub(crate) fn startup_recovery_report(&self) -> &RecoveryReportV1 {
        &self.startup_recovery_report
    }

    /// Consumes this slot's sole Store connection for daemon-owned maintenance.
    ///
    /// The mutex remains held until the consuming close completes, so concurrent callers observe
    /// either the open Store or the same cached terminal close outcome.
    pub(crate) fn close_for_maintenance(&self) -> Result<(), StoreErrorV1> {
        let mut state = mutex_lock(&self.state);
        let current = std::mem::replace(&mut *state, WorkspaceStoreStateV1::Closed(Ok(())));
        let result = match current {
            WorkspaceStoreStateV1::Closed(result) => {
                *state = WorkspaceStoreStateV1::Closed(result.clone());
                return result;
            }
            WorkspaceStoreStateV1::Open(store) => {
                let result = (*store).close_for_maintenance();
                #[cfg(test)]
                {
                    if let Some(failpoint) = &self.close_failpoint {
                        failpoint.close_count.fetch_add(1, Ordering::SeqCst);
                        result.and(Err(failpoint.error.clone()))
                    } else {
                        result
                    }
                }
                #[cfg(not(test))]
                {
                    result
                }
            }
        };
        *state = WorkspaceStoreStateV1::Closed(result.clone());
        result
    }

    fn with_open_store<ResultValue>(
        &self,
        operation: impl FnOnce(&SqliteStoreV1) -> Result<ResultValue, StoreErrorV1>,
    ) -> Result<ResultValue, StoreErrorV1> {
        match &*mutex_lock(&self.state) {
            WorkspaceStoreStateV1::Open(store) => operation(store),
            WorkspaceStoreStateV1::Closed(result) => match result {
                Ok(()) => Err(StoreErrorV1::StorageUnavailableV1 {
                    reason: StoreUnavailableReasonV1::Recovery,
                }),
                Err(error) => Err(error.clone()),
            },
        }
    }
}

impl StoreContractV1 for WorkspaceStoreSlotV1 {
    fn admit(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        request: AdmitRequestV1,
    ) -> Result<AdmitOutcomeV1, StoreErrorV1> {
        self.with_open_store(|store| store.admit(identity, request))
    }

    fn claim_next(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        worker: WorkerIdV1,
        now: podway_store::EpochMillisV1,
    ) -> Result<Option<ClaimedJobV1>, StoreErrorV1> {
        self.with_open_store(|store| store.claim_next(identity, worker, now))
    }

    fn cancel_before_claim(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        job: JobIdV1,
        expected_job_revision: RevisionV1,
        now: podway_store::EpochMillisV1,
    ) -> Result<CancelOutcomeV1, StoreErrorV1> {
        self.with_open_store(|store| {
            store.cancel_before_claim(identity, job, expected_job_revision, now)
        })
    }

    fn commit_terminal(
        &self,
        claim: ClaimTokenV1,
        expected_workspace_revision: RevisionV1,
        transition: Option<StateTransitionV1>,
        result: TerminalResultV1,
        now: podway_store::EpochMillisV1,
    ) -> Result<TerminalReceiptV1, StoreErrorV1> {
        self.with_open_store(|store| {
            store.commit_terminal(claim, expected_workspace_revision, transition, result, now)
        })
    }

    fn read_workspace_view(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
    ) -> Result<WorkspaceViewV1, StoreErrorV1> {
        self.with_open_store(|store| store.read_workspace_view(identity))
    }
}

impl StoreReadContractV1 for WorkspaceStoreSlotV1 {
    fn read_session_aggregate(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
    ) -> Result<Option<SessionAggregateV1>, StoreErrorV1> {
        self.with_open_store(|store| store.read_session_aggregate(identity))
    }

    fn read_job(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        job: &JobIdV1,
    ) -> Result<Option<JobViewV1>, StoreErrorV1> {
        self.with_open_store(|store| store.read_job(identity, job))
    }

    fn list_jobs(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        query: JobListQueryV1,
    ) -> Result<Vec<JobViewV1>, StoreErrorV1> {
        self.with_open_store(|store| store.list_jobs(identity, query))
    }
}

impl StoreIdempotencyReadContractV1 for WorkspaceStoreSlotV1 {
    fn read_idempotent_outcome(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
        request_digest: &CanonicalRequestDigestV1,
    ) -> Result<Option<AdmitOutcomeV1>, StoreErrorV1> {
        self.with_open_store(|store| {
            store.read_idempotent_outcome(identity, idempotency_key, request_digest)
        })
    }

    fn read_idempotent_execution(
        &self,
        identity: &podway_store::DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<Option<IdempotentExecutionV1>, StoreErrorV1> {
        self.with_open_store(|store| store.read_idempotent_execution(identity, idempotency_key))
    }
}

/// Immutable context captured for one identity-keyed scheduler generation.
///
/// `git_evidence` is retained so execution can revalidate the exact worktree evidence rather than
/// treating a root path or registry entry as authority.
#[derive(Clone)]
pub struct WorkspaceSchedulerContextV1 {
    binding: WorkspaceBindingV1,
    workspace_root: ValidatedWorkspaceRootV1,
    database: ValidatedDatabaseFileV1,
    runtime_directory_path: PathBuf,
    store: Arc<WorkspaceStoreSlotV1>,
    store_options: SqliteStoreOptionsV1,
    config: WorkspaceConfigV1,
    queue_limit: u16,
    git_evidence: podway_git::ValidatedWorktreeV1,
    coordination: Arc<WorkspaceSchedulerCoordinationV1>,
    retirement: Arc<Mutex<Option<WorkspaceSchedulerRetirementGuardV1>>>,
}

#[derive(Debug)]
struct WorkspaceSchedulerCoordinationV1 {
    claim_gate: Mutex<()>,
    state: Mutex<WorkspaceSchedulerCoordinationStateV1>,
    changed: Condvar,
    maintenance: Arc<WorkspaceMaintenanceCoordinatorV1>,
}
#[derive(Clone, Debug)]
struct WorkspaceSchedulerRetirementBindingV1 {
    key: WorkspaceSchedulerKeyV1,
    generation: crate::SchedulerGenerationV1,
    source: podway_store::DurableWorktreeIdentityV1,
    store_slot: WorkspaceStoreSlotTokenV1,
}
struct WorkspaceSchedulerRetirementGuardV1 {
    coordinator: Arc<WorkspaceMaintenanceCoordinatorV1>,
    binding: WorkspaceSchedulerRetirementBindingV1,
}
impl Drop for WorkspaceSchedulerRetirementGuardV1 {
    fn drop(&mut self) {
        self.coordinator
            .unregister_scheduler_generation(&self.binding);
    }
}

#[derive(Debug)]
struct WorkspaceSchedulerCoordinationStateV1 {
    accepting_claims: bool,
    recovery_required: bool,
    notification_version: u64,
}
struct WorkspaceSchedulerContextInputV1 {
    binding: WorkspaceBindingV1,
    database: ValidatedDatabaseFileV1,
    runtime_directory_path: PathBuf,
    store: Arc<WorkspaceStoreSlotV1>,
    store_options: SqliteStoreOptionsV1,
    config: WorkspaceConfigV1,
    git_evidence: podway_git::ValidatedWorktreeV1,
    maintenance: Arc<WorkspaceMaintenanceCoordinatorV1>,
}

impl WorkspaceSchedulerContextV1 {
    fn new(input: WorkspaceSchedulerContextInputV1) -> Self {
        let WorkspaceSchedulerContextInputV1 {
            binding,
            database,
            runtime_directory_path,
            store,
            store_options,
            config,
            git_evidence,
            maintenance,
        } = input;
        let queue_limit = config.job_queue.max_pending;
        let workspace_root = binding.last_validated_root().clone();
        Self {
            binding,
            workspace_root,
            database,
            runtime_directory_path,
            store,
            store_options,
            config,
            queue_limit,
            git_evidence,
            coordination: Arc::new(WorkspaceSchedulerCoordinationV1 {
                claim_gate: Mutex::new(()),
                state: Mutex::new(WorkspaceSchedulerCoordinationStateV1 {
                    accepting_claims: true,
                    recovery_required: false,
                    notification_version: 0,
                }),
                changed: Condvar::new(),
                maintenance,
            }),
            retirement: Arc::new(Mutex::new(None)),
        }
    }

    pub fn binding(&self) -> &WorkspaceBindingV1 {
        &self.binding
    }

    pub fn workspace_root(&self) -> &ValidatedWorkspaceRootV1 {
        &self.workspace_root
    }

    pub fn database_path(&self) -> &Path {
        &self.database.path
    }

    fn database_file_identity(&self) -> &DatabaseFileIdentityV1 {
        &self.database.identity
    }

    pub fn runtime_directory_path(&self) -> &Path {
        &self.runtime_directory_path
    }

    pub fn store(&self) -> WorkspaceStoreReadFacadeV1 {
        WorkspaceStoreReadFacadeV1::new(Arc::clone(&self.store))
    }

    pub(crate) fn store_for_mutation(&self) -> Arc<WorkspaceStoreSlotV1> {
        Arc::clone(&self.store)
    }

    /// Consumes the sole Store handle shared by this context's clones for maintenance.
    pub(crate) fn close_store_for_maintenance(&self) -> Result<(), StoreErrorV1> {
        let result = self.store.close_for_maintenance();
        if let Some(retirement) = self.retirement_binding() {
            self.coordination
                .maintenance
                .record_store_close_result(&retirement, &result);
        }
        result
    }
    pub fn store_options(&self) -> &SqliteStoreOptionsV1 {
        &self.store_options
    }
    pub fn config(&self) -> &WorkspaceConfigV1 {
        &self.config
    }
    pub const fn queue_limit(&self) -> u16 {
        self.queue_limit
    }
    pub fn git_evidence(&self) -> &podway_git::ValidatedWorktreeV1 {
        &self.git_evidence
    }
    fn bind_maintenance_generation(
        &self,
        key: WorkspaceSchedulerKeyV1,
        generation: crate::SchedulerGenerationV1,
    ) {
        let mut current = mutex_lock(&self.retirement);
        if current.is_some() {
            return;
        }
        let retirement = WorkspaceSchedulerRetirementBindingV1 {
            key,
            generation,
            source: self.binding.identity().clone(),
            store_slot: self.store.token,
        };
        *current = Some(WorkspaceSchedulerRetirementGuardV1 {
            coordinator: Arc::clone(&self.coordination.maintenance),
            binding: retirement.clone(),
        });
        drop(current);
        self.coordination
            .maintenance
            .register_scheduler_generation(retirement);
    }
    fn retirement_binding(&self) -> Option<WorkspaceSchedulerRetirementBindingV1> {
        mutex_lock(&self.retirement)
            .as_ref()
            .map(|retirement| retirement.binding.clone())
    }

    pub(crate) fn worker_read_job(
        &self,
        job: &podway_store::JobIdV1,
    ) -> Result<Option<podway_store::JobViewV1>, StoreErrorV1> {
        use podway_store::StoreReadContractV1 as _;

        self.store.read_job(self.binding.identity(), job)
    }

    pub(crate) fn with_claim_permission<R>(
        &self,
        operation: impl FnOnce(&WorkspaceBindingV1) -> R,
    ) -> Option<R> {
        let _claim_gate = mutex_lock(&self.coordination.claim_gate);
        if !mutex_lock(&self.coordination.state).accepting_claims {
            return None;
        }
        let _claim = self.coordination.maintenance.acquire_claim(
            WorkspaceMaintenanceKeyV1::from_durable_identity(self.binding.identity()),
        )?;
        Some(operation(&self.binding))
    }

    pub(crate) fn stop_claims(&self) {
        let _claim_gate = mutex_lock(&self.coordination.claim_gate);
        mutex_lock(&self.coordination.state).accepting_claims = false;
        if let Some(retirement) = self.retirement_binding() {
            self.coordination
                .maintenance
                .record_claims_stopped(&retirement);
        }
    }

    pub(crate) fn record_work_drained(&self) {
        if let Some(retirement) = self.retirement_binding() {
            self.coordination
                .maintenance
                .record_work_drained(&retirement);
        }
    }

    pub(crate) fn mark_recovery_required(&self) {
        mutex_lock(&self.coordination.state).recovery_required = true;
    }

    pub(crate) fn recovery_required(&self) -> bool {
        mutex_lock(&self.coordination.state).recovery_required
    }

    pub(crate) fn notification_version(&self) -> u64 {
        mutex_lock(&self.coordination.state).notification_version
    }

    pub(crate) fn notify_after_authoritative_change(&self) {
        let mut state = mutex_lock(&self.coordination.state);
        state.notification_version = state.notification_version.wrapping_add(1);
        drop(state);
        self.coordination.changed.notify_all();
    }

    pub(crate) fn wait_for_notification_after(&self, observed: u64, timeout: std::time::Duration) {
        let state = mutex_lock(&self.coordination.state);
        if state.notification_version == observed {
            let _ = self
                .coordination
                .changed
                .wait_timeout(state, timeout)
                .expect("workspace scheduler notification lock must not be poisoned");
        }
    }
}

/// A scheduler context was either still validated or must be retired by its lifecycle owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceSchedulerRevalidationV1 {
    Current,
    RetireRequired {
        key: WorkspaceSchedulerKeyV1,
        generation: crate::SchedulerGenerationV1,
        source: WorkspaceResolutionErrorV1,
    },
}

/// Failures while composing a scheduler context from one selected workspace.
#[derive(Debug)]
pub enum WorkspaceRuntimeErrorV1 {
    Resolution(WorkspaceResolutionErrorV1),
    Layout(WorkspaceLayoutErrorV1),
    Store(StoreErrorV1),
    Registry(RegistryErrorV1),
    ConfigRead {
        path: PathBuf,
        source: io::Error,
    },
    ConfigAdmission(ConfigError),
    StoreOptions(StoreValueErrorV1),
    Scheduler(DaemonCompositionErrorV1),
    BindingDisappeared {
        database_path: PathBuf,
    },
    BindingIdentityMismatch {
        expected: Box<podway_store::DurableWorktreeIdentityV1>,
        actual: Box<podway_store::DurableWorktreeIdentityV1>,
    },
    BindingMismatch {
        expected: Box<WorkspaceBindingV1>,
        actual: Box<WorkspaceBindingV1>,
    },
    RebindEvidenceMismatch {
        binding_root: Box<ValidatedWorkspaceRootV1>,
        git_previous_root: Option<Box<ValidatedWorkspaceRootV1>>,
        git_current_root: Box<ValidatedWorkspaceRootV1>,
    },
    RuntimeDirectoryMissing {
        database_path: PathBuf,
    },
    RuntimePathMismatch {
        database_path: PathBuf,
        runtime_directory_path: PathBuf,
    },
    RuntimeDirectory(ValidatedRuntimeDirectoryErrorV1),
    MaintenanceInProgress,
    ResetMarkerConflict,
    ResetIdempotencyConflict {
        existing: CanonicalRequestDigestV1,
        requested: CanonicalRequestDigestV1,
    },
    ResetSchedulerRetirement,
    RuntimePathsUnsupportedPlatform,
    ResetSourceNotRegistered,
    ResetSourceAmbiguous,
    ResetRegistryPredecessorStale,
    RevalidationKeyMismatch {
        expected: Box<WorkspaceSchedulerKeyV1>,
        actual: Box<WorkspaceSchedulerKeyV1>,
    },
}

impl fmt::Display for WorkspaceRuntimeErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolution(_) => formatter.write_str("workspace resolution failed"),
            Self::Layout(_) => formatter.write_str("workspace layout initialization failed"),
            Self::Store(_) => formatter.write_str("workspace Store operation failed"),
            Self::Registry(_) => formatter.write_str("workspace metadata registry update failed"),
            Self::ConfigRead { path, .. } => {
                write!(
                    formatter,
                    "cannot read workspace configuration {}",
                    path.display()
                )
            }
            Self::ConfigAdmission(_) => formatter.write_str("workspace configuration is invalid"),
            Self::StoreOptions(_) => formatter.write_str("workspace Store options are invalid"),
            Self::Scheduler(_) => formatter.write_str("workspace scheduler creation failed"),
            Self::BindingDisappeared { database_path } => write!(
                formatter,
                "workspace binding disappeared from {} before Store open",
                database_path.display()
            ),
            Self::BindingIdentityMismatch { .. } => {
                formatter.write_str("workspace binding identity changed during activation")
            }
            Self::BindingMismatch { .. } => {
                formatter.write_str("workspace binding does not match the validated workspace")
            }
            Self::RebindEvidenceMismatch { .. } => formatter
                .write_str("workspace root rebinding lacks matching Git and Store evidence"),
            Self::RuntimeDirectoryMissing { database_path } => write!(
                formatter,
                "workspace database {} has no runtime-directory parent",
                database_path.display()
            ),
            Self::RuntimePathMismatch { .. } => formatter.write_str(
                "workspace runtime directory does not match the validated database location",
            ),
            Self::RuntimeDirectory(_) => {
                formatter.write_str("validated reset runtime-directory operation failed")
            }
            Self::MaintenanceInProgress => {
                formatter.write_str("workspace maintenance is already in progress")
            }
            Self::ResetMarkerConflict => {
                formatter.write_str("reset marker conflicts with the requested operation")
            }
            Self::ResetIdempotencyConflict { .. } => {
                formatter.write_str("reset marker conflicts with the requested idempotency digest")
            }
            Self::ResetSchedulerRetirement => {
                formatter.write_str("old scheduler retirement did not complete")
            }
            Self::RuntimePathsUnsupportedPlatform => {
                formatter.write_str("workspace runtime paths require Unix native path support")
            }
            Self::ResetSourceNotRegistered => {
                formatter.write_str("no registered reset source matches the validated worktree")
            }
            Self::ResetSourceAmbiguous => formatter
                .write_str("multiple registered reset sources match the validated worktree"),
            Self::ResetRegistryPredecessorStale => formatter
                .write_str("registered reset predecessor changed before destructive maintenance"),
            Self::RevalidationKeyMismatch { .. } => {
                formatter.write_str("revalidated workspace has a different scheduler identity")
            }
        }
    }
}

impl Error for WorkspaceRuntimeErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Resolution(source) => Some(source),
            Self::Store(source) => Some(source),
            Self::Registry(source) => Some(source),
            Self::RuntimeDirectory(source) => Some(source),
            Self::ConfigRead { source, .. } => Some(source),
            Self::ConfigAdmission(source) => Some(source),
            Self::StoreOptions(source) => Some(source),
            Self::Scheduler(source) => Some(source),
            Self::Layout(_)
            | Self::BindingDisappeared { .. }
            | Self::BindingIdentityMismatch { .. }
            | Self::BindingMismatch { .. }
            | Self::RebindEvidenceMismatch { .. }
            | Self::RuntimeDirectoryMissing { .. }
            | Self::RuntimePathMismatch { .. }
            | Self::RuntimePathsUnsupportedPlatform
            | Self::MaintenanceInProgress
            | Self::ResetMarkerConflict
            | Self::ResetIdempotencyConflict { .. }
            | Self::ResetSchedulerRetirement
            | Self::ResetSourceNotRegistered
            | Self::ResetSourceAmbiguous
            | Self::ResetRegistryPredecessorStale
            | Self::RevalidationKeyMismatch { .. } => None,
        }
    }
}

/// A fully validated workspace resolution that carries no mutation authority.
///
/// The resolver observes Git and the Store binding twice before constructing this value. It never
/// creates a scheduler or refreshes daemon registry metadata. An already-active scheduler is
/// included only when its immutable context exactly matches the new resolution; callers must use
/// the returned binding rather than a display path for every Store operation.
#[derive(Clone)]
pub struct ReadonlyWorkspaceResolutionV1 {
    binding: WorkspaceBindingV1,
    workspace_root: ValidatedWorkspaceRootV1,
    database_path: PathBuf,
    store_options: SqliteStoreOptionsV1,
    worktree: podway_git::ValidatedWorktreeV1,
    active_scheduler: Option<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>>,
}

impl ReadonlyWorkspaceResolutionV1 {
    pub fn binding(&self) -> &WorkspaceBindingV1 {
        &self.binding
    }

    pub fn workspace_root(&self) -> &ValidatedWorkspaceRootV1 {
        &self.workspace_root
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn store_options(&self) -> &SqliteStoreOptionsV1 {
        &self.store_options
    }

    pub fn worktree(&self) -> &podway_git::ValidatedWorktreeV1 {
        &self.worktree
    }

    pub fn active_scheduler(
        &self,
    ) -> Option<&Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>> {
        self.active_scheduler.as_ref()
    }
}
/// The exclusive reset-maintenance key for one Git worktree. It intentionally excludes the
/// workspace UUID and every filesystem path, so an old identity and its reset target share a
/// single lease.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct WorkspaceMaintenanceKeyV1 {
    common_directory_fingerprint: CanonicalRequestDigestV1,
    worktree_administration_fingerprint: CanonicalRequestDigestV1,
}

impl WorkspaceMaintenanceKeyV1 {
    fn from_worktree(worktree: &podway_git::ValidatedWorktreeV1) -> Self {
        Self {
            common_directory_fingerprint: worktree
                .identity()
                .common_directory_fingerprint()
                .clone(),
            worktree_administration_fingerprint: worktree
                .identity()
                .worktree_administration_fingerprint()
                .clone(),
        }
    }
    fn from_durable_identity(identity: &podway_store::DurableWorktreeIdentityV1) -> Self {
        Self {
            common_directory_fingerprint: identity.common_dir_identity().clone(),
            worktree_administration_fingerprint: identity.worktree_admin_identity().clone(),
        }
    }
}

#[derive(Debug, Default)]
struct WorkspaceMaintenanceCoordinatorV1 {
    state: Mutex<WorkspaceMaintenanceCoordinatorStateV1>,
}
#[derive(Debug, Default)]
struct WorkspaceMaintenanceCoordinatorStateV1 {
    maintenance: HashSet<WorkspaceMaintenanceKeyV1>,
    activations: HashMap<WorkspaceMaintenanceKeyV1, usize>,
    claims: HashMap<WorkspaceMaintenanceKeyV1, usize>,
    rebinds: HashSet<WorkspaceMaintenanceKeyV1>,
    generations: HashMap<WorkspaceStoreSlotTokenV1, WorkspaceSchedulerRetirementStateV1>,
}
#[derive(Debug)]
struct WorkspaceSchedulerRetirementStateV1 {
    binding: WorkspaceSchedulerRetirementBindingV1,
    claims_stopped: bool,
    work_drained: bool,
    store_closed: bool,
}
impl WorkspaceMaintenanceCoordinatorV1 {
    fn acquire(
        self: &Arc<Self>,
        key: WorkspaceMaintenanceKeyV1,
    ) -> Option<WorkspaceMaintenanceLeaseV1> {
        let mut state = mutex_lock(&self.state);
        if state.maintenance.contains(&key)
            || state.activations.contains_key(&key)
            || state.claims.contains_key(&key)
        {
            return None;
        }
        state.maintenance.insert(key.clone());
        Some(WorkspaceMaintenanceLeaseV1 {
            coordinator: Arc::clone(self),
            key: Some(key),
        })
    }
    fn acquire_activation(
        self: &Arc<Self>,
        key: WorkspaceMaintenanceKeyV1,
    ) -> Option<WorkspaceActivationLeaseV1> {
        let mut state = mutex_lock(&self.state);
        if state.maintenance.contains(&key) || state.rebinds.contains(&key) {
            return None;
        }
        *state.activations.entry(key.clone()).or_insert(0) += 1;
        Some(WorkspaceActivationLeaseV1 {
            coordinator: Arc::clone(self),
            key: Some(key),
        })
    }
    fn acquire_claim(
        self: &Arc<Self>,
        key: WorkspaceMaintenanceKeyV1,
    ) -> Option<WorkspaceClaimLeaseV1> {
        let mut state = mutex_lock(&self.state);
        if state.maintenance.contains(&key) || state.rebinds.contains(&key) {
            return None;
        }
        *state.claims.entry(key.clone()).or_insert(0) += 1;
        Some(WorkspaceClaimLeaseV1 {
            coordinator: Arc::clone(self),
            key: Some(key),
        })
    }

    fn acquire_rebind(
        self: &Arc<Self>,
        key: WorkspaceMaintenanceKeyV1,
    ) -> Option<WorkspaceRebindLeaseV1> {
        let mut state = mutex_lock(&self.state);
        if state.maintenance.contains(&key)
            || state.rebinds.contains(&key)
            || state.claims.contains_key(&key)
            || state.activations.get(&key).copied() != Some(1)
        {
            return None;
        }
        state.rebinds.insert(key.clone());
        Some(WorkspaceRebindLeaseV1 {
            coordinator: Arc::clone(self),
            key: Some(key),
        })
    }
    fn register_scheduler_generation(&self, binding: WorkspaceSchedulerRetirementBindingV1) {
        let mut state = mutex_lock(&self.state);
        state.generations.entry(binding.store_slot).or_insert(
            WorkspaceSchedulerRetirementStateV1 {
                binding,
                claims_stopped: false,
                work_drained: false,
                store_closed: false,
            },
        );
    }
    fn unregister_scheduler_generation(&self, binding: &WorkspaceSchedulerRetirementBindingV1) {
        let mut state = mutex_lock(&self.state);
        if state
            .generations
            .get(&binding.store_slot)
            .is_some_and(|record| {
                record.binding.key == binding.key
                    && record.binding.generation == binding.generation
                    && record.binding.source == binding.source
            })
        {
            state.generations.remove(&binding.store_slot);
        }
    }
    fn record_claims_stopped(&self, binding: &WorkspaceSchedulerRetirementBindingV1) {
        let mut state = mutex_lock(&self.state);
        match state.generations.get_mut(&binding.store_slot) {
            Some(record)
                if record.binding.key == binding.key
                    && record.binding.generation == binding.generation
                    && record.binding.source == binding.source =>
            {
                record.claims_stopped = true;
            }
            _ => {}
        }
    }
    fn record_work_drained(&self, binding: &WorkspaceSchedulerRetirementBindingV1) {
        let mut state = mutex_lock(&self.state);
        match state.generations.get_mut(&binding.store_slot) {
            Some(record)
                if record.binding.key == binding.key
                    && record.binding.generation == binding.generation
                    && record.binding.source == binding.source
                    && record.claims_stopped =>
            {
                record.work_drained = true;
            }
            _ => {}
        }
    }
    fn record_store_closed(&self, binding: &WorkspaceSchedulerRetirementBindingV1) {
        let mut state = mutex_lock(&self.state);
        match state.generations.get_mut(&binding.store_slot) {
            Some(record)
                if record.binding.key == binding.key
                    && record.binding.generation == binding.generation
                    && record.binding.source == binding.source
                    && record.claims_stopped
                    && record.work_drained =>
            {
                record.store_closed = true;
            }
            _ => {}
        }
    }
    fn record_store_close_result(
        &self,
        binding: &WorkspaceSchedulerRetirementBindingV1,
        result: &Result<(), StoreErrorV1>,
    ) {
        if result.is_ok() {
            self.record_store_closed(binding);
        }
    }
    fn has_retirement_receipt(
        &self,
        key: &WorkspaceMaintenanceKeyV1,
        previous_workspace_uuid: &WorkspaceId,
    ) -> bool {
        mutex_lock(&self.state).generations.values().any(|record| {
            WorkspaceMaintenanceKeyV1::from_durable_identity(&record.binding.source) == *key
                && record.binding.source.workspace_uuid() == previous_workspace_uuid
                && record.claims_stopped
                && record.work_drained
                && record.store_closed
        })
    }
    fn has_registered_generation(
        &self,
        key: &WorkspaceMaintenanceKeyV1,
        previous_workspace_uuid: &WorkspaceId,
    ) -> bool {
        mutex_lock(&self.state).generations.values().any(|record| {
            WorkspaceMaintenanceKeyV1::from_durable_identity(&record.binding.source) == *key
                && record.binding.source.workspace_uuid() == previous_workspace_uuid
        })
    }
    fn has_unclosed_generation(&self, key: &WorkspaceMaintenanceKeyV1) -> bool {
        mutex_lock(&self.state).generations.values().any(|record| {
            WorkspaceMaintenanceKeyV1::from_durable_identity(&record.binding.source) == *key
                && !(record.claims_stopped && record.work_drained && record.store_closed)
        })
    }
}
struct WorkspaceMaintenanceLeaseV1 {
    coordinator: Arc<WorkspaceMaintenanceCoordinatorV1>,
    key: Option<WorkspaceMaintenanceKeyV1>,
}
impl WorkspaceMaintenanceLeaseV1 {
    fn matches(&self, key: &WorkspaceMaintenanceKeyV1) -> bool {
        self.key.as_ref() == Some(key)
    }
}
impl Drop for WorkspaceMaintenanceLeaseV1 {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            mutex_lock(&self.coordinator.state).maintenance.remove(&key);
        }
    }
}
struct WorkspaceClaimLeaseV1 {
    coordinator: Arc<WorkspaceMaintenanceCoordinatorV1>,
    key: Option<WorkspaceMaintenanceKeyV1>,
}

impl Drop for WorkspaceClaimLeaseV1 {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            let mut state = mutex_lock(&self.coordinator.state);
            let count = state
                .claims
                .get_mut(&key)
                .expect("claim lifecycle lease must remain registered");
            *count -= 1;
            if *count == 0 {
                state.claims.remove(&key);
            }
        }
    }
}
struct WorkspaceRebindLeaseV1 {
    coordinator: Arc<WorkspaceMaintenanceCoordinatorV1>,
    key: Option<WorkspaceMaintenanceKeyV1>,
}

impl Drop for WorkspaceRebindLeaseV1 {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            mutex_lock(&self.coordinator.state).rebinds.remove(&key);
        }
    }
}

struct WorkspaceActivationLeaseV1 {
    coordinator: Arc<WorkspaceMaintenanceCoordinatorV1>,
    key: Option<WorkspaceMaintenanceKeyV1>,
}
impl Drop for WorkspaceActivationLeaseV1 {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            let mut state = mutex_lock(&self.coordinator.state);
            let count = state
                .activations
                .get_mut(&key)
                .expect("activation lifecycle lease must remain registered");
            *count -= 1;
            if *count == 0 {
                state.activations.remove(&key);
            }
        }
    }
}

fn process_maintenance_coordinator_v1() -> Arc<WorkspaceMaintenanceCoordinatorV1> {
    static COORDINATOR: OnceLock<Arc<WorkspaceMaintenanceCoordinatorV1>> = OnceLock::new();
    Arc::clone(COORDINATOR.get_or_init(|| Arc::new(WorkspaceMaintenanceCoordinatorV1::default())))
}

/// The durable receipt and active target scheduler produced by reset maintenance.
pub struct ResetAllCompletionV1 {
    scheduler: Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>,
    receipt: TerminalReceiptV1,
    marker: ResetMarkerV1,
}

impl ResetAllCompletionV1 {
    pub fn scheduler(&self) -> &Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>> {
        &self.scheduler
    }

    pub fn receipt(&self) -> &TerminalReceiptV1 {
        &self.receipt
    }

    /// Observational marker data for projecting the exact terminal after completion.
    pub fn marker(&self) -> &ResetMarkerV1 {
        &self.marker
    }
}
/// Manager-issued reset-source observation. The registry UUID is only a compare-and-swap
/// predecessor; `store_inspection` records the lossless Store-first inspection outcome.
#[derive(Clone)]
pub struct ResetSourceAuthorityV1 {
    worktree: podway_git::ValidatedWorktreeV1,
    registry_previous_workspace_uuid: WorkspaceId,
    persisted_identity: Option<podway_store::DurableWorktreeIdentityV1>,
    store_inspection: ResetStoreInspectionV1,
}

impl ResetSourceAuthorityV1 {
    pub fn registry_previous_workspace_uuid(&self) -> &WorkspaceId {
        &self.registry_previous_workspace_uuid
    }
    pub fn persisted_identity(&self) -> Option<&podway_store::DurableWorktreeIdentityV1> {
        self.persisted_identity.as_ref()
    }
    pub fn store_inspection(&self) -> &ResetStoreInspectionV1 {
        &self.store_inspection
    }
    pub(crate) fn routing_identity(&self) -> podway_store::DurableWorktreeIdentityV1 {
        self.persisted_identity.clone().unwrap_or_else(|| {
            podway_store::DurableWorktreeIdentityV1::new(
                self.worktree
                    .identity()
                    .common_directory_fingerprint()
                    .clone(),
                self.registry_previous_workspace_uuid.clone(),
                self.worktree
                    .identity()
                    .worktree_administration_fingerprint()
                    .clone(),
            )
        })
    }
    fn unavailable_store_proof(&self) -> Option<ValidatedUnavailableStoreV1> {
        let source = self.routing_identity();
        match &self.store_inspection {
            ResetStoreInspectionV1::Readable => None,
            ResetStoreInspectionV1::Absent => Some(ValidatedUnavailableStoreV1::absent(source)),
            ResetStoreInspectionV1::Unreadable(error) => Some(
                ValidatedUnavailableStoreV1::unreadable(source, error.clone()),
            ),
        }
    }
    fn matches_reset(&self, reset: &ResetWorkspaceResolutionV1) -> bool {
        same_git_root_evidence(&self.worktree, reset.worktree())
    }
}
/// One manager-owned, exclusive reset transaction. Its lease begins before source inspection and
/// remains held through marker handling, retirement verification, destructive maintenance, and
/// target activation.
pub(crate) struct WorkspaceResetMaintenanceV1<'a> {
    manager: &'a WorkspaceRuntimeManagerV1,
    selector: WorktreeSelectorV1,
    reset: ResetWorkspaceResolutionV1,
    authority: ResetSourceAuthorityV1,
    maintenance_key: WorkspaceMaintenanceKeyV1,
    lease: WorkspaceMaintenanceLeaseV1,
    filesystem_authority: ResetMaintenanceFilesystemTokenV1,
}

impl WorkspaceResetMaintenanceV1<'_> {
    pub(crate) fn authority(&self) -> &ResetSourceAuthorityV1 {
        &self.authority
    }

    pub(crate) fn discover_marker(&self) -> Result<Option<ResetMarkerV1>, WorkspaceRuntimeErrorV1> {
        self.reset
            .open_runtime_directory()
            .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?
            .read_reset_marker()
            .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)
    }
    pub(crate) fn validate_resume_request(
        &self,
        expected_idempotency_key: &IdempotencyKeyV1,
        expected_digest: &CanonicalRequestDigestV1,
    ) -> Result<(), WorkspaceRuntimeErrorV1> {
        let marker = self
            .discover_marker()?
            .ok_or(WorkspaceRuntimeErrorV1::ResetMarkerConflict)?;
        if !marker_matches_registry_generation(
            &marker,
            self.authority.registry_previous_workspace_uuid(),
        ) || marker.idempotency_key() != expected_idempotency_key
        {
            return Err(WorkspaceRuntimeErrorV1::ResetMarkerConflict);
        }
        if marker.request_digest() != expected_digest {
            return Err(WorkspaceRuntimeErrorV1::ResetIdempotencyConflict {
                existing: marker.request_digest().clone(),
                requested: expected_digest.clone(),
            });
        }
        self.manager
            .revalidate_reset_registry_predecessor(&self.reset, &self.authority)
    }

    pub(crate) fn active_old_scheduler(
        &self,
    ) -> Option<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>> {
        self.manager
            .active_old_scheduler_for_reset_authority(&self.authority)
    }

    pub(crate) fn activate_old_scheduler_for_preparation(
        &self,
        observation: WorkspaceRuntimeObservationV1,
    ) -> Result<
        Option<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>>,
        WorkspaceRuntimeErrorV1,
    > {
        if let Some(scheduler) = self.active_old_scheduler() {
            return Ok(Some(scheduler));
        }
        let Some(identity) = self.authority.persisted_identity() else {
            return Ok(None);
        };
        let resolved = self
            .manager
            .resolver
            .resolve_existing(self.selector.clone(), Some(identity.workspace_uuid()))
            .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        if WorkspaceMaintenanceKeyV1::from_worktree(resolved.worktree()) != self.maintenance_key {
            return Err(WorkspaceRuntimeErrorV1::MaintenanceInProgress);
        }
        self.manager
            .activate_existing_resolved_during_maintenance(
                resolved,
                observation,
                &self.maintenance_key,
                &self.lease,
            )
            .map(Some)
    }

    pub(crate) fn complete_prepared(
        &self,
        prepared: PreparedWorkspaceResetAllV1,
        observation: WorkspaceRuntimeObservationV1,
    ) -> Result<ResetAllCompletionV1, WorkspaceRuntimeErrorV1> {
        self.manager
            .complete_reset_all_authorized(ResetAllCompletionInputV1 {
                selector: &self.selector,
                reset: &self.reset,
                authority: &self.authority,
                prepared: Some(prepared),
                expected_request: None,
                observation,
                maintenance_key: &self.maintenance_key,
                lease: &self.lease,
                filesystem_authority: &self.filesystem_authority,
            })
    }

    pub(crate) fn resume(
        &self,
        expected_idempotency_key: &IdempotencyKeyV1,
        expected_digest: &CanonicalRequestDigestV1,
        observation: WorkspaceRuntimeObservationV1,
    ) -> Result<ResetAllCompletionV1, WorkspaceRuntimeErrorV1> {
        self.manager
            .complete_reset_all_authorized(ResetAllCompletionInputV1 {
                selector: &self.selector,
                reset: &self.reset,
                authority: &self.authority,
                prepared: None,
                expected_request: Some((expected_idempotency_key, expected_digest)),
                observation,
                maintenance_key: &self.maintenance_key,
                lease: &self.lease,
                filesystem_authority: &self.filesystem_authority,
            })
    }
}

struct ResetAllCompletionInputV1<'a> {
    selector: &'a WorktreeSelectorV1,
    reset: &'a ResetWorkspaceResolutionV1,
    authority: &'a ResetSourceAuthorityV1,
    prepared: Option<PreparedWorkspaceResetAllV1>,
    expected_request: Option<(&'a IdempotencyKeyV1, &'a CanonicalRequestDigestV1)>,
    observation: WorkspaceRuntimeObservationV1,
    maintenance_key: &'a WorkspaceMaintenanceKeyV1,
    lease: &'a WorkspaceMaintenanceLeaseV1,
    filesystem_authority: &'a ResetMaintenanceFilesystemTokenV1,
}

/// Read-only registry observations exposed outside the daemon composition.
#[derive(Clone, Copy)]
pub struct WorkspaceRegistryReadFacadeV1<'a> {
    registry: &'a RegistryStoreV1,
}

impl WorkspaceRegistryReadFacadeV1<'_> {
    pub fn registry_path(&self) -> &Path {
        self.registry.registry_path()
    }

    pub fn load(&self) -> Result<WorkspaceRegistryV1, RegistryErrorV1> {
        self.registry.load()
    }

    pub fn lookup(
        &self,
        workspace_uuid: &WorkspaceId,
    ) -> Result<Option<WorkspaceRegistryEntryV1>, RegistryErrorV1> {
        self.registry.lookup(workspace_uuid)
    }
}

/// Read-only scheduler observations exposed outside the daemon composition.
#[derive(Clone, Copy)]
pub struct WorkspaceSchedulerReadFacadeV1<'a> {
    registry: &'a WorkspaceSchedulerRegistryV1<WorkspaceSchedulerContextV1>,
}

impl WorkspaceSchedulerReadFacadeV1<'_> {
    pub fn get_active(
        &self,
        key: &WorkspaceSchedulerKeyV1,
    ) -> Option<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>> {
        self.registry.get_active(key)
    }
}

/// The daemon-owned workspace runtime composition.
///
/// Every activation uses the concrete two-pass `WorkspaceResolverV1`; the registry is updated only
/// after SQLite binding verification and never receives task, session, or queue data.
pub struct WorkspaceRuntimeManagerV1 {
    resolver: WorkspaceResolverV1<NativeGitResolverV1, SqliteWorkspaceBindingInspectorV1>,
    layout_initializer: WorkspaceLayoutInitializerV1,
    inspection_options: SqliteStoreOptionsV1,
    registry: RegistryStoreV1,
    schedulers: WorkspaceSchedulerRegistryV1<WorkspaceSchedulerContextV1>,
    maintenance: Arc<WorkspaceMaintenanceCoordinatorV1>,
    reset_crash_injection: ResetAllCrashInjectionV1,
}

impl WorkspaceRuntimeManagerV1 {
    pub fn new(paths: &ServiceRuntimePathsV1, inspection_options: SqliteStoreOptionsV1) -> Self {
        Self::with_observability(paths, inspection_options, None)
    }

    /// Constructs a manager whose registry and scheduler lifecycle share one event emitter.
    pub fn with_observability(
        paths: &ServiceRuntimePathsV1,
        inspection_options: SqliteStoreOptionsV1,
        observability: Option<ObservabilityEmitterV1>,
    ) -> Self {
        let (registry, schedulers) = match observability {
            Some(emitter) => (
                RegistryStoreV1::with_observability(paths, Some(emitter.clone())),
                WorkspaceSchedulerRegistryV1::with_observability(Some(emitter)),
            ),
            None => (
                RegistryStoreV1::new(paths),
                WorkspaceSchedulerRegistryV1::new(),
            ),
        };
        Self {
            resolver: WorkspaceResolverV1::new(
                NativeGitResolverV1::new(),
                SqliteWorkspaceBindingInspectorV1::new(inspection_options.clone()),
            ),
            layout_initializer: WorkspaceLayoutInitializerV1::new(),
            inspection_options,
            registry,
            schedulers,
            maintenance: process_maintenance_coordinator_v1(),
            reset_crash_injection: ResetAllCrashInjectionV1::default(),
        }
    }
    /// Constructs an isolated test manager with one deliberately injected crash boundary.
    ///
    /// Production composition uses [`Self::new`] or [`Self::with_observability`], neither of which
    /// accepts ambient failpoint configuration.
    pub fn with_reset_crash_boundary_for_tests(
        paths: &ServiceRuntimePathsV1,
        inspection_options: SqliteStoreOptionsV1,
        boundary: ResetAllCrashBoundaryV1,
    ) -> Self {
        let mut manager = Self::new(paths, inspection_options);
        manager.reset_crash_injection = ResetAllCrashInjectionV1(Some(boundary));
        manager
    }

    pub fn resolver(
        &self,
    ) -> &WorkspaceResolverV1<NativeGitResolverV1, SqliteWorkspaceBindingInspectorV1> {
        &self.resolver
    }

    pub const fn layout_initializer(&self) -> WorkspaceLayoutInitializerV1 {
        self.layout_initializer
    }

    pub fn registry(&self) -> WorkspaceRegistryReadFacadeV1<'_> {
        WorkspaceRegistryReadFacadeV1 {
            registry: &self.registry,
        }
    }

    pub fn schedulers(&self) -> WorkspaceSchedulerReadFacadeV1<'_> {
        WorkspaceSchedulerReadFacadeV1 {
            registry: &self.schedulers,
        }
    }

    pub(crate) fn scheduler_registry(
        &self,
    ) -> &WorkspaceSchedulerRegistryV1<WorkspaceSchedulerContextV1> {
        &self.schedulers
    }
    /// Derives the stable Git key read-only, acquires the exclusive maintenance lease, and only
    /// then inspects the registry and source Store. The returned transaction is the sole reset
    /// mutation authority and cannot release its lease before target activation.
    pub(crate) fn begin_reset_maintenance(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<WorkspaceResetMaintenanceV1<'_>, WorkspaceRuntimeErrorV1> {
        let reset = self
            .resolver
            .resolve_for_reset(selector.clone())
            .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        let maintenance_key = WorkspaceMaintenanceKeyV1::from_worktree(reset.worktree());
        let lease = self
            .maintenance
            .acquire(maintenance_key.clone())
            .ok_or(WorkspaceRuntimeErrorV1::MaintenanceInProgress)?;
        let authority = self.registered_reset_source_authority(selector.clone())?;
        if !authority.matches_reset(&reset) {
            return Err(WorkspaceRuntimeErrorV1::ResetSourceAmbiguous);
        }
        Ok(WorkspaceResetMaintenanceV1 {
            manager: self,
            selector,
            reset,
            authority,
            maintenance_key,
            lease,
            filesystem_authority: ResetMaintenanceFilesystemTokenV1::issue(),
        })
    }
    /// Looks for a reset marker using only fresh Git containment evidence and the validated
    /// runtime-directory descriptor. It intentionally never opens the workspace Store.
    pub fn discover_reset_marker(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<Option<ResetMarkerV1>, WorkspaceRuntimeErrorV1> {
        let reset = self
            .resolver
            .resolve_for_reset(selector)
            .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        reset
            .open_runtime_directory()
            .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?
            .read_reset_marker()
            .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)
    }

    /// Binds reset source ownership to durable Store evidence when readable while retaining the
    /// registry UUID only as the replacement compare-and-swap predecessor.
    pub fn registered_reset_source_authority(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<ResetSourceAuthorityV1, WorkspaceRuntimeErrorV1> {
        let reset = self
            .resolver
            .resolve_for_reset(selector.clone())
            .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        let registry = self
            .registry
            .load()
            .map_err(WorkspaceRuntimeErrorV1::Registry)?;
        let entries = registry
            .workspaces()
            .iter()
            .filter(|entry| entry.last_known_root() == reset.workspace_root())
            .collect::<Vec<_>>();
        let entry = match entries.as_slice() {
            [entry] => entry,
            [] => return Err(WorkspaceRuntimeErrorV1::ResetSourceNotRegistered),
            _ => return Err(WorkspaceRuntimeErrorV1::ResetSourceAmbiguous),
        };
        let (persisted_identity, store_inspection) = match self
            .resolver
            .resolve_existing(selector, Some(entry.workspace_uuid()))
        {
            Ok(resolved) => (
                Some(resolved.store_identity().clone()),
                ResetStoreInspectionV1::Readable,
            ),
            Err(WorkspaceResolutionErrorV1::ExistingBindingMissing) => {
                (None, ResetStoreInspectionV1::Absent)
            }
            Err(WorkspaceResolutionErrorV1::BindingInspection {
                source: WorkspaceBindingInspectionErrorV1::Store(error),
            }) => (None, ResetStoreInspectionV1::Unreadable(error)),
            Err(WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch { expected, actual }) => {
                let marker = reset
                    .open_runtime_directory()
                    .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?
                    .read_reset_marker()
                    .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?;
                if marker.as_ref().is_some_and(|marker| {
                    marker.target_workspace_uuid() == &actual
                        && marker.previous_workspace_uuid() == entry.workspace_uuid()
                }) {
                    (None, ResetStoreInspectionV1::Absent)
                } else {
                    return Err(WorkspaceRuntimeErrorV1::Resolution(
                        WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch {
                            expected,
                            actual,
                        },
                    ));
                }
            }
            Err(error) => return Err(WorkspaceRuntimeErrorV1::Resolution(error)),
        };
        Ok(ResetSourceAuthorityV1 {
            worktree: reset.worktree().clone(),
            registry_previous_workspace_uuid: entry.workspace_uuid().clone(),
            persisted_identity,
            store_inspection,
        })
    }
    /// Converts the manager's exact reset-source inspection into the only no-Store preparation
    /// capability. A readable inspection deliberately yields no proof.
    pub(crate) fn unavailable_reset_store_proof(
        &self,
        source: &ResetSourceAuthorityV1,
    ) -> Option<ValidatedUnavailableStoreV1> {
        source.unavailable_store_proof()
    }
    /// Returns persisted reset-source identity only when the old Store remains readable.
    ///
    /// Callers that support unreadable-Store recovery must retain the returned
    /// [`ResetSourceAuthorityV1`] instead of manufacturing a durable identity from registry data.
    pub fn registered_reset_source_identity(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<podway_store::DurableWorktreeIdentityV1, WorkspaceRuntimeErrorV1> {
        self.registered_reset_source_authority(selector)?
            .persisted_identity()
            .cloned()
            .ok_or(WorkspaceRuntimeErrorV1::Resolution(
                WorkspaceResolutionErrorV1::ExistingBindingMissing,
            ))
    }
    /// Returns an active scheduler only after its persisted context identity agrees with the reset
    /// authority. The unreadable path uses fresh Git fingerprints solely to route the lookup; the
    /// returned scheduler is accepted only after its persisted binding verifies that route.
    pub fn active_old_scheduler_for_reset_authority(
        &self,
        source: &ResetSourceAuthorityV1,
    ) -> Option<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>> {
        let routing_identity = source.routing_identity();
        self.schedulers
            .get_active(&WorkspaceSchedulerKeyV1::from_durable_identity(
                &routing_identity,
            ))
            .filter(|scheduler| {
                let context = scheduler.context_snapshot();
                same_git_root_evidence(context.git_evidence(), &source.worktree)
                    && context.binding().identity().workspace_uuid()
                        == source.registry_previous_workspace_uuid()
                    && source
                        .persisted_identity()
                        .is_none_or(|persisted| context.binding().identity() == persisted)
            })
    }
    /// Returns only the active scheduler for a persisted reset source identity. It accepts no path
    /// and cannot create or rebind a scheduler.
    pub fn active_old_scheduler_for_reset(
        &self,
        source: &podway_store::DurableWorktreeIdentityV1,
    ) -> Option<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>> {
        self.schedulers
            .get_active(&WorkspaceSchedulerKeyV1::from_durable_identity(source))
            .filter(|scheduler| scheduler.context_snapshot().binding().identity() == source)
    }
    /// Performs marker-bound destructive maintenance while the caller retains the one exclusive
    /// reset lease acquired before source inspection and Store-first preparation.
    fn complete_reset_all_authorized(
        &self,
        input: ResetAllCompletionInputV1<'_>,
    ) -> Result<ResetAllCompletionV1, WorkspaceRuntimeErrorV1> {
        let ResetAllCompletionInputV1 {
            selector,
            reset,
            authority,
            prepared,
            expected_request,
            observation,
            maintenance_key,
            lease,
            filesystem_authority,
        } = input;
        if !lease.matches(maintenance_key)
            || WorkspaceMaintenanceKeyV1::from_worktree(reset.worktree()) != *maintenance_key
            || !authority.matches_reset(reset)
        {
            return Err(WorkspaceRuntimeErrorV1::MaintenanceInProgress);
        }
        match prepared.as_ref() {
            Some(prepared)
                if prepared.previous_workspace_uuid()
                    != authority.registry_previous_workspace_uuid()
                    || prepared.marker().previous_workspace_uuid()
                        != authority.registry_previous_workspace_uuid()
                    || !prepared.matches_source(&authority.routing_identity()) =>
            {
                return Err(WorkspaceRuntimeErrorV1::ResetMarkerConflict);
            }
            _ => {}
        }

        // Every source and predecessor observation used to prepare the operation is renewed under
        // this lease before a marker can become durable authority.
        self.revalidate_reset_source_authority(selector, reset, authority)?;
        self.revalidate_reset_registry_predecessor(reset, authority)?;

        let runtime_directory = reset
            .open_runtime_directory()
            .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?;
        let existing_marker = runtime_directory
            .read_reset_marker()
            .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?;
        let registered_workspace_uuid = authority.registry_previous_workspace_uuid();
        if existing_marker.is_none() && prepared.is_some() {
            self.require_scheduler_retired_for_reset(
                reset,
                registered_workspace_uuid,
                authority.persisted_identity(),
            )?;
        }
        let marker_was_present = existing_marker.is_some();
        let marker = match (existing_marker, prepared.as_ref(), expected_request) {
            (Some(existing), Some(prepared), _) => {
                if existing != *prepared.marker() {
                    if existing.idempotency_key() == prepared.marker().idempotency_key()
                        && existing.request_digest() != prepared.marker().request_digest()
                    {
                        return Err(WorkspaceRuntimeErrorV1::ResetIdempotencyConflict {
                            existing: existing.request_digest().clone(),
                            requested: prepared.marker().request_digest().clone(),
                        });
                    }
                    return Err(WorkspaceRuntimeErrorV1::ResetMarkerConflict);
                }
                existing
            }
            (Some(existing), None, Some((expected_key, expected_digest))) => {
                if existing.idempotency_key() != expected_key {
                    return Err(WorkspaceRuntimeErrorV1::ResetMarkerConflict);
                }
                if existing.request_digest() != expected_digest {
                    return Err(WorkspaceRuntimeErrorV1::ResetIdempotencyConflict {
                        existing: existing.request_digest().clone(),
                        requested: expected_digest.clone(),
                    });
                }
                existing
            }
            (Some(existing), None, None) => existing,
            (None, Some(prepared), _) => {
                if prepared.marker().target_workspace_uuid() == registered_workspace_uuid {
                    return Err(WorkspaceRuntimeErrorV1::ResetMarkerConflict);
                }
                runtime_directory
                    .publish_reset_marker(filesystem_authority, prepared.marker())
                    .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?;
                self.reset_crash_injection
                    .abort_at(ResetAllCrashBoundaryV1::MarkerCreated);
                prepared.marker().clone()
            }
            (None, None, _) => return Err(WorkspaceRuntimeErrorV1::ResetMarkerConflict),
        };
        if !marker_matches_registry_generation(&marker, registered_workspace_uuid) {
            return Err(WorkspaceRuntimeErrorV1::ResetMarkerConflict);
        }
        if marker_was_present {
            self.require_scheduler_retired_for_reset(
                reset,
                marker.previous_workspace_uuid(),
                (marker.previous_workspace_uuid() == registered_workspace_uuid)
                    .then_some(authority.persisted_identity())
                    .flatten(),
            )?;
        }

        let receipt = if !marker_was_present {
            self.revalidate_reset_registry_predecessor(reset, authority)?;
            runtime_directory
                .remove_reset_database_files(filesystem_authority, &marker)
                .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?;
            self.reset_crash_injection
                .abort_at(ResetAllCrashBoundaryV1::OldDatabaseDeleted);
            let receipt = self
                .seed_reset_target(reset, &marker, observation.store_now())
                .map_err(WorkspaceRuntimeErrorV1::Store)?;
            self.reset_crash_injection
                .abort_at(ResetAllCrashBoundaryV1::NewTargetDatabaseCreated);
            receipt
        } else {
            // A resumed marker verifies its exact target first. It is never recreated merely from
            // marker-derived receipt data, and a predecessor-bound target remains fail-closed.
            self.revalidate_reset_registry_predecessor(reset, authority)?;
            match self.seed_reset_target(reset, &marker, observation.store_now()) {
                Ok(receipt) => receipt,
                Err(error) if reset_seed_requires_fixed_replacement(&error) => {
                    self.revalidate_reset_registry_predecessor(reset, authority)?;
                    runtime_directory
                        .remove_reset_database_files(filesystem_authority, &marker)
                        .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?;
                    self.reset_crash_injection
                        .abort_at(ResetAllCrashBoundaryV1::OldDatabaseDeleted);
                    let receipt = self
                        .seed_reset_target(reset, &marker, observation.store_now())
                        .map_err(WorkspaceRuntimeErrorV1::Store)?;
                    self.reset_crash_injection
                        .abort_at(ResetAllCrashBoundaryV1::NewTargetDatabaseCreated);
                    receipt
                }
                Err(error) => return Err(WorkspaceRuntimeErrorV1::Store(error)),
            }
        };
        self.revalidate_reset_registry_predecessor(reset, authority)?;
        if registered_workspace_uuid == marker.previous_workspace_uuid() {
            self.registry
                .replace_for_reset(
                    marker.previous_workspace_uuid(),
                    marker.target_workspace_uuid().clone(),
                    reset.workspace_root().clone(),
                    observation.registry_seen_at().clone(),
                )
                .map_err(WorkspaceRuntimeErrorV1::Registry)?;
        }
        runtime_directory
            .remove_reset_marker(filesystem_authority, &marker)
            .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?;
        let resolved = self
            .resolver
            .resolve_existing(selector.clone(), Some(marker.target_workspace_uuid()))
            .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        let scheduler = self.activate_existing_resolved_during_maintenance(
            resolved,
            observation,
            maintenance_key,
            lease,
        )?;
        Ok(ResetAllCompletionV1 {
            scheduler,
            receipt,
            marker,
        })
    }

    /// Creates a descriptor-safe worktree layout and atomically establishes its first Store binding.
    ///
    /// This operation only creates the workspace database. It does not admit a command or create a
    /// session. Once the binding exists, the same existing-workspace path used by ordinary requests
    /// performs the bound revalidation and scheduler activation.
    pub fn bootstrap(
        &self,
        selector: WorktreeSelectorV1,
        observation: WorkspaceRuntimeObservationV1,
    ) -> Result<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>, WorkspaceRuntimeErrorV1>
    {
        // Resolve Git-only containment first so lifecycle serialization begins before bootstrap
        // inspects or opens SQLite, mutates the layout, or publishes a scheduler.
        let lifecycle = self
            .resolver
            .resolve_for_reset(selector.clone())
            .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        let maintenance_key = WorkspaceMaintenanceKeyV1::from_worktree(lifecycle.worktree());
        let _activation = self
            .maintenance
            .acquire_activation(maintenance_key.clone())
            .ok_or(WorkspaceRuntimeErrorV1::MaintenanceInProgress)?;
        let candidate = self
            .resolver
            .resolve_bootstrap(selector.clone())
            .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        if WorkspaceMaintenanceKeyV1::from_worktree(candidate.worktree()) != maintenance_key
            || !same_git_root_evidence(lifecycle.worktree(), candidate.worktree())
        {
            return Err(WorkspaceRuntimeErrorV1::MaintenanceInProgress);
        }
        self.layout_initializer
            .initialize_with_config(candidate.worktree(), DEFAULT_WORKSPACE_CONFIG_YAML_V1)
            .map_err(WorkspaceRuntimeErrorV1::Layout)?;
        self.ensure_activation_marker_clear(&candidate)?;

        // Admit the configuration before Store creation. The layout either created the canonical
        // default or descriptor-validated an existing regular config file.
        let config = read_admitted_workspace_config(&candidate)?;
        let options = self.store_options_for(&config)?;
        self.revalidate_before_store_open(&candidate)?;
        let store = SqliteStoreV1::open(
            candidate.database_path(),
            candidate.workspace_root(),
            candidate.store_identity().clone(),
            options.clone(),
            observation.store_now(),
        )
        .map_err(WorkspaceRuntimeErrorV1::Store)?;
        let expected_binding = WorkspaceBindingV1::new(
            candidate.store_identity().clone(),
            candidate.workspace_root().clone(),
        );
        require_exact_binding(candidate.database_path(), &options, &expected_binding)?;
        drop(store);

        let bound = self
            .resolver
            .resolve_existing(selector, Some(candidate.store_identity().workspace_uuid()))
            .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        self.activate_existing_resolved_guarded(bound, observation, false)
    }

    /// Resolves an already-bound workspace and returns its durable-identity scheduler generation.
    pub fn resolve_existing(
        &self,
        selector: WorktreeSelectorV1,
        expected_workspace_id: Option<&WorkspaceId>,
        observation: WorkspaceRuntimeObservationV1,
    ) -> Result<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>, WorkspaceRuntimeErrorV1>
    {
        let resolved = self
            .resolver
            .resolve_existing(selector, expected_workspace_id)
            .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        self.activate_existing_resolved(resolved, observation)
    }
    /// Resolves an existing workspace for a read-only route.
    ///
    /// Unlike [`Self::resolve_existing`], this path never creates or rebinds a scheduler, opens
    /// SQLite for mutation, or refreshes registry metadata. It may reuse an active scheduler only
    /// after comparing every identity-bearing context field to the new two-pass resolution.
    pub fn resolve_existing_readonly(
        &self,
        selector: WorktreeSelectorV1,
        expected_workspace_id: Option<&WorkspaceId>,
    ) -> Result<ReadonlyWorkspaceResolutionV1, WorkspaceRuntimeErrorV1> {
        let resolved = self
            .resolver
            .resolve_existing(selector, expected_workspace_id)
            .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        let config = read_admitted_workspace_config(&resolved)?;
        let store_options = self.store_options_for(&config)?;
        let binding = WorkspaceBindingV1::new(
            resolved.store_identity().clone(),
            resolved.workspace_root().clone(),
        );
        let runtime_directory_path = runtime_directory_path(&resolved)?;
        let active_scheduler =
            self.schedulers
                .get_active(resolved.scheduler_key())
                .filter(|scheduler| {
                    let context = scheduler.context_snapshot();
                    database_file_identity(resolved.database_path())
                        .is_some_and(|identity| identity == *context.database_file_identity())
                        && context.binding() == &binding
                        && context.database_path() == resolved.database_path()
                        && context.runtime_directory_path() == runtime_directory_path.as_path()
                        && context.git_evidence() == resolved.worktree()
                });
        Ok(ReadonlyWorkspaceResolutionV1 {
            binding,
            workspace_root: resolved.workspace_root().clone(),
            database_path: resolved.database_path().to_path_buf(),
            store_options,
            worktree: resolved.worktree().clone(),
            active_scheduler,
        })
    }

    /// Revalidates the Git/Store evidence held by an active scheduler without treating registry
    /// metadata as authority. A missing, deleted, or durable-key-mismatched root requires
    /// retirement.
    pub fn revalidate_scheduler(
        &self,
        scheduler: &Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>,
    ) -> Result<WorkspaceSchedulerRevalidationV1, WorkspaceRuntimeErrorV1> {
        let context = scheduler.context_snapshot();
        if database_file_identity(context.database_path())
            .is_some_and(|identity| &identity != context.database_file_identity())
        {
            return Ok(WorkspaceSchedulerRevalidationV1::RetireRequired {
                key: scheduler.key().clone(),
                generation: scheduler.generation(),
                source: WorkspaceResolutionErrorV1::RuntimeDatabasePathChangedDuringResolution,
            });
        }

        let selector = WorktreeSelectorV1::new(
            WORKTREE_SELECTOR_VERSION_V1,
            Some(context.git_evidence().identity().clone()),
            context.git_evidence().roots().worktree_root().clone(),
        )
        .map_err(|source| WorkspaceResolutionErrorV1::Selector { source })
        .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        match self.resolver.resolve_existing(
            selector,
            Some(context.binding().identity().workspace_uuid()),
        ) {
            Ok(resolved) => {
                let expected = scheduler.key().clone();
                let actual = resolved.scheduler_key().clone();
                if expected != actual {
                    return Ok(WorkspaceSchedulerRevalidationV1::RetireRequired {
                        key: expected,
                        generation: scheduler.generation(),
                        source: WorkspaceResolutionErrorV1::RevalidatedStoreIdentityMismatch {
                            expected: Box::new(context.binding().identity().clone()),
                            actual: Box::new(resolved.store_identity().clone()),
                        },
                    });
                }
                Ok(WorkspaceSchedulerRevalidationV1::Current)
            }
            Err(source) => Ok(WorkspaceSchedulerRevalidationV1::RetireRequired {
                key: scheduler.key().clone(),
                generation: scheduler.generation(),
                source,
            }),
        }
    }

    fn require_scheduler_retired_for_reset(
        &self,
        reset: &ResetWorkspaceResolutionV1,
        previous_workspace_uuid: &WorkspaceId,
        persisted_identity: Option<&podway_store::DurableWorktreeIdentityV1>,
    ) -> Result<(), WorkspaceRuntimeErrorV1> {
        let maintenance_key = WorkspaceMaintenanceKeyV1::from_worktree(reset.worktree());
        if self.maintenance.has_unclosed_generation(&maintenance_key) {
            return Err(WorkspaceRuntimeErrorV1::ResetSchedulerRetirement);
        }
        match persisted_identity {
            Some(persisted_identity)
                if self
                    .schedulers
                    .get_active(&WorkspaceSchedulerKeyV1::from_durable_identity(
                        persisted_identity,
                    ))
                    .is_some() =>
            {
                return Err(WorkspaceRuntimeErrorV1::ResetSchedulerRetirement);
            }
            _ => {}
        }
        if self
            .maintenance
            .has_registered_generation(&maintenance_key, previous_workspace_uuid)
            && !self
                .maintenance
                .has_retirement_receipt(&maintenance_key, previous_workspace_uuid)
        {
            return Err(WorkspaceRuntimeErrorV1::ResetSchedulerRetirement);
        }
        Ok(())
    }
    /// Repeats the lossless Store/registry source inspection under the held lease. A prepared
    /// capability is invalidated rather than reinterpreted if any source authority changed.
    fn revalidate_reset_source_authority(
        &self,
        selector: &WorktreeSelectorV1,
        reset: &ResetWorkspaceResolutionV1,
        authority: &ResetSourceAuthorityV1,
    ) -> Result<(), WorkspaceRuntimeErrorV1> {
        let fresh = self.registered_reset_source_authority(selector.clone())?;
        if !fresh.matches_reset(reset)
            || fresh.registry_previous_workspace_uuid()
                != authority.registry_previous_workspace_uuid()
            || fresh.persisted_identity() != authority.persisted_identity()
            || fresh.store_inspection() != authority.store_inspection()
        {
            return Err(WorkspaceRuntimeErrorV1::ResetRegistryPredecessorStale);
        }
        Ok(())
    }
    /// Rechecks the exact registry predecessor while the maintenance lease is held. This precedes
    /// every reset seed or fixed-file deletion so cloned/stale source observations fail closed.
    fn revalidate_reset_registry_predecessor(
        &self,
        reset: &ResetWorkspaceResolutionV1,
        authority: &ResetSourceAuthorityV1,
    ) -> Result<(), WorkspaceRuntimeErrorV1> {
        let registry = self
            .registry
            .load()
            .map_err(WorkspaceRuntimeErrorV1::Registry)?;
        let entries = registry
            .workspaces()
            .iter()
            .filter(|entry| entry.last_known_root() == reset.workspace_root())
            .collect::<Vec<_>>();
        let [entry] = entries.as_slice() else {
            return Err(WorkspaceRuntimeErrorV1::ResetRegistryPredecessorStale);
        };
        if entry.workspace_uuid() != authority.registry_previous_workspace_uuid() {
            return Err(WorkspaceRuntimeErrorV1::ResetRegistryPredecessorStale);
        }
        Ok(())
    }

    fn seed_reset_target(
        &self,
        reset: &ResetWorkspaceResolutionV1,
        marker: &ResetMarkerV1,
        now: UnixMillis,
    ) -> Result<TerminalReceiptV1, StoreErrorV1> {
        SqliteStoreV1::seed_or_verify_reset_target(
            reset.database_path(),
            reset.workspace_root(),
            reset.target_identity(marker.target_workspace_uuid().clone()),
            self.inspection_options.clone(),
            reset_seed_request(reset, marker)?,
            DomainResult::WorkspaceReset {
                workspace_id: marker.target_workspace_uuid().clone(),
                revision: RevisionV1::ZERO,
            },
            now,
        )
    }

    fn ensure_activation_marker_clear(
        &self,
        resolved: &ResolvedWorkspaceV1,
    ) -> Result<(), WorkspaceRuntimeErrorV1> {
        let runtime_directory = ValidatedRuntimeDirectoryV1::open(resolved.worktree())
            .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?;
        if runtime_directory
            .read_reset_marker()
            .map_err(WorkspaceRuntimeErrorV1::RuntimeDirectory)?
            .is_some()
        {
            return Err(WorkspaceRuntimeErrorV1::MaintenanceInProgress);
        }
        Ok(())
    }
    fn activate_existing_resolved(
        &self,
        resolved: ResolvedWorkspaceV1,
        observation: WorkspaceRuntimeObservationV1,
    ) -> Result<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>, WorkspaceRuntimeErrorV1>
    {
        let maintenance_key = WorkspaceMaintenanceKeyV1::from_worktree(resolved.worktree());
        let _activation = self
            .maintenance
            .acquire_activation(maintenance_key)
            .ok_or(WorkspaceRuntimeErrorV1::MaintenanceInProgress)?;
        self.ensure_activation_marker_clear(&resolved)?;
        self.activate_existing_resolved_guarded(resolved, observation, false)
    }
    fn activate_existing_resolved_during_maintenance(
        &self,
        resolved: ResolvedWorkspaceV1,
        observation: WorkspaceRuntimeObservationV1,
        maintenance_key: &WorkspaceMaintenanceKeyV1,
        lease: &WorkspaceMaintenanceLeaseV1,
    ) -> Result<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>, WorkspaceRuntimeErrorV1>
    {
        if !lease.matches(maintenance_key)
            || WorkspaceMaintenanceKeyV1::from_worktree(resolved.worktree()) != *maintenance_key
        {
            return Err(WorkspaceRuntimeErrorV1::MaintenanceInProgress);
        }
        self.activate_existing_resolved_guarded(resolved, observation, true)
    }
    fn activate_existing_resolved_guarded(
        &self,
        resolved: ResolvedWorkspaceV1,
        observation: WorkspaceRuntimeObservationV1,
        maintenance_authorized: bool,
    ) -> Result<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>, WorkspaceRuntimeErrorV1>
    {
        let maintenance_key = WorkspaceMaintenanceKeyV1::from_worktree(resolved.worktree());
        let key = resolved.scheduler_key().clone();
        if let Some(scheduler) = self.schedulers.get_active(&key) {
            let current = scheduler.context_snapshot();
            let expected_binding = WorkspaceBindingV1::new(
                resolved.store_identity().clone(),
                resolved.workspace_root().clone(),
            );
            let expected_runtime_directory = runtime_directory_path(&resolved)?;
            let current_database_identity = database_file_identity(resolved.database_path());
            if current.database_path() == resolved.database_path()
                && current_database_identity.as_ref() != Some(current.database_file_identity())
            {
                return Err(WorkspaceRuntimeErrorV1::Resolution(
                    WorkspaceResolutionErrorV1::RuntimeDatabasePathChangedDuringResolution,
                ));
            }
            if current.binding() == &expected_binding
                && current.database_path() == resolved.database_path()
                && current_database_identity.as_ref() == Some(current.database_file_identity())
                && current.runtime_directory_path() == expected_runtime_directory
                && current.git_evidence() == resolved.worktree()
            {
                refresh_registry_metadata(
                    &self.registry,
                    &resolved,
                    current.binding(),
                    expected_binding.identity().workspace_uuid().clone(),
                    observation.registry_seen_at().clone(),
                )?;
                current
                    .bind_maintenance_generation(scheduler.key().clone(), scheduler.generation());
                return Ok(scheduler);
            }
        }

        let _rebind = if maintenance_authorized {
            None
        } else {
            Some(
                self.maintenance
                    .acquire_rebind(maintenance_key.clone())
                    .ok_or(WorkspaceRuntimeErrorV1::MaintenanceInProgress)?,
            )
        };

        let config = read_admitted_workspace_config(&resolved)?;
        let options = self.store_options_for(&config)?;
        let binding_before_open =
            SqliteStoreV1::inspect_workspace_binding(resolved.database_path(), &options)
                .map_err(WorkspaceRuntimeErrorV1::Store)?
                .ok_or_else(|| WorkspaceRuntimeErrorV1::BindingDisappeared {
                    database_path: resolved.database_path().to_path_buf(),
                })?;
        validate_pre_open_binding(&resolved, &binding_before_open)?;
        self.revalidate_before_store_open(&resolved)?;

        // This is the first Store mutation-capable operation in the existing path, and it follows
        // resolver observations, binding reinspection, and fresh exact Git/root revalidation.
        let store = Arc::new(WorkspaceStoreSlotV1::new(
            SqliteStoreV1::open(
                resolved.database_path(),
                resolved.workspace_root(),
                resolved.store_identity().clone(),
                options.clone(),
                observation.store_now(),
            )
            .map_err(WorkspaceRuntimeErrorV1::Store)?,
        ));
        let expected_binding = WorkspaceBindingV1::new(
            resolved.store_identity().clone(),
            resolved.workspace_root().clone(),
        );
        let binding = require_exact_binding(resolved.database_path(), &options, &expected_binding)?;
        let database = ValidatedDatabaseFileV1 {
            path: resolved.database_path().to_path_buf(),
            identity: database_file_identity(resolved.database_path()).ok_or(
                WorkspaceRuntimeErrorV1::Resolution(
                    WorkspaceResolutionErrorV1::RuntimeDatabasePathChangedDuringResolution,
                ),
            )?,
        };
        refresh_registry_metadata(
            &self.registry,
            &resolved,
            &binding_before_open,
            binding.identity().workspace_uuid().clone(),
            observation.registry_seen_at().clone(),
        )?;

        let context = WorkspaceSchedulerContextV1::new(WorkspaceSchedulerContextInputV1 {
            binding,
            database,
            runtime_directory_path: runtime_directory_path(&resolved)?,
            store,
            store_options: options,
            config,
            git_evidence: resolved.worktree().clone(),
            maintenance: Arc::clone(&self.maintenance),
        });
        let scheduler = self
            .schedulers
            .get_or_create(key, {
                let context = context.clone();
                move || context
            })
            .map_err(WorkspaceRuntimeErrorV1::Scheduler)?;
        let current = scheduler.context_snapshot();
        if context_requires_rebind(&current, &context) {
            let prior =
                scheduler.rebind_context_serialized_after(Arc::new(context), |previous| {
                    previous.stop_claims();
                    previous.record_work_drained();
                    previous
                        .close_store_for_maintenance()
                        .map_err(WorkspaceRuntimeErrorV1::Store)
                })?;
            drop(prior);
        }
        scheduler
            .context_snapshot()
            .bind_maintenance_generation(scheduler.key().clone(), scheduler.generation());
        Ok(scheduler)
    }
    /// Rechecks the exact Git/root evidence immediately before a Store operation can mutate state.
    ///
    /// The selector is derived solely from a prior validated worktree snapshot, never from an
    /// independently supplied path. A root replacement therefore cannot reuse stale evidence.
    fn revalidate_before_store_open(
        &self,
        resolved: &ResolvedWorkspaceV1,
    ) -> Result<(), WorkspaceRuntimeErrorV1> {
        let selector = WorktreeSelectorV1::new(
            WORKTREE_SELECTOR_VERSION_V1,
            Some(resolved.worktree().identity().clone()),
            resolved.worktree().roots().worktree_root().clone(),
        )
        .map_err(|source| WorkspaceResolutionErrorV1::Selector { source })
        .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        let actual = self
            .resolver
            .git_resolver()
            .resolve(selector)
            .map_err(|source| {
                WorkspaceRuntimeErrorV1::Resolution(WorkspaceResolutionErrorV1::Git {
                    observation: WorkspaceGitObservationV1::BoundRevalidation,
                    source,
                })
            })?;
        if !same_git_root_evidence(resolved.worktree(), &actual) {
            return Err(WorkspaceRuntimeErrorV1::Resolution(
                WorkspaceResolutionErrorV1::Git {
                    observation: WorkspaceGitObservationV1::BoundRevalidation,
                    source: GitResolveErrorV1::IdentityMismatch {
                        expected: Box::new(resolved.worktree().identity().clone()),
                        actual: Box::new(actual.identity().clone()),
                    },
                },
            ));
        }
        Ok(())
    }

    fn store_options_for(
        &self,
        config: &WorkspaceConfigV1,
    ) -> Result<SqliteStoreOptionsV1, WorkspaceRuntimeErrorV1> {
        SqliteStoreOptionsV1::new(u32::from(config.job_queue.max_pending))
            .and_then(|options| {
                options.with_busy_timeout_ms(self.inspection_options.busy_timeout_ms())
            })
            .map_err(WorkspaceRuntimeErrorV1::StoreOptions)
    }
}
fn reset_seed_request(
    reset: &ResetWorkspaceResolutionV1,
    marker: &ResetMarkerV1,
) -> Result<AdmitRequestV1, StoreErrorV1> {
    let preconditions = RevisionAttemptItemPreconditionsV1::new(None, None, None, None)
        .expect("empty reset-all preconditions are valid");
    let request = AdmitRequestV1::new(
        podway_store::CommandV1::WorkspaceResetAll,
        marker.idempotency_key().clone(),
        marker.operation_id().clone(),
        preconditions,
        marker.request_digest().clone(),
        marker.submitted_at_ms(),
    );
    let Some(response_request_id) = marker.response_request_id() else {
        return Ok(request);
    };
    let response_context = PersistedResponseContextV1::new(
        response_request_id.as_str(),
        "workspace.reset_all",
        marker.target_workspace_uuid().clone(),
        reset.worktree().roots().worktree_root().display().as_str(),
        1,
    )
    .map_err(|_| StoreErrorV1::InternalInvariantViolationV1 {
        invariant: StoreInvariantV1::ResetSeed,
    })?;
    Ok(request.with_response_context(response_context))
}

fn reset_seed_requires_fixed_replacement(error: &StoreErrorV1) -> bool {
    match error {
        StoreErrorV1::CorruptStateV1 { .. }
        | StoreErrorV1::StorageIntegrityV1 { .. }
        | StoreErrorV1::NewerStateV1 { .. } => true,
        StoreErrorV1::PrimaryOperationAndCleanupFailureV1 { .. } => false,
        StoreErrorV1::AlreadyClaimedV1 { .. }
        | StoreErrorV1::CancellationLostV1 { .. }
        | StoreErrorV1::ClaimStaleV1 { .. }
        | StoreErrorV1::IdempotencyDigestConflictV1 { .. }
        | StoreErrorV1::InternalInvariantViolationV1 { .. }
        | StoreErrorV1::InvalidStateV1(_)
        | StoreErrorV1::JobNotFoundV1 { .. }
        | StoreErrorV1::PreconditionConflictV1 { .. }
        | StoreErrorV1::SessionIdentityConflictV1 { .. }
        | StoreErrorV1::StorageUnavailableV1 { .. } => false,
    }
}

fn same_git_root_evidence(
    expected: &podway_git::ValidatedWorktreeV1,
    actual: &podway_git::ValidatedWorktreeV1,
) -> bool {
    expected.identity().common_directory_fingerprint()
        == actual.identity().common_directory_fingerprint()
        && expected.identity().worktree_administration_fingerprint()
            == actual.identity().worktree_administration_fingerprint()
        && expected.roots() == actual.roots()
        && expected.kind() == actual.kind()
        && expected.containment() == actual.containment()
}
fn marker_matches_registry_generation(
    marker: &ResetMarkerV1,
    registered_workspace_uuid: &WorkspaceId,
) -> bool {
    marker.target_workspace_uuid() != marker.previous_workspace_uuid()
        && (marker.previous_workspace_uuid() == registered_workspace_uuid
            || marker.target_workspace_uuid() == registered_workspace_uuid)
}
fn read_admitted_workspace_config(
    resolved: &ResolvedWorkspaceV1,
) -> Result<WorkspaceConfigV1, WorkspaceRuntimeErrorV1> {
    let path = workspace_config_path(resolved)?;
    #[cfg(unix)]
    {
        read_admitted_workspace_config_unix(&path)
    }
    #[cfg(not(unix))]
    {
        let mut config =
            fs::File::open(&path).map_err(|source| WorkspaceRuntimeErrorV1::ConfigRead {
                path: path.clone(),
                source,
            })?;
        let bytes = read_workspace_config_bytes(&mut config, 0).map_err(|source| {
            WorkspaceRuntimeErrorV1::ConfigRead {
                path: path.clone(),
                source,
            }
        })?;
        admit_workspace_config_bytes(bytes).map_err(WorkspaceRuntimeErrorV1::ConfigAdmission)
    }
}

#[cfg(unix)]
fn read_admitted_workspace_config_unix(
    path: &Path,
) -> Result<WorkspaceConfigV1, WorkspaceRuntimeErrorV1> {
    use nix::{
        fcntl::{OFlag, open, openat},
        sys::stat::Mode,
    };

    let podway_path = path
        .parent()
        .ok_or_else(|| WorkspaceRuntimeErrorV1::ConfigRead {
            path: path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "config path has no parent"),
        })?;
    let podway = open(
        podway_path,
        OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .map_err(|source| WorkspaceRuntimeErrorV1::ConfigRead {
        path: podway_path.to_path_buf(),
        source: io::Error::from_raw_os_error(source as i32),
    })?;
    let config = openat(
        &podway,
        "config.yaml",
        OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .map_err(|source| WorkspaceRuntimeErrorV1::ConfigRead {
        path: path.to_path_buf(),
        source: io::Error::from_raw_os_error(source as i32),
    })?;
    let mut config = std::fs::File::from(config);
    let metadata = config
        .metadata()
        .map_err(|source| WorkspaceRuntimeErrorV1::ConfigRead {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.file_type().is_file() {
        return Err(WorkspaceRuntimeErrorV1::ConfigRead {
            path: path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidData, "config is not a regular file"),
        });
    }
    if metadata.len() > podway_config::MAX_WORKSPACE_CONFIG_BYTES_V1 as u64 {
        return Err(WorkspaceRuntimeErrorV1::ConfigAdmission(
            ConfigError::InputTooLarge {
                maximum: podway_config::MAX_WORKSPACE_CONFIG_BYTES_V1,
                actual: metadata.len() as usize,
            },
        ));
    }
    let bytes =
        read_workspace_config_bytes(&mut config, metadata.len() as usize).map_err(|source| {
            WorkspaceRuntimeErrorV1::ConfigRead {
                path: path.to_path_buf(),
                source,
            }
        })?;
    admit_workspace_config_bytes(bytes).map_err(WorkspaceRuntimeErrorV1::ConfigAdmission)
}
fn read_workspace_config_bytes(
    reader: &mut impl Read,
    initial_capacity: usize,
) -> Result<Vec<u8>, io::Error> {
    let maximum = podway_config::MAX_WORKSPACE_CONFIG_BYTES_V1;
    let mut bytes = Vec::with_capacity(initial_capacity.min(maximum));
    reader
        .take(u64::try_from(maximum).expect("workspace config byte limit fits u64") + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn admit_workspace_config_bytes(bytes: Vec<u8>) -> Result<WorkspaceConfigV1, ConfigError> {
    let maximum = podway_config::MAX_WORKSPACE_CONFIG_BYTES_V1;
    if bytes.len() > maximum {
        return Err(ConfigError::InputTooLarge {
            maximum,
            actual: bytes.len(),
        });
    }
    parse_workspace_config_v1(bytes)
}

fn workspace_config_path(
    resolved: &ResolvedWorkspaceV1,
) -> Result<PathBuf, WorkspaceRuntimeErrorV1> {
    #[cfg(unix)]
    {
        Ok(resolved
            .workspace_root()
            .to_path_buf()
            .join(".podway")
            .join("config.yaml"))
    }
    #[cfg(not(unix))]
    {
        let _ = resolved;
        Err(WorkspaceRuntimeErrorV1::RuntimePathsUnsupportedPlatform)
    }
}

fn require_exact_binding(
    database_path: &Path,
    options: &SqliteStoreOptionsV1,
    expected: &WorkspaceBindingV1,
) -> Result<WorkspaceBindingV1, WorkspaceRuntimeErrorV1> {
    let actual = SqliteStoreV1::inspect_workspace_binding(database_path, options)
        .map_err(WorkspaceRuntimeErrorV1::Store)?
        .ok_or_else(|| WorkspaceRuntimeErrorV1::BindingDisappeared {
            database_path: database_path.to_path_buf(),
        })?;
    if &actual != expected {
        return Err(WorkspaceRuntimeErrorV1::BindingMismatch {
            expected: Box::new(expected.clone()),
            actual: Box::new(actual),
        });
    }
    Ok(expected.clone())
}

fn validate_pre_open_binding(
    resolved: &ResolvedWorkspaceV1,
    binding: &WorkspaceBindingV1,
) -> Result<(), WorkspaceRuntimeErrorV1> {
    if binding.identity() != resolved.store_identity() {
        return Err(WorkspaceRuntimeErrorV1::BindingIdentityMismatch {
            expected: Box::new(resolved.store_identity().clone()),
            actual: Box::new(binding.identity().clone()),
        });
    }

    let current_root = resolved.workspace_root().clone();
    let previous_root = resolved
        .move_metadata()
        .previous_root()
        .map(validated_root_from_lossless)
        .transpose()?;
    let expected_root = previous_root.as_ref().unwrap_or(&current_root);
    if binding.last_validated_root() != expected_root {
        return Err(WorkspaceRuntimeErrorV1::RebindEvidenceMismatch {
            binding_root: Box::new(binding.last_validated_root().clone()),
            git_previous_root: previous_root.map(Box::new),
            git_current_root: Box::new(current_root),
        });
    }
    Ok(())
}

fn refresh_registry_metadata(
    registry: &RegistryStoreV1,
    resolved: &ResolvedWorkspaceV1,
    binding_before_open: &WorkspaceBindingV1,
    workspace_uuid: WorkspaceId,
    seen_at: Rfc3339MillisV1,
) -> Result<(), WorkspaceRuntimeErrorV1> {
    let current_root = resolved.workspace_root().clone();
    if resolved.move_metadata().relocated_from_prior_root() {
        reconcile_registry_move(
            registry,
            workspace_uuid,
            binding_before_open.last_validated_root(),
            current_root,
            seen_at,
        )
    } else {
        match registry.insert_or_refresh(
            workspace_uuid.clone(),
            current_root.clone(),
            seen_at.clone(),
        ) {
            Ok(_) => Ok(()),
            Err(RegistryErrorV1::WorkspaceRootConflict {
                registered_root,
                supplied_root,
                ..
            }) if binding_before_open.last_validated_root() == &current_root
                && supplied_root == current_root =>
            {
                // A prior activation may have committed the Store root before publication of this
                // metadata update. Store and fresh Git evidence are authoritative; use the exact
                // stale registry root only as the compare-and-swap predecessor.
                reconcile_registry_move(
                    registry,
                    workspace_uuid,
                    &registered_root,
                    current_root,
                    seen_at,
                )
            }
            Err(source) => Err(WorkspaceRuntimeErrorV1::Registry(source)),
        }
    }
}

fn reconcile_registry_move(
    registry: &RegistryStoreV1,
    workspace_uuid: WorkspaceId,
    previous_root: &ValidatedWorkspaceRootV1,
    current_root: ValidatedWorkspaceRootV1,
    seen_at: Rfc3339MillisV1,
) -> Result<(), WorkspaceRuntimeErrorV1> {
    match registry.move_workspace(
        &workspace_uuid,
        previous_root,
        current_root.clone(),
        seen_at.clone(),
    ) {
        Ok(_) => Ok(()),
        Err(RegistryErrorV1::WorkspaceNotRegistered { .. }) => registry
            .insert_or_refresh(workspace_uuid, current_root, seen_at)
            .map(|_| ())
            .map_err(WorkspaceRuntimeErrorV1::Registry),
        Err(RegistryErrorV1::WorkspaceRootConflict {
            registered_root,
            supplied_root,
            ..
        }) if registered_root == current_root && supplied_root == *previous_root => registry
            .insert_or_refresh(workspace_uuid, current_root, seen_at)
            .map(|_| ())
            .map_err(WorkspaceRuntimeErrorV1::Registry),
        Err(source) => Err(WorkspaceRuntimeErrorV1::Registry(source)),
    }
}

fn runtime_directory_path(
    resolved: &ResolvedWorkspaceV1,
) -> Result<PathBuf, WorkspaceRuntimeErrorV1> {
    let runtime_directory_path = resolved
        .database_path()
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| WorkspaceRuntimeErrorV1::RuntimeDirectoryMissing {
            database_path: resolved.database_path().to_path_buf(),
        })?;
    let mut expected_database_path = runtime_directory_path.clone();
    expected_database_path.push("state.sqlite3");
    if expected_database_path != resolved.database_path() {
        return Err(WorkspaceRuntimeErrorV1::RuntimePathMismatch {
            database_path: resolved.database_path().to_path_buf(),
            runtime_directory_path,
        });
    }
    Ok(runtime_directory_path)
}

fn context_requires_rebind(
    current: &WorkspaceSchedulerContextV1,
    candidate: &WorkspaceSchedulerContextV1,
) -> bool {
    current.binding != candidate.binding
        || current.workspace_root != candidate.workspace_root
        || current.database != candidate.database
        || current.runtime_directory_path != candidate.runtime_directory_path
        || current.store_options != candidate.store_options
        || current.config != candidate.config
        || current.queue_limit != candidate.queue_limit
        || current.git_evidence != candidate.git_evidence
}

#[cfg(unix)]
fn database_file_identity(path: &Path) -> Option<DatabaseFileIdentityV1> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::metadata(path).ok()?;
    metadata.is_file().then_some(DatabaseFileIdentityV1 {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn database_file_identity(path: &Path) -> Option<DatabaseFileIdentityV1> {
    let metadata = fs::metadata(path).ok()?;
    metadata.is_file().then_some(DatabaseFileIdentityV1 {
        length: metadata.len(),
        modified: metadata.modified().ok(),
    })
}
#[cfg(unix)]
fn validated_root_from_lossless(
    path: &podway_git::LosslessPathV1,
) -> Result<ValidatedWorkspaceRootV1, WorkspaceRuntimeErrorV1> {
    let bytes = path
        .decode_path_bytes()
        .map_err(|source| WorkspaceResolutionErrorV1::StoredRootPathInvalid { source })
        .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
    ValidatedWorkspaceRootV1::from_unix_bytes(bytes).map_err(WorkspaceRuntimeErrorV1::StoreOptions)
}

#[cfg(not(unix))]
fn validated_root_from_lossless(
    _path: &podway_git::LosslessPathV1,
) -> Result<ValidatedWorkspaceRootV1, WorkspaceRuntimeErrorV1> {
    Err(WorkspaceRuntimeErrorV1::RuntimePathsUnsupportedPlatform)
}
fn mutex_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .expect("workspace scheduler coordination lock must not be poisoned")
}
#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{self, Read},
        sync::{
            Arc, Barrier, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        thread,
    };

    use podway_config::{ConfigError, MAX_WORKSPACE_CONFIG_BYTES_V1};
    use podway_core::{UnixMillis, WorkspaceId};
    use podway_store::{
        CanonicalRequestDigestV1, DurableWorktreeIdentityV1, SqliteStoreOptionsV1, SqliteStoreV1,
        StoreErrorV1, StoreUnavailableReasonV1, ValidatedWorkspaceRootV1,
    };

    use super::{
        WorkspaceMaintenanceCoordinatorV1, WorkspaceMaintenanceKeyV1, WorkspaceSchedulerKeyV1,
        WorkspaceSchedulerRetirementBindingV1, WorkspaceStoreSlotTokenV1, WorkspaceStoreSlotV1,
        WorkspaceStoreStateV1, admit_workspace_config_bytes, read_workspace_config_bytes,
        reset_seed_requires_fixed_replacement,
    };
    static CONFIG_TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    #[cfg(unix)]
    #[test]
    fn real_store_slot_close_failpoint_executes_once_and_caches_the_exact_failure() {
        use std::os::unix::ffi::OsStrExt;

        let sequence = CONFIG_TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!("podway-store-slot-close-{}", sequence));
        fs::create_dir_all(&directory).expect("fixture Store directory must exist");
        let root =
            ValidatedWorkspaceRootV1::from_unix_bytes(directory.as_os_str().as_bytes().to_vec())
                .expect("fixture root must be valid");
        let identity = DurableWorktreeIdentityV1::new(
            CanonicalRequestDigestV1::new(format!("sha256:{}", "a".repeat(64)))
                .expect("fixture common fingerprint must be valid"),
            WorkspaceId::new("00000000-0000-4000-8000-000000005101")
                .expect("fixture workspace UUID must be valid"),
            CanonicalRequestDigestV1::new(format!("sha256:{}", "b".repeat(64)))
                .expect("fixture worktree fingerprint must be valid"),
        );
        let store = SqliteStoreV1::open(
            directory.join("state.sqlite3"),
            &root,
            identity,
            SqliteStoreOptionsV1::new(1).expect("fixture Store options must be valid"),
            UnixMillis::new(1_700_000_000_123),
        )
        .expect("fixture Store must open");
        let expected = StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Busy,
        };
        let close_count = Arc::new(AtomicU64::new(0));
        let slot = WorkspaceStoreSlotV1 {
            startup_recovery_report: store.startup_recovery_report().clone(),
            state: Mutex::new(WorkspaceStoreStateV1::Open(Box::new(store))),
            token: WorkspaceStoreSlotTokenV1::issue(),
            close_failpoint: Some(super::WorkspaceStoreCloseFailpointV1 {
                error: expected.clone(),
                close_count: Arc::clone(&close_count),
            }),
        };

        assert_eq!(slot.close_for_maintenance(), Err(expected.clone()));
        assert_eq!(slot.close_for_maintenance(), Err(expected));
        assert_eq!(
            close_count.load(Ordering::SeqCst),
            1,
            "the real Store close executes once before its exact failpoint result is cached"
        );
        drop(slot);
        fs::remove_dir_all(directory).expect("fixture Store directory must be removed");
    }
    #[test]
    fn same_key_admission_preparation_and_activation_are_mutually_exclusive() {
        let coordinator = Arc::new(WorkspaceMaintenanceCoordinatorV1::default());
        let key = WorkspaceMaintenanceKeyV1 {
            common_directory_fingerprint: CanonicalRequestDigestV1::new(format!(
                "sha256:{}",
                "a".repeat(64)
            ))
            .expect("fixture common-directory fingerprint must be valid"),
            worktree_administration_fingerprint: CanonicalRequestDigestV1::new(format!(
                "sha256:{}",
                "b".repeat(64)
            ))
            .expect("fixture worktree-administration fingerprint must be valid"),
        };

        let maintenance = coordinator
            .acquire(key.clone())
            .expect("reset maintenance must acquire exclusivity");
        assert!(
            coordinator.acquire_activation(key.clone()).is_none(),
            "activation must not pass while reset maintenance owns the lifecycle"
        );
        drop(maintenance);

        let activation = coordinator
            .acquire_activation(key.clone())
            .expect("activation must hold a shared lifecycle lease");
        assert!(
            coordinator.acquire(key.clone()).is_none(),
            "reset maintenance must not pass while activation publishes its scheduler"
        );
        drop(activation);
        let claim = coordinator
            .acquire_claim(key.clone())
            .expect("ordinary admission must register a same-key claim");
        assert!(
            coordinator.acquire(key.clone()).is_none(),
            "reset preparation must not begin while same-key admission owns a claim"
        );
        drop(claim);

        let maintenance = coordinator
            .acquire(key.clone())
            .expect("reset maintenance must acquire after the admission releases");
        assert!(
            coordinator.acquire_claim(key).is_none(),
            "same-key admission must not begin after reset preparation acquires maintenance"
        );
        drop(maintenance);
    }

    #[test]
    fn concurrent_claims_and_activations_cannot_cross_maintenance_boundaries() {
        let coordinator = Arc::new(WorkspaceMaintenanceCoordinatorV1::default());
        let key = WorkspaceMaintenanceKeyV1 {
            common_directory_fingerprint: CanonicalRequestDigestV1::new(format!(
                "sha256:{}",
                "c".repeat(64)
            ))
            .expect("fixture common-directory fingerprint must be valid"),
            worktree_administration_fingerprint: CanonicalRequestDigestV1::new(format!(
                "sha256:{}",
                "d".repeat(64)
            ))
            .expect("fixture worktree-administration fingerprint must be valid"),
        };

        let maintenance = coordinator
            .acquire(key.clone())
            .expect("maintenance must acquire before concurrent contenders");
        let start = Arc::new(Barrier::new(3));
        let claim_attempt = {
            let coordinator = Arc::clone(&coordinator);
            let key = key.clone();
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                coordinator.acquire_claim(key).is_none()
            })
        };
        let activation_attempt = {
            let coordinator = Arc::clone(&coordinator);
            let key = key.clone();
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                coordinator.acquire_activation(key).is_none()
            })
        };
        start.wait();
        assert!(claim_attempt.join().expect("claim contender must join"));
        assert!(
            activation_attempt
                .join()
                .expect("activation contender must join")
        );
        drop(maintenance);

        let claim_ready = Arc::new(Barrier::new(2));
        let claim_release = Arc::new(Barrier::new(2));
        let claim_holder = {
            let coordinator = Arc::clone(&coordinator);
            let key = key.clone();
            let claim_ready = Arc::clone(&claim_ready);
            let claim_release = Arc::clone(&claim_release);
            thread::spawn(move || {
                let claim = coordinator
                    .acquire_claim(key)
                    .expect("claim must acquire before maintenance");
                claim_ready.wait();
                claim_release.wait();
                drop(claim);
            })
        };
        claim_ready.wait();
        assert!(
            coordinator.acquire(key.clone()).is_none(),
            "maintenance cannot cross a live claim boundary"
        );
        claim_release.wait();
        claim_holder.join().expect("claim holder must join");

        let activation_ready = Arc::new(Barrier::new(2));
        let activation_release = Arc::new(Barrier::new(2));
        let activation_holder = {
            let coordinator = Arc::clone(&coordinator);
            let key = key.clone();
            let activation_ready = Arc::clone(&activation_ready);
            let activation_release = Arc::clone(&activation_release);
            thread::spawn(move || {
                let activation = coordinator
                    .acquire_activation(key)
                    .expect("activation must acquire before maintenance");
                activation_ready.wait();
                activation_release.wait();
                drop(activation);
            })
        };
        activation_ready.wait();
        assert!(
            coordinator.acquire(key.clone()).is_none(),
            "maintenance cannot cross a live activation boundary"
        );
        activation_release.wait();
        activation_holder
            .join()
            .expect("activation holder must join");
        assert!(coordinator.acquire(key).is_some());
    }
    #[test]
    fn retirement_receipts_are_bound_to_each_closed_store_slot() {
        let coordinator = WorkspaceMaintenanceCoordinatorV1::default();
        let common = CanonicalRequestDigestV1::new(format!("sha256:{}", "a".repeat(64)))
            .expect("fixture common fingerprint must be valid");
        let worktree = CanonicalRequestDigestV1::new(format!("sha256:{}", "b".repeat(64)))
            .expect("fixture worktree fingerprint must be valid");
        let source = DurableWorktreeIdentityV1::new(
            common,
            WorkspaceId::new("00000000-0000-4000-8000-000000005901")
                .expect("fixture workspace UUID must be valid"),
            worktree,
        );
        let maintenance_key = WorkspaceMaintenanceKeyV1::from_durable_identity(&source);
        let key = WorkspaceSchedulerKeyV1::from_durable_identity(&source);
        let old = WorkspaceSchedulerRetirementBindingV1 {
            key: key.clone(),
            generation: crate::SchedulerGenerationV1::initial(),
            source: source.clone(),
            store_slot: WorkspaceStoreSlotTokenV1(5_901),
        };
        let current = WorkspaceSchedulerRetirementBindingV1 {
            key,
            generation: crate::SchedulerGenerationV1::initial(),
            source: source.clone(),
            store_slot: WorkspaceStoreSlotTokenV1(5_902),
        };
        coordinator.register_scheduler_generation(old.clone());
        coordinator.register_scheduler_generation(current.clone());
        coordinator.record_claims_stopped(&old);
        coordinator.record_work_drained(&old);
        coordinator.record_store_close_result(
            &old,
            &Err(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Busy,
            }),
        );
        assert!(
            !coordinator.has_retirement_receipt(&maintenance_key, source.workspace_uuid()),
            "a cached close failure must not earn retirement credit"
        );
        coordinator.record_store_closed(&old);

        assert!(
            coordinator.has_retirement_receipt(&maintenance_key, source.workspace_uuid()),
            "the old slot earns a receipt only after stop, drain, and close"
        );
        assert!(
            coordinator.has_unclosed_generation(&maintenance_key),
            "closing an old slot must not credit the distinct current slot"
        );

        coordinator.record_store_closed(&current);
        assert!(
            coordinator.has_unclosed_generation(&maintenance_key),
            "close cannot be credited before stop and drain"
        );
        coordinator.record_claims_stopped(&current);
        coordinator.record_work_drained(&current);
        coordinator.record_store_closed(&current);
        assert!(!coordinator.has_unclosed_generation(&maintenance_key));
    }
    #[test]
    fn combined_reset_seed_cleanup_failure_is_not_replaceable() {
        let error = StoreErrorV1::PrimaryOperationAndCleanupFailureV1 {
            primary: Box::new(StoreErrorV1::NewerStateV1 {
                found_schema_version: 2,
                supported_schema_version: 1,
            }),
            cleanup: Box::new(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Busy,
            }),
        };
        assert!(
            !reset_seed_requires_fixed_replacement(&error),
            "a failed cleanup must remain fatal even when the primary error is replaceable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn config_fifo_is_rejected_without_waiting_for_a_writer() {
        use nix::{sys::stat::Mode, unistd::mkfifo};

        let sequence = CONFIG_TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "podway-config-fifo-{}-{sequence}",
            std::process::id()
        ));
        let podway = directory.join(".podway");
        let path = podway.join("config.yaml");
        fs::create_dir_all(&podway).expect("FIFO fixture parent must exist");
        mkfifo(&path, Mode::S_IRUSR | Mode::S_IWUSR).expect("config FIFO must be created");

        let result = super::read_admitted_workspace_config_unix(&path);
        assert!(matches!(
            result,
            Err(super::WorkspaceRuntimeErrorV1::ConfigRead { source, .. })
                if source.kind() == io::ErrorKind::InvalidData
        ));

        fs::remove_file(path).expect("FIFO fixture must be removed");
        fs::remove_dir_all(directory).expect("FIFO fixture directory must be removed");
    }

    struct GrowingReaderV1 {
        bytes: Vec<u8>,
        position: usize,
    }

    impl GrowingReaderV1 {
        fn new(bytes: Vec<u8>) -> Self {
            Self { bytes, position: 0 }
        }
    }

    impl Read for GrowingReaderV1 {
        fn read(&mut self, buffer: &mut [u8]) -> Result<usize, io::Error> {
            let remaining = &self.bytes[self.position..];
            let copied = remaining.len().min(buffer.len());
            buffer[..copied].copy_from_slice(&remaining[..copied]);
            self.position += copied;
            Ok(copied)
        }
    }

    #[test]
    fn config_read_rejects_post_metadata_growth_without_reading_past_the_limit() {
        let mut reader = GrowingReaderV1::new(vec![b' '; MAX_WORKSPACE_CONFIG_BYTES_V1 + 2]);
        let bytes = read_workspace_config_bytes(&mut reader, 1)
            .expect("the bounded reader must read the permitted prefix");

        assert_eq!(bytes.len(), MAX_WORKSPACE_CONFIG_BYTES_V1 + 1);
        assert_eq!(reader.position, MAX_WORKSPACE_CONFIG_BYTES_V1 + 1);
        assert_eq!(
            admit_workspace_config_bytes(bytes),
            Err(ConfigError::InputTooLarge {
                maximum: MAX_WORKSPACE_CONFIG_BYTES_V1,
                actual: MAX_WORKSPACE_CONFIG_BYTES_V1 + 1,
            })
        );
    }
}
