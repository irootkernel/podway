//! Daemon-owned composition of validated workspaces, SQLite state, metadata, and schedulers.
//!
//! The Store remains the durable authority. This module only admits a workspace after Git and the
//! existing Store binding agree, then keeps registry data as non-authoritative metadata.

use std::{
    error::Error,
    fmt, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard},
};

use podway_config::{
    ConfigError, DEFAULT_WORKSPACE_CONFIG_YAML_V1, WorkspaceConfigV1, parse_workspace_config_v1,
};
use podway_core::{UnixMillis, WorkspaceId};
use podway_git::{
    GitResolveErrorV1, GitResolverContractV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorkspaceLayoutErrorV1, WorkspaceLayoutInitializerV1, WorktreeSelectorV1,
};
use podway_protocol::Rfc3339MillisV1;
use podway_service::ServiceRuntimePathsV1;
use podway_store::{
    SqliteStoreOptionsV1, SqliteStoreV1, StoreErrorV1, StoreValueErrorV1, ValidatedWorkspaceRootV1,
    WorkspaceBindingV1,
};

use crate::{
    DaemonCompositionErrorV1,
    registry::{RegistryErrorV1, RegistryStoreV1},
    scheduler::{WorkspaceSchedulerKeyV1, WorkspaceSchedulerRegistryV1, WorkspaceSchedulerV1},
    workspace::{
        ResolvedWorkspaceV1, SqliteWorkspaceBindingInspectorV1, WorkspaceGitObservationV1,
        WorkspaceResolutionErrorV1, WorkspaceResolverV1,
    },
};

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
    store: Arc<SqliteStoreV1>,
    store_options: SqliteStoreOptionsV1,
    config: WorkspaceConfigV1,
    queue_limit: u16,
    git_evidence: podway_git::ValidatedWorktreeV1,
    coordination: Arc<WorkspaceSchedulerCoordinationV1>,
}

#[derive(Debug)]
struct WorkspaceSchedulerCoordinationV1 {
    claim_gate: Mutex<()>,
    state: Mutex<WorkspaceSchedulerCoordinationStateV1>,
    changed: Condvar,
}

#[derive(Debug)]
struct WorkspaceSchedulerCoordinationStateV1 {
    accepting_claims: bool,
    recovery_required: bool,
    notification_version: u64,
}

impl WorkspaceSchedulerContextV1 {
    fn new(
        binding: WorkspaceBindingV1,
        database: ValidatedDatabaseFileV1,
        runtime_directory_path: PathBuf,
        store: Arc<SqliteStoreV1>,
        store_options: SqliteStoreOptionsV1,
        config: WorkspaceConfigV1,
        git_evidence: podway_git::ValidatedWorktreeV1,
    ) -> Self {
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
            }),
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

    pub fn store(&self) -> &Arc<SqliteStoreV1> {
        &self.store
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
        Some(operation(&self.binding))
    }

    pub(crate) fn stop_claims(&self) {
        let _claim_gate = mutex_lock(&self.coordination.claim_gate);
        mutex_lock(&self.coordination.state).accepting_claims = false;
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

    fn with_stable_coordination_from(mut self, current: &Self) -> Self {
        self.coordination = Arc::clone(&current.coordination);
        self
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
    RuntimePathsUnsupportedPlatform,
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
            Self::RuntimePathsUnsupportedPlatform => {
                formatter.write_str("workspace runtime paths require Unix native path support")
            }
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
            | Self::RevalidationKeyMismatch { .. } => None,
        }
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
}

impl WorkspaceRuntimeManagerV1 {
    pub fn new(paths: &ServiceRuntimePathsV1, inspection_options: SqliteStoreOptionsV1) -> Self {
        Self {
            resolver: WorkspaceResolverV1::new(
                NativeGitResolverV1::new(),
                SqliteWorkspaceBindingInspectorV1::new(inspection_options.clone()),
            ),
            layout_initializer: WorkspaceLayoutInitializerV1::new(),
            inspection_options,
            registry: RegistryStoreV1::new(paths),
            schedulers: WorkspaceSchedulerRegistryV1::new(),
        }
    }

    pub fn resolver(
        &self,
    ) -> &WorkspaceResolverV1<NativeGitResolverV1, SqliteWorkspaceBindingInspectorV1> {
        &self.resolver
    }

    pub const fn layout_initializer(&self) -> WorkspaceLayoutInitializerV1 {
        self.layout_initializer
    }

    pub fn registry(&self) -> &RegistryStoreV1 {
        &self.registry
    }

    pub fn schedulers(&self) -> &WorkspaceSchedulerRegistryV1<WorkspaceSchedulerContextV1> {
        &self.schedulers
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
        let candidate = self
            .resolver
            .resolve_bootstrap(selector.clone())
            .map_err(WorkspaceRuntimeErrorV1::Resolution)?;
        self.layout_initializer
            .initialize_with_config(candidate.worktree(), DEFAULT_WORKSPACE_CONFIG_YAML_V1)
            .map_err(WorkspaceRuntimeErrorV1::Layout)?;

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
        self.activate_existing_resolved(bound, observation)
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

    fn activate_existing_resolved(
        &self,
        resolved: ResolvedWorkspaceV1,
        observation: WorkspaceRuntimeObservationV1,
    ) -> Result<Arc<WorkspaceSchedulerV1<WorkspaceSchedulerContextV1>>, WorkspaceRuntimeErrorV1>
    {
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
                return Ok(scheduler);
            }
        }

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
        let store = Arc::new(
            SqliteStoreV1::open(
                resolved.database_path(),
                resolved.workspace_root(),
                resolved.store_identity().clone(),
                options.clone(),
                observation.store_now(),
            )
            .map_err(WorkspaceRuntimeErrorV1::Store)?,
        );
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

        let context = WorkspaceSchedulerContextV1::new(
            binding,
            database,
            runtime_directory_path(&resolved)?,
            store,
            options,
            config,
            resolved.worktree().clone(),
        );
        let scheduler = self
            .schedulers
            .get_or_create(key, {
                let context = context.clone();
                move || context
            })
            .map_err(WorkspaceRuntimeErrorV1::Scheduler)?;
        let current = scheduler.context_snapshot();
        if context_requires_rebind(&current, &context) {
            let replacement = Arc::new(context.with_stable_coordination_from(&current));
            drop(scheduler.rebind_context_serialized(replacement));
        }
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

fn same_git_root_evidence(
    expected: &podway_git::ValidatedWorktreeV1,
    actual: &podway_git::ValidatedWorktreeV1,
) -> bool {
    expected.identity() == actual.identity()
        && expected.roots() == actual.roots()
        && expected.kind() == actual.kind()
        && expected.containment() == actual.containment()
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
        sync::atomic::{AtomicU64, Ordering},
    };

    use podway_config::{ConfigError, MAX_WORKSPACE_CONFIG_BYTES_V1};

    use super::{admit_workspace_config_bytes, read_workspace_config_bytes};
    static CONFIG_TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
