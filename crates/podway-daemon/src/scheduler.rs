//! Durable-identity scheduler lifecycle primitives.
//!
//! This module intentionally owns only ephemeral coordination. Durable FIFO ordering and command
//! state remain authoritative in `podway_store`; progress notifications are wake-up hints only.

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{Arc, Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use podway_store::{DurableWorktreeIdentityV1, GitIdentityV1, WorkspaceUuidV1};

use crate::{
    DaemonCompositionErrorV1, SchedulerGenerationV1,
    observability::{EventCategoryV1, ObservabilityV1, SeverityV1},
};

/// The durable identity fields used to select one workspace scheduler.
///
/// This deliberately contains no root, path, display string, or path-derived alias. Callers must
/// construct it from the Store-validated durable identity for every request.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceSchedulerKeyV1 {
    workspace_uuid: WorkspaceUuidV1,
    common_directory_digest: GitIdentityV1,
    worktree_administration_digest: GitIdentityV1,
}

impl WorkspaceSchedulerKeyV1 {
    /// Copies only durable scheduler identity fields from the Store boundary.
    pub fn from_durable_identity(identity: &DurableWorktreeIdentityV1) -> Self {
        Self {
            workspace_uuid: identity.workspace_uuid().clone(),
            common_directory_digest: identity.common_dir_identity().clone(),
            worktree_administration_digest: identity.worktree_admin_identity().clone(),
        }
    }

    pub fn workspace_uuid(&self) -> &WorkspaceUuidV1 {
        &self.workspace_uuid
    }

    pub fn common_directory_digest(&self) -> &GitIdentityV1 {
        &self.common_directory_digest
    }

    pub fn worktree_administration_digest(&self) -> &GitIdentityV1 {
        &self.worktree_administration_digest
    }
}

impl From<&DurableWorktreeIdentityV1> for WorkspaceSchedulerKeyV1 {
    fn from(identity: &DurableWorktreeIdentityV1) -> Self {
        Self::from_durable_identity(identity)
    }
}

/// A monotonically increasing wake-up version for scheduler progress.
///
/// Version changes do not carry state. A waiter must query its authoritative state (normally the
/// Store) after every wake-up before deciding that work is complete.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspaceSchedulerProgressVersionV1(u64);

impl WorkspaceSchedulerProgressVersionV1 {
    pub const INITIAL: Self = Self(0);

    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Result<Self, WorkspaceSchedulerProgressErrorV1> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(WorkspaceSchedulerProgressErrorV1::VersionExhausted)
    }
}

/// A progress version could not advance without wrapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceSchedulerProgressErrorV1 {
    VersionExhausted,
}

impl fmt::Display for WorkspaceSchedulerProgressErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionExhausted => formatter.write_str("scheduler progress version exhausted"),
        }
    }
}

impl Error for WorkspaceSchedulerProgressErrorV1 {}

struct ProgressStateV1 {
    version: WorkspaceSchedulerProgressVersionV1,
}

/// One ephemeral scheduler generation for a durable worktree identity.
///
/// The serialization mutex is deliberately local to this identity. It coordinates in-process
/// operations only; callers still use Store transactions for FIFO and durable state decisions.
pub struct WorkspaceSchedulerV1<C> {
    key: WorkspaceSchedulerKeyV1,
    generation: SchedulerGenerationV1,
    context: RwLock<Arc<C>>,
    serialization: Mutex<()>,
    progress: Mutex<ProgressStateV1>,
    progress_changed: Condvar,
}

impl<C> WorkspaceSchedulerV1<C> {
    pub fn new(
        key: WorkspaceSchedulerKeyV1,
        generation: SchedulerGenerationV1,
        context: C,
    ) -> Self {
        Self::with_context(key, generation, Arc::new(context))
    }

    pub fn with_context(
        key: WorkspaceSchedulerKeyV1,
        generation: SchedulerGenerationV1,
        context: Arc<C>,
    ) -> Self {
        Self {
            key,
            generation,
            context: RwLock::new(context),
            serialization: Mutex::new(()),
            progress: Mutex::new(ProgressStateV1 {
                version: WorkspaceSchedulerProgressVersionV1::INITIAL,
            }),
            progress_changed: Condvar::new(),
        }
    }

