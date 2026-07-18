//! Bounded, non-authoritative daemon diagnostics.
//!
//! Events intentionally carry only a stable category and severity. Callers cannot attach request,
//! task, item, artifact, or user supplied data to this interface.

use std::{
    collections::VecDeque,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, TryLockError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const MAX_EVENT_BYTES_V1: usize = 8 * 1024;
pub const MAX_STRING_BYTES_V1: usize = 256;
pub const PRIMARY_CAPACITY_V1: usize = 4096;
pub const FALLBACK_CAPACITY_V1: usize = 256;
pub const MAX_EMIT_WAIT_V1: Duration = Duration::from_millis(2);
pub const MAX_WRITER_CYCLE_V1: Duration = Duration::from_millis(50);
pub const MAX_SHUTDOWN_FLUSH_V1: Duration = Duration::from_secs(2);
pub const ROTATION_BYTES_V1: u64 = 10 * 1024 * 1024;
pub const RETAINED_ROTATIONS_V1: usize = 5;
pub const RETENTION_DAYS_V1: u64 = 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventCategoryV1 {
    Lifecycle,
    StaleSocketOrLock,
    Scheduler,
    Admission,
    Idempotency,
    CancelOrClaim,
    TerminalOrRequeueOrSaturation,
    MigrationOrIntegrity,
    MoveOrRepair,
    ServiceOutcome,
    Rotation,
    StaleEvidence,
}
impl EventCategoryV1 {
    const fn name(self) -> &'static str {
        match self {
            Self::Lifecycle => "lifecycle",
            Self::StaleSocketOrLock => "stale_socket_or_lock",
            Self::Scheduler => "scheduler",
            Self::Admission => "admission",
            Self::Idempotency => "idempotency",
            Self::CancelOrClaim => "cancel_or_claim",
            Self::TerminalOrRequeueOrSaturation => "terminal_or_requeue_or_saturation",
            Self::MigrationOrIntegrity => "migration_or_integrity",
            Self::MoveOrRepair => "move_or_repair",
            Self::ServiceOutcome => "service_outcome",
            Self::Rotation => "rotation",
            Self::StaleEvidence => "stale_evidence",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeverityV1 {
    Debug,
    Info,
    Warn,
    Error,
}
impl SeverityV1 {
    const fn name(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
    const fn uses_fallback(self) -> bool {
        matches!(self, Self::Warn | Self::Error)
    }
}

pub const REDACTED_V1: &str = "[redacted]";
pub fn redact_v1(_: &str) -> &'static str {
    REDACTED_V1
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ObservabilityCountersV1 {
    pub primary_dropped: u64,
    pub fallback_dropped: u64,
    pub stderr_dropped: u64,
    pub sink_failures: u64,
    pub unflushed: u64,
    pub queued: u64,
    pub writing: u64,
    pub flush_failures: u64,
    pub flushing: u64,
    pub admission_contention: u64,
    pub admission_poisoned: u64,
    pub stopped_dropped: u64,
    pub degraded_dropped: u64,
    pub bootstrap_failures: u64,
    pub worker_panics: u64,
    pub worker_disconnects: u64,
    pub worker_join_failures: u64,
    pub shutdown_timeouts: u64,
}
#[derive(Default)]
struct Counters {
    primary_dropped: AtomicU64,
    fallback_dropped: AtomicU64,
    stderr_dropped: AtomicU64,
    sink_failures: AtomicU64,
    unflushed: AtomicU64,
    queued: AtomicU64,
    writing: AtomicU64,
    flush_failures: AtomicU64,
    flushing: AtomicU64,
    admission_contention: AtomicU64,
    admission_poisoned: AtomicU64,
    stopped_dropped: AtomicU64,
    degraded_dropped: AtomicU64,
    bootstrap_failures: AtomicU64,
    worker_panics: AtomicU64,
    worker_disconnects: AtomicU64,
    worker_join_failures: AtomicU64,
    shutdown_timeouts: AtomicU64,
}
macro_rules! counters_snapshot {
    ($c:expr) => {
        ObservabilityCountersV1 {
            primary_dropped: $c.primary_dropped.load(Ordering::Relaxed),
            fallback_dropped: $c.fallback_dropped.load(Ordering::Relaxed),
            stderr_dropped: $c.stderr_dropped.load(Ordering::Relaxed),
            sink_failures: $c.sink_failures.load(Ordering::Relaxed),
            unflushed: $c.unflushed.load(Ordering::Relaxed),
            queued: $c.queued.load(Ordering::Relaxed),
            writing: $c.writing.load(Ordering::Relaxed),
            flush_failures: $c.flush_failures.load(Ordering::Relaxed),
            flushing: $c.flushing.load(Ordering::Relaxed),
            admission_contention: $c.admission_contention.load(Ordering::Relaxed),
            admission_poisoned: $c.admission_poisoned.load(Ordering::Relaxed),
            stopped_dropped: $c.stopped_dropped.load(Ordering::Relaxed),
            degraded_dropped: $c.degraded_dropped.load(Ordering::Relaxed),
            bootstrap_failures: $c.bootstrap_failures.load(Ordering::Relaxed),
            worker_panics: $c.worker_panics.load(Ordering::Relaxed),
            worker_disconnects: $c.worker_disconnects.load(Ordering::Relaxed),
            worker_join_failures: $c.worker_join_failures.load(Ordering::Relaxed),
            shutdown_timeouts: $c.shutdown_timeouts.load(Ordering::Relaxed),
        }
    };
}
impl Counters {
    fn snapshot(&self) -> ObservabilityCountersV1 {
        counters_snapshot!(self)
    }
}

pub trait LogSinkV1: Send + Sync + 'static {
    fn write_event(&self, event: &str) -> io::Result<()>;
    fn flush(&self) -> io::Result<()>;
}
pub trait ClockV1: Send + Sync + 'static {
    fn unix_seconds(&self) -> u64;
}
#[derive(Default)]
pub struct SystemClockV1;
impl ClockV1 for SystemClockV1 {
    fn unix_seconds(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

struct Queue {
    primary: VecDeque<String>,
    fallback: VecDeque<String>,
    stopping: bool,
}
struct Shared {
    queue: Mutex<Queue>,
    available: Condvar,
    counters: Counters,
    sink: Option<Arc<dyn LogSinkV1>>,
    clock: Arc<dyn ClockV1>,
    stopped: AtomicBool,
    in_flight: AtomicU64,
    degraded: bool,
}
/// Cloneable, non-blocking producer held by runtime code. It never owns worker lifecycle.
#[derive(Clone)]
pub struct ObservabilityEmitterV1 {
    shared: Arc<Shared>,
}
/// Sole owner of the writer worker. Shutdown consumes this value exactly once.
pub struct ObservabilityV1 {
    emitter: ObservabilityEmitterV1,
    done: Option<mpsc::Receiver<WorkerReport>>,
    worker: Option<thread::JoinHandle<()>>,
}
#[derive(Clone, Copy)]
enum WorkerReport {
    Completed,
    Panicked,
}

impl ObservabilityV1 {
    pub fn start(sink: Arc<dyn LogSinkV1>, clock: Arc<dyn ClockV1>) -> Self {
        Self::start_inner(Some(sink), clock)
    }
    /// A typed bootstrap degradation: no sink could be opened, so every subsequent event is counted.
    pub fn start_degraded(clock: Arc<dyn ClockV1>) -> Self {
        Self::start_inner(None, clock)
    }
    fn start_inner(sink: Option<Arc<dyn LogSinkV1>>, clock: Arc<dyn ClockV1>) -> Self {
        let degraded = sink.is_none();
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                primary: VecDeque::with_capacity(PRIMARY_CAPACITY_V1),
                fallback: VecDeque::with_capacity(FALLBACK_CAPACITY_V1),
                stopping: false,
            }),
            available: Condvar::new(),
            counters: Counters::default(),
            sink,
            clock,
            stopped: AtomicBool::new(false),
            in_flight: AtomicU64::new(0),
            degraded,
        });
        if degraded {
            shared
                .counters
                .bootstrap_failures
                .fetch_add(1, Ordering::Relaxed);
        }
        let emitter = ObservabilityEmitterV1 {
            shared: Arc::clone(&shared),
        };
        let (done, worker) = if degraded {
            (None, None)
        } else {
            let (sender, receiver) = mpsc::channel();
            let worker_shared = Arc::clone(&shared);
            let worker = thread::spawn(move || {
                let report = if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    writer_loop(worker_shared)
                }))
                .is_ok()
                {
                    WorkerReport::Completed
                } else {
                    WorkerReport::Panicked
                };
                let _ = sender.send(report);
            });
            (Some(receiver), Some(worker))
        };
        Self {
            emitter,
            done,
            worker,
        }
    }
    pub fn emitter(&self) -> ObservabilityEmitterV1 {
        self.emitter.clone()
    }
    pub fn emit(&self, category: EventCategoryV1, severity: SeverityV1) {
        self.emitter.emit(category, severity);
    }
    pub fn counters(&self) -> ObservabilityCountersV1 {
        self.emitter.counters()
    }
    /// Requests bounded shutdown; a blocked worker is detached and the exact remaining state is counted.
    pub fn shutdown(mut self) -> ObservabilityCountersV1 {
        self.emitter.request_stop();
        if let Some(done) = self.done.take() {
            match done.recv_timeout(MAX_SHUTDOWN_FLUSH_V1) {
                Ok(WorkerReport::Completed) => {
                    if self
                        .worker
                        .take()
                        .is_some_and(|worker| worker.join().is_err())
                    {
                        self.emitter
                            .shared
                            .counters
                            .worker_join_failures
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
                Ok(WorkerReport::Panicked) => {
                    self.emitter
                        .shared
                        .counters
                        .worker_panics
                        .fetch_add(1, Ordering::Relaxed);
                    let _ = self.worker.take().map(thread::JoinHandle::join);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.emitter
                        .shared
                        .counters
                        .shutdown_timeouts
                        .fetch_add(1, Ordering::Relaxed);
                    self.record_unflushed();
                    self.worker.take();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.emitter
                        .shared
                        .counters
                        .worker_disconnects
                        .fetch_add(1, Ordering::Relaxed);
                    self.record_unflushed();
                    self.worker.take();
                }
            }
        }
        self.emitter.shared.stopped.store(true, Ordering::Release);
        self.counters()
    }
    fn record_unflushed(&self) {
        let queued = self.emitter.shared.counters.queued.load(Ordering::Acquire);
        self.emitter.shared.counters.unflushed.fetch_add(
            queued.saturating_add(self.emitter.shared.in_flight.load(Ordering::Acquire)),
            Ordering::Relaxed,
        );
    }
}
impl Drop for ObservabilityV1 {
    fn drop(&mut self) {
        self.emitter.request_stop();
    }
}
impl ObservabilityEmitterV1 {
    pub fn emit(&self, category: EventCategoryV1, severity: SeverityV1) {
        if self.shared.degraded {
            self.shared
                .counters
                .degraded_dropped
                .fetch_add(1, Ordering::Relaxed);
            self.shared
                .counters
                .sink_failures
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        let event = format_event(self.shared.clock.unix_seconds(), category, severity);
        let deadline = std::time::Instant::now() + MAX_EMIT_WAIT_V1;
        loop {
            match self.shared.queue.try_lock() {
                Ok(mut queue) => {
                    if queue.stopping || self.shared.stopped.load(Ordering::Acquire) {
                        self.shared
                            .counters
                            .stopped_dropped
                            .fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    if queue.primary.len() < PRIMARY_CAPACITY_V1 {
                        queue.primary.push_back(event);
                        self.shared.counters.queued.fetch_add(1, Ordering::Release);
                        self.shared.available.notify_one();
                        return;
                    }
                    self.shared
                        .counters
                        .primary_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    if severity.uses_fallback() && queue.fallback.len() < FALLBACK_CAPACITY_V1 {
                        queue.fallback.push_back(event);
                        self.shared.counters.queued.fetch_add(1, Ordering::Release);
                        self.shared.available.notify_one();
                        return;
                    }
                    if severity.uses_fallback() {
                        self.shared
                            .counters
                            .fallback_dropped
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    return;
                }
                Err(TryLockError::Poisoned(_)) => {
                    self.shared
                        .counters
                        .admission_poisoned
                        .fetch_add(1, Ordering::Relaxed);
                    self.shared
                        .counters
                        .primary_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                }
                Err(TryLockError::WouldBlock) if std::time::Instant::now() >= deadline => {
                    self.shared
                        .counters
                        .admission_contention
                        .fetch_add(1, Ordering::Relaxed);
                    self.shared
                        .counters
                        .primary_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    if severity.uses_fallback() {
                        self.shared
                            .counters
                            .fallback_dropped
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    return;
                }
                Err(TryLockError::WouldBlock) => thread::yield_now(),
            }
        }
    }
    pub fn counters(&self) -> ObservabilityCountersV1 {
        self.shared.counters.snapshot()
    }
    fn request_stop(&self) {
        self.shared.stopped.store(true, Ordering::Release);
        match self.shared.queue.lock() {
            Ok(mut queue) => {
                queue.stopping = true;
                self.shared.available.notify_all();
            }
            Err(poison) => {
                self.shared
                    .counters
                    .admission_poisoned
                    .fetch_add(1, Ordering::Relaxed);
                let mut queue = poison.into_inner();
                queue.stopping = true;
                self.shared.available.notify_all();
            }
        }
    }
}

fn writer_loop(shared: Arc<Shared>) {
    loop {
        let event = {
            let mut queue = shared
                .queue
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            while queue.primary.is_empty() && queue.fallback.is_empty() && !queue.stopping {
                let (next, _) = shared
                    .available
                    .wait_timeout(queue, MAX_WRITER_CYCLE_V1)
                    .unwrap_or_else(|poison| poison.into_inner());
                queue = next;
            }
            // WARN/ERROR is strict priority, so sustained primary traffic cannot starve it.
            let event = queue
                .fallback
                .pop_front()
                .or_else(|| queue.primary.pop_front());
            if event.is_some() {
                shared.counters.queued.fetch_sub(1, Ordering::AcqRel);
            }
            event
        };
        match event {
            Some(event) => {
                shared.in_flight.store(1, Ordering::Release);
                shared.counters.writing.store(1, Ordering::Release);
                if shared
                    .sink
                    .as_ref()
                    .is_some_and(|sink| sink.write_event(&event).is_err())
                {
                    shared
                        .counters
                        .sink_failures
                        .fetch_add(1, Ordering::Relaxed);
                }
                shared.counters.writing.store(0, Ordering::Release);
                shared.in_flight.store(0, Ordering::Release);
            }
            None => {
                shared.counters.flushing.store(1, Ordering::Release);
                if shared
                    .sink
                    .as_ref()
                    .is_some_and(|sink| sink.flush().is_err())
                {
                    shared
                        .counters
                        .flush_failures
                        .fetch_add(1, Ordering::Relaxed);
                    shared
                        .counters
                        .sink_failures
                        .fetch_add(1, Ordering::Relaxed);
                }
                shared.counters.flushing.store(0, Ordering::Release);
                return;
            }
        }
    }
}
fn format_event(seconds: u64, category: EventCategoryV1, severity: SeverityV1) -> String {
    let event = format!(
        "ts={seconds} severity={} category={} detail={REDACTED_V1}\n",
        severity.name(),
        category.name()
    );
    debug_assert!(event.len() <= MAX_EVENT_BYTES_V1 && REDACTED_V1.len() <= MAX_STRING_BYTES_V1);
    event
}

/// Local file sink with fixed-size rotation and deterministic retention pruning.
pub struct RotatingFileSinkV1 {
    path: PathBuf,
    clock: Arc<dyn ClockV1>,
    state: Mutex<FileState>,
}
struct FileState {
    file: File,
    bytes: u64,
}
impl RotatingFileSinkV1 {
    pub fn open(path: impl AsRef<Path>, clock: Arc<dyn ClockV1>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        prune_retention(&path, clock.as_ref())?;
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let bytes = file.metadata()?.len();
        Ok(Self {
            path,
            clock,
            state: Mutex::new(FileState { file, bytes }),
        })
    }
    fn rotate(&self, state: &mut FileState) -> io::Result<()> {
        state.file.flush()?;
        remove_if_present(
            &self
                .path
                .with_extension(format!("log.{}", RETAINED_ROTATIONS_V1 + 1)),
        )?;
        remove_if_present(
            &self
                .path
                .with_extension(format!("log.{RETAINED_ROTATIONS_V1}")),
        )?;
        for index in (1..RETAINED_ROTATIONS_V1).rev() {
            let from = self.path.with_extension(format!("log.{index}"));
            let to = self.path.with_extension(format!("log.{}", index + 1));
            match fs::rename(&from, &to) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let first = self.path.with_extension("log.1");
        remove_if_present(&first)?;
        match fs::rename(&self.path, &first) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        state.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        state.bytes = 0;
        prune_retention(&self.path, self.clock.as_ref())
    }
}
fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
fn prune_retention(path: &Path, clock: &dyn ClockV1) -> io::Result<()> {
    let cutoff = clock
        .unix_seconds()
        .saturating_sub(RETENTION_DAYS_V1 * 24 * 60 * 60);
    for index in 1..=RETAINED_ROTATIONS_V1 + 1 {
        let candidate = path.with_extension(format!("log.{index}"));
        let metadata = match fs::metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?;
        if modified.as_secs() < cutoff {
            remove_if_present(&candidate)?;
        }
    }
    Ok(())
}
impl LogSinkV1 for RotatingFileSinkV1 {
    fn write_event(&self, event: &str) -> io::Result<()> {
        if event.len() > MAX_EVENT_BYTES_V1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "oversized event",
            ));
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.bytes.saturating_add(event.len() as u64) > ROTATION_BYTES_V1 {
            self.rotate(&mut state)?;
        }
        state.file.write_all(event.as_bytes())?;
        state.bytes = state.bytes.saturating_add(event.len() as u64);
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .file
            .flush()
    }
}
impl fmt::Debug for ObservabilityV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObservabilityV1")
            .field("counters", &self.counters())
            .finish()
    }
}