    pub fn key(&self) -> &WorkspaceSchedulerKeyV1 {
        &self.key
    }

    pub const fn generation(&self) -> SchedulerGenerationV1 {
        self.generation
    }

    /// Returns a stable context reference for one caller without holding the context lock.
    pub fn context_snapshot(&self) -> Arc<C> {
        Arc::clone(&read_lock(&self.context))
    }

    /// Retires the current context while serialization is held, then publishes its replacement.
    ///
    /// The replacement is not visible when retirement fails.
    pub(crate) fn rebind_context_serialized_after<E>(
        &self,
        context: Arc<C>,
        retire: impl FnOnce(&Arc<C>) -> Result<(), E>,
    ) -> Result<Arc<C>, E> {
        let _serialization = mutex_lock(&self.serialization);
        let mut current = write_lock(&self.context);
        retire(&current)?;
        Ok(std::mem::replace(&mut *current, context))
    }

    /// Runs one operation under this identity's in-process serialization mutex.
    pub fn with_serialized<R>(&self, operation: impl FnOnce(Arc<C>) -> R) -> R {
        let _serialization = mutex_lock(&self.serialization);
        operation(self.context_snapshot())
    }

    /// Runs an operation only when this identity's serialization mutex is immediately available.
    pub fn try_with_serialized<R>(&self, operation: impl FnOnce(Arc<C>) -> R) -> Option<R> {
        let _serialization = match self.serialization.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => return None,
            Err(std::sync::TryLockError::Poisoned(_)) => {
                panic!("workspace scheduler serialization lock must not be poisoned")
            }
        };
        Some(operation(self.context_snapshot()))
    }

    /// Returns the current progress-watch version.
    pub fn progress_version(&self) -> WorkspaceSchedulerProgressVersionV1 {
        mutex_lock(&self.progress).version
    }

    /// Advances the progress-watch version and wakes all waiters.
    ///
    /// The wake-up is only a hint: consumers must re-read their authoritative predicate after it.
    pub fn notify_progress(
        &self,
    ) -> Result<WorkspaceSchedulerProgressVersionV1, WorkspaceSchedulerProgressErrorV1> {
        let mut progress = mutex_lock(&self.progress);
        progress.version = progress.version.next()?;
        let version = progress.version;
        drop(progress);
        self.progress_changed.notify_all();
        Ok(version)
    }

    /// Waits until a progress-watch version differs from `observed`.
    ///
    /// This method does not establish completion. Its caller must treat the returned version as a
    /// hint and query the Store or another authoritative state source.
    pub fn wait_for_progress_after(
        &self,
        observed: WorkspaceSchedulerProgressVersionV1,
    ) -> WorkspaceSchedulerProgressVersionV1 {
        let mut progress = mutex_lock(&self.progress);
        while progress.version == observed {
            progress = condvar_wait(&self.progress_changed, progress);
        }
        progress.version
    }

    /// Waits while `should_keep_waiting` remains true, rechecking it after every wake-up.
    ///
    /// The predicate runs without the progress lock so it may query the Store or invoke other
    /// scheduler APIs. Notifications never decide the predicate; they only cause another check.
    pub fn wait_for_progress_while<P>(
        &self,
        observed: WorkspaceSchedulerProgressVersionV1,
        mut should_keep_waiting: P,
    ) -> WorkspaceSchedulerProgressVersionV1
    where
        P: FnMut() -> bool,
    {
        let mut observed = observed;
        loop {
            if !should_keep_waiting() {
                return self.progress_version();
            }

            let progress = mutex_lock(&self.progress);
            if progress.version != observed {
                observed = progress.version;
                drop(progress);
                continue;
            }

            let progress = condvar_wait(&self.progress_changed, progress);
            observed = progress.version;
            drop(progress);
        }
    }
}

/// Registry failures caused before a retirement close/drain callback can start.
pub enum WorkspaceSchedulerRetirementStartErrorV1<C> {
    NotRegistered {
        key: WorkspaceSchedulerKeyV1,
        generation: SchedulerGenerationV1,
    },
    NotCurrent {
        key: WorkspaceSchedulerKeyV1,
        generation: SchedulerGenerationV1,
    },
    CloseInProgress {
        key: WorkspaceSchedulerKeyV1,
        generation: SchedulerGenerationV1,
    },
    AlreadyRetiring {
        retry: WorkspaceSchedulerRetirementRetryV1<C>,
    },
}

impl<C> fmt::Debug for WorkspaceSchedulerRetirementStartErrorV1<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRegistered { key, generation } => formatter
                .debug_struct("NotRegistered")
                .field("key", key)
                .field("generation", generation)
                .finish(),
            Self::NotCurrent { key, generation } => formatter
                .debug_struct("NotCurrent")
                .field("key", key)
                .field("generation", generation)
                .finish(),
            Self::CloseInProgress { key, generation } => formatter
                .debug_struct("CloseInProgress")
                .field("key", key)
                .field("generation", generation)
                .finish(),
            Self::AlreadyRetiring { retry } => formatter
                .debug_tuple("AlreadyRetiring")
                .field(retry)
                .finish(),
        }
    }
}

impl<C> fmt::Display for WorkspaceSchedulerRetirementStartErrorV1<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRegistered { generation, .. } => {
                write!(
                    formatter,
                    "scheduler generation {} is not registered",
                    generation.get()
                )
            }
            Self::NotCurrent { generation, .. } => {
                write!(
                    formatter,
                    "scheduler generation {} is not current",
                    generation.get()
                )
            }
            Self::CloseInProgress { generation, .. } => write!(
                formatter,
                "scheduler generation {} close/drain is already in progress",
                generation.get()
            ),
            Self::AlreadyRetiring { retry } => write!(
                formatter,
                "scheduler generation {} remains retiring after a failed close/drain",
                retry.generation().get()
            ),
        }
    }
}

impl<C> Error for WorkspaceSchedulerRetirementStartErrorV1<C> {}

/// A close/drain callback or its compare-and-remove completion failed.
pub enum WorkspaceSchedulerRetirementErrorV1<C, E> {
    Start(WorkspaceSchedulerRetirementStartErrorV1<C>),
    CloseFailed {
        source: E,
        retry: WorkspaceSchedulerRetirementRetryV1<C>,
    },
    CloseFailedStale {
        source: E,
        key: WorkspaceSchedulerKeyV1,
        generation: SchedulerGenerationV1,
    },
    StaleCompletion {
        key: WorkspaceSchedulerKeyV1,
        generation: SchedulerGenerationV1,
    },
}

impl<C, E: fmt::Debug> fmt::Debug for WorkspaceSchedulerRetirementErrorV1<C, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start(error) => formatter.debug_tuple("Start").field(error).finish(),
            Self::CloseFailed { source, retry } => formatter
                .debug_struct("CloseFailed")
                .field("source", source)
                .field("retry", retry)
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
            Self::StaleCompletion { key, generation } => formatter
                .debug_struct("StaleCompletion")
                .field("key", key)
                .field("generation", generation)
                .finish(),
        }
    }
}

impl<C, E: fmt::Display> fmt::Display for WorkspaceSchedulerRetirementErrorV1<C, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start(error) => error.fmt(formatter),
            Self::CloseFailed { source, retry } => write!(
                formatter,
                "scheduler generation {} close/drain failed: {source}",
                retry.generation().get()
            ),
            Self::CloseFailedStale {
                source, generation, ..
            } => write!(
                formatter,
                "scheduler generation {} close/drain failed after becoming stale: {source}",
                generation.get()
            ),
            Self::StaleCompletion { generation, .. } => write!(
                formatter,
                "scheduler generation {} completion became stale",
                generation.get()
            ),
        }
    }
}

impl<C: 'static, E> Error for WorkspaceSchedulerRetirementErrorV1<C, E>
where
    E: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Start(error) => Some(error),
            Self::CloseFailed { source, .. } | Self::CloseFailedStale { source, .. } => {
                Some(source)
            }
            Self::StaleCompletion { .. } => None,
        }
    }
}

/// A typed capability to retry a failed close/drain without reopening its scheduler slot.
pub struct WorkspaceSchedulerRetirementRetryV1<C> {
    inner: Arc<WorkspaceSchedulerRegistryInnerV1<C>>,
    scheduler: Arc<WorkspaceSchedulerV1<C>>,
}
impl<C> Clone for WorkspaceSchedulerRetirementRetryV1<C> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            scheduler: Arc::clone(&self.scheduler),
        }
    }
}

impl<C> WorkspaceSchedulerRetirementRetryV1<C> {
    pub fn key(&self) -> &WorkspaceSchedulerKeyV1 {
        self.scheduler.key()
    }

    pub fn generation(&self) -> SchedulerGenerationV1 {
        self.scheduler.generation()
    }

    /// Re-runs close/drain only when this exact scheduler remains retired-but-not-closing.
    pub fn retry<F, E>(&self, close: F) -> Result<(), WorkspaceSchedulerRetirementErrorV1<C, E>>
    where
        F: FnOnce(&WorkspaceSchedulerV1<C>) -> Result<(), E>,
    {
        begin_retirement_retry(self).map_err(WorkspaceSchedulerRetirementErrorV1::Start)?;
        finish_retirement(self, close)
    }
}

impl<C> fmt::Debug for WorkspaceSchedulerRetirementRetryV1<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceSchedulerRetirementRetryV1")
            .field("key", self.scheduler.key())
            .field("generation", &self.scheduler.generation())
            .finish()
    }
}

/// An identity-keyed registry of active and fail-closed retiring scheduler generations.
pub struct WorkspaceSchedulerRegistryV1<C> {
    inner: Arc<WorkspaceSchedulerRegistryInnerV1<C>>,
}

impl<C> WorkspaceSchedulerRegistryV1<C> {
    pub fn new() -> Self {
        Self::with_observability(None)
    }

    /// Adds an optional non-authoritative categorical event producer.
    pub fn with_observability(observability: Option<Arc<Mutex<ObservabilityV1>>>) -> Self {
        Self {
            inner: Arc::new(WorkspaceSchedulerRegistryInnerV1 {
                state: Mutex::new(WorkspaceSchedulerRegistryStateV1 {
                    slots: HashMap::new(),
                    generations: HashMap::new(),
                    creating: HashSet::new(),
                }),
                changed: Condvar::new(),
                observability,
            }),
        }
    }

    /// Returns the current active generation without reserving or creating a scheduler.
    ///
    /// A creating, absent, or retiring slot is intentionally reported as unavailable. Callers that
    /// need creation or retirement waiting must use [`Self::get_or_create`].
    pub fn get_active(
        &self,
        key: &WorkspaceSchedulerKeyV1,
    ) -> Option<Arc<WorkspaceSchedulerV1<C>>> {
        let state = mutex_lock(&self.inner.state);
        match state.slots.get(key) {
            Some(WorkspaceSchedulerRegistrySlotV1::Active(scheduler)) => {
                Some(Arc::clone(scheduler))
            }
            Some(WorkspaceSchedulerRegistrySlotV1::Retiring { .. }) | None => None,
        }
    }

    /// Atomically returns the active scheduler or reserves exactly one generation for `factory`.
    ///
    /// A factory runs without the registry lock. Calls that encounter a creating or actively closing
    /// slot wait for a state change. A failed retirement returns a typed unavailable error and never
    /// permits recreation.
    pub fn get_or_create<F>(
        &self,
        key: WorkspaceSchedulerKeyV1,
        factory: F,
    ) -> Result<Arc<WorkspaceSchedulerV1<C>>, DaemonCompositionErrorV1>
    where
        F: FnOnce() -> C,
    {
        let mut factory = Some(factory);
        loop {
            let mut state = mutex_lock(&self.inner.state);
            match state.slots.get(&key) {
                Some(WorkspaceSchedulerRegistrySlotV1::Active(scheduler)) => {
                    return Ok(Arc::clone(scheduler));
                }
                Some(WorkspaceSchedulerRegistrySlotV1::Retiring {
                    close_in_progress: true,
                    ..
                }) => {
                    state = condvar_wait(&self.inner.changed, state);
                    drop(state);
                    continue;
                }
                Some(WorkspaceSchedulerRegistrySlotV1::Retiring {
                    scheduler,
                    close_in_progress: false,
                }) => {
                    self.inner.emit(
                        EventCategoryV1::TerminalOrRequeueOrSaturation,
                        SeverityV1::Warn,
                    );
                    return Err(DaemonCompositionErrorV1::SchedulerRetiring {
                        generation: scheduler.generation(),
                    });
                }
                None => {}
            }

            if state.creating.contains(&key) {
                state = condvar_wait(&self.inner.changed, state);
                drop(state);
                continue;
            }

            let generation = match state.generations.get(&key).copied() {
                Some(generation) => generation.next()?,
                None => SchedulerGenerationV1::initial(),
            };
            state.generations.insert(key.clone(), generation);
            state.creating.insert(key.clone());
            drop(state);

            let context = match catch_unwind(AssertUnwindSafe(|| {
                factory
                    .take()
                    .expect("factory remains available until this caller reserves a generation")(
                )
            })) {
                Ok(context) => context,
                Err(payload) => {
                    let mut state = mutex_lock(&self.inner.state);
                    state.creating.remove(&key);
                    drop(state);
                    self.inner.changed.notify_all();
                    resume_unwind(payload);
                }
            };

            let scheduler = Arc::new(WorkspaceSchedulerV1::new(key.clone(), generation, context));
            let mut state = mutex_lock(&self.inner.state);
            state.creating.remove(&key);
            state.slots.insert(
                key,
                WorkspaceSchedulerRegistrySlotV1::Active(Arc::clone(&scheduler)),
            );
            drop(state);
            self.inner
                .emit(EventCategoryV1::Scheduler, SeverityV1::Info);
            self.inner.changed.notify_all();
            return Ok(scheduler);
        }
    }

    /// Marks this exact active generation retiring, then closes/drains it outside registry locks.
    ///
    /// A close failure leaves the slot retiring. The returned retry capability is the only way to
    /// attempt another close; the registry never reopens or recreates that generation on failure.
    /// A close panic is resumed only after the retiring slot is made retryable and waiters wake.
    pub fn retire<F, E>(
        &self,
        scheduler: &Arc<WorkspaceSchedulerV1<C>>,
        close: F,
    ) -> Result<(), WorkspaceSchedulerRetirementErrorV1<C, E>>
    where
        F: FnOnce(&WorkspaceSchedulerV1<C>) -> Result<(), E>,
    {
        let retry = begin_retirement(Arc::clone(&self.inner), scheduler)
            .map_err(WorkspaceSchedulerRetirementErrorV1::Start)?;
        finish_retirement(&retry, close)
    }
}

impl<C> Clone for WorkspaceSchedulerRegistryV1<C> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<C> Default for WorkspaceSchedulerRegistryV1<C> {
    fn default() -> Self {
        Self::new()
    }
}

struct WorkspaceSchedulerRegistryInnerV1<C> {
    state: Mutex<WorkspaceSchedulerRegistryStateV1<C>>,
    changed: Condvar,
    observability: Option<Arc<Mutex<ObservabilityV1>>>,
}

struct WorkspaceSchedulerRegistryStateV1<C> {
    slots: HashMap<WorkspaceSchedulerKeyV1, WorkspaceSchedulerRegistrySlotV1<C>>,
    generations: HashMap<WorkspaceSchedulerKeyV1, SchedulerGenerationV1>,
    creating: HashSet<WorkspaceSchedulerKeyV1>,
}

enum WorkspaceSchedulerRegistrySlotV1<C> {
    Active(Arc<WorkspaceSchedulerV1<C>>),
    Retiring {
        scheduler: Arc<WorkspaceSchedulerV1<C>>,
        close_in_progress: bool,
    },
}

fn begin_retirement<C>(
    inner: Arc<WorkspaceSchedulerRegistryInnerV1<C>>,
    scheduler: &Arc<WorkspaceSchedulerV1<C>>,
) -> Result<WorkspaceSchedulerRetirementRetryV1<C>, WorkspaceSchedulerRetirementStartErrorV1<C>> {
    let key = scheduler.key().clone();
    let generation = scheduler.generation();
    let retry = WorkspaceSchedulerRetirementRetryV1 {
        inner: Arc::clone(&inner),
        scheduler: Arc::clone(scheduler),
    };
    let mut state = mutex_lock(&inner.state);

    let active_scheduler = match state.slots.get(&key) {
        Some(WorkspaceSchedulerRegistrySlotV1::Active(current))
            if current.generation() == generation && Arc::ptr_eq(current, scheduler) =>
        {
            Arc::clone(current)
        }
        Some(WorkspaceSchedulerRegistrySlotV1::Retiring {
            scheduler: current,
            close_in_progress,
        }) if current.generation() == generation && Arc::ptr_eq(current, scheduler) => {
            if *close_in_progress {
                return Err(WorkspaceSchedulerRetirementStartErrorV1::CloseInProgress {
                    key,
                    generation,
                });
            }
            return Err(WorkspaceSchedulerRetirementStartErrorV1::AlreadyRetiring { retry });
        }
        Some(_) => {
            return Err(WorkspaceSchedulerRetirementStartErrorV1::NotCurrent { key, generation });
        }
        None => {
            return Err(WorkspaceSchedulerRetirementStartErrorV1::NotRegistered {
                key,
                generation,
            });
        }
    };

    state.slots.insert(
        key,
        WorkspaceSchedulerRegistrySlotV1::Retiring {
            scheduler: active_scheduler,
            close_in_progress: true,
        },
    );
    drop(state);
    Ok(retry)
}

fn begin_retirement_retry<C>(
    retry: &WorkspaceSchedulerRetirementRetryV1<C>,
) -> Result<(), WorkspaceSchedulerRetirementStartErrorV1<C>> {
    let key = retry.scheduler.key().clone();
    let generation = retry.scheduler.generation();
    let mut state = mutex_lock(&retry.inner.state);
    match state.slots.get_mut(&key) {
        Some(WorkspaceSchedulerRegistrySlotV1::Retiring {
            scheduler,
            close_in_progress,
        }) if scheduler.generation() == generation && Arc::ptr_eq(scheduler, &retry.scheduler) => {
            if *close_in_progress {
                return Err(WorkspaceSchedulerRetirementStartErrorV1::CloseInProgress {
                    key,
                    generation,
                });
            }
            *close_in_progress = true;
            Ok(())
        }
        Some(_) => Err(WorkspaceSchedulerRetirementStartErrorV1::NotCurrent { key, generation }),
        None => Err(WorkspaceSchedulerRetirementStartErrorV1::NotRegistered { key, generation }),
    }
}

fn finish_retirement<C, F, E>(
    retry: &WorkspaceSchedulerRetirementRetryV1<C>,
    close: F,
) -> Result<(), WorkspaceSchedulerRetirementErrorV1<C, E>>
where
    F: FnOnce(&WorkspaceSchedulerV1<C>) -> Result<(), E>,
{
    match catch_unwind(AssertUnwindSafe(|| close(&retry.scheduler))) {
        Ok(Ok(())) => {
            let key = retry.scheduler.key().clone();
            let generation = retry.scheduler.generation();
            let mut state = mutex_lock(&retry.inner.state);
            let is_current = matches!(
                state.slots.get(&key),
                Some(WorkspaceSchedulerRegistrySlotV1::Retiring {
                    scheduler,
                    close_in_progress: true,
                }) if scheduler.generation() == generation && Arc::ptr_eq(scheduler, &retry.scheduler)
            );
            if is_current {
                state.slots.remove(&key);
            }
            drop(state);
            if is_current {
                retry.inner.changed.notify_all();
                Ok(())
            } else {
                Err(WorkspaceSchedulerRetirementErrorV1::StaleCompletion { key, generation })
            }
        }
        Ok(Err(source)) => {
            let key = retry.scheduler.key().clone();
            let generation = retry.scheduler.generation();
            if clear_close_in_progress(retry) {
                Err(WorkspaceSchedulerRetirementErrorV1::CloseFailed {
                    source,
                    retry: retry.clone(),
                })
            } else {
                Err(WorkspaceSchedulerRetirementErrorV1::CloseFailedStale {
                    source,
                    key,
                    generation,
                })
            }
        }
        Err(payload) => {
            clear_close_in_progress(retry);
            resume_unwind(payload);
        }
    }
}
fn clear_close_in_progress<C>(retry: &WorkspaceSchedulerRetirementRetryV1<C>) -> bool {
    let key = retry.scheduler.key().clone();
    let generation = retry.scheduler.generation();
    let mut state = mutex_lock(&retry.inner.state);
    let is_current = matches!(
        state.slots.get(&key),
        Some(WorkspaceSchedulerRegistrySlotV1::Retiring {
            scheduler,
            close_in_progress: true,
        }) if scheduler.generation() == generation && Arc::ptr_eq(scheduler, &retry.scheduler)
    );
    if let (
        true,
        Some(WorkspaceSchedulerRegistrySlotV1::Retiring {
            close_in_progress, ..
        }),
    ) = (is_current, state.slots.get_mut(&key))
    {
        *close_in_progress = false;
    }
    drop(state);
    retry.inner.changed.notify_all();
    is_current
}

impl<C> WorkspaceSchedulerRegistryInnerV1<C> {
    fn emit(&self, category: EventCategoryV1, severity: SeverityV1) {
        let observer = self
            .observability
            .as_ref()
            .and_then(|observability| observability.try_lock().ok());
        if let Some(observability) = observer {
            observability.emit(category, severity);
        }
    }
}

fn mutex_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .expect("workspace scheduler state lock must not be poisoned")
}

fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read()
        .expect("workspace scheduler context lock must not be poisoned")
}

fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .expect("workspace scheduler context lock must not be poisoned")
}

fn condvar_wait<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar
        .wait(guard)
        .expect("workspace scheduler wait lock must not be poisoned")
}
#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    use podway_core::{Sha256Digest, WorkspaceId};
    use podway_store::DurableWorktreeIdentityV1;

    use super::*;

    fn test_key() -> WorkspaceSchedulerKeyV1 {
        let identity = DurableWorktreeIdentityV1::new(
            Sha256Digest::new(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .expect("fixture common-directory digest is valid"),
            WorkspaceId::new("00000000-0000-0000-0000-000000000012")
                .expect("fixture workspace UUID is valid"),
            Sha256Digest::new(
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )
            .expect("fixture worktree-administration digest is valid"),
        );
        WorkspaceSchedulerKeyV1::from_durable_identity(&identity)
    }

    #[test]
    fn stale_close_completion_does_not_remove_a_replacement_generation() {
        let registry = WorkspaceSchedulerRegistryV1::new();
        let key = test_key();
        let retiring = registry
            .get_or_create(key.clone(), || ())
            .expect("initial scheduler generation is valid");
        let retry = begin_retirement(Arc::clone(&registry.inner), &retiring)
            .expect("initial scheduler marks its exact generation retiring");
        let close_entered = Arc::new(Barrier::new(2));
        let close_complete = Arc::new(Barrier::new(2));
        let completion = {
            let close_entered = Arc::clone(&close_entered);
            let close_complete = Arc::clone(&close_complete);
            thread::spawn(move || {
                finish_retirement(&retry, |_| {
                    close_entered.wait();
                    close_complete.wait();
                    Ok::<(), ()>(())
                })
            })
        };

        close_entered.wait();
        let replacement = Arc::new(WorkspaceSchedulerV1::new(
            key.clone(),
            retiring
                .generation()
                .next()
                .expect("replacement generation advances"),
            (),
        ));
        let mut state = mutex_lock(&registry.inner.state);
        state.slots.insert(
            key.clone(),
            WorkspaceSchedulerRegistrySlotV1::Active(Arc::clone(&replacement)),
        );
        state
            .generations
            .insert(key.clone(), replacement.generation());
        drop(state);
        registry.inner.changed.notify_all();

        close_complete.wait();
        assert!(matches!(
            completion
                .join()
                .expect("stale completion thread does not panic"),
            Err(WorkspaceSchedulerRetirementErrorV1::StaleCompletion { .. })
        ));
        let current = registry
            .get_or_create(key, || ())
            .expect("replacement remains registered after stale completion");
        assert!(Arc::ptr_eq(&current, &replacement));
    }

    #[test]
    fn lifecycle_rebind_retires_before_replacement_and_preserves_on_failure() {
        let scheduler =
            WorkspaceSchedulerV1::new(test_key(), SchedulerGenerationV1::initial(), 11_usize);
        let rejected = scheduler.rebind_context_serialized_after(Arc::new(22), |current| {
            assert_eq!(**current, 11);
            Err::<(), _>("retirement failed")
        });
        assert_eq!(rejected, Err("retirement failed"));
        assert_eq!(*scheduler.context_snapshot(), 11);

        let prior = scheduler
            .rebind_context_serialized_after(Arc::new(22), |current| {
                assert_eq!(**current, 11);
                Ok::<(), &str>(())
            })
            .expect("successful retirement must publish the replacement");
        assert_eq!(*prior, 11);
        assert_eq!(*scheduler.context_snapshot(), 22);
    }
}
