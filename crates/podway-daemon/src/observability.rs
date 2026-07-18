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
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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

/// Stable, content-free diagnostic identifiers.
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

/// Inputs are always represented by this fixed marker, never their original contents.
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
}

#[derive(Default)]
struct Counters {
    primary_dropped: AtomicU64,
    fallback_dropped: AtomicU64,
    stderr_dropped: AtomicU64,
    sink_failures: AtomicU64,
    unflushed: AtomicU64,
}
impl Counters {
    fn snapshot(&self) -> ObservabilityCountersV1 {
        ObservabilityCountersV1 {
            primary_dropped: self.primary_dropped.load(Ordering::Relaxed),
            fallback_dropped: self.fallback_dropped.load(Ordering::Relaxed),
            stderr_dropped: self.stderr_dropped.load(Ordering::Relaxed),
            sink_failures: self.sink_failures.load(Ordering::Relaxed),
            unflushed: self.unflushed.load(Ordering::Relaxed),
        }
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
    sink: Arc<dyn LogSinkV1>,
    clock: Arc<dyn ClockV1>,
    stopped: AtomicBool,
    in_flight: AtomicU64,
}

/// The only producer API. Emission failures are intentionally not returned to domain callers.
pub struct ObservabilityV1 {
    shared: Arc<Shared>,
    done: mpsc::Receiver<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ObservabilityV1 {
    pub fn start(sink: Arc<dyn LogSinkV1>, clock: Arc<dyn ClockV1>) -> Self {
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
        });
        let (sender, done) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let worker = thread::spawn(move || {
            writer_loop(worker_shared);
            let _ = sender.send(());
        });
        Self {
            shared,
            done,
            worker: Some(worker),
        }
    }

    /// Emits only categorical metadata. This method never performs I/O and never returns an error.
    pub fn emit(&self, category: EventCategoryV1, severity: SeverityV1) {
        let event = format_event(self.shared.clock.unix_seconds(), category, severity);
        if self.enqueue_primary(event.clone(), severity.uses_fallback()) {
            return;
        }
        self.shared
            .counters
            .primary_dropped
            .fetch_add(1, Ordering::Relaxed);
        if severity.uses_fallback() && self.enqueue_fallback(event) {
            return;
        }
        if severity.uses_fallback() {
            self.shared
                .counters
                .fallback_dropped
                .fetch_add(1, Ordering::Relaxed);
        }
        // Bounded stderr fallback is categorical; its byte size is fixed and below 1 KiB.
        let deadline = Instant::now() + MAX_EMIT_WAIT_V1;
        if Instant::now() <= deadline {
            let _ = writeln!(
                io::stderr(),
                "podwayd observability {} dropped",
                severity.name()
            );
        } else {
            self.shared
                .counters
                .stderr_dropped
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn counters(&self) -> ObservabilityCountersV1 {
        self.shared.counters.snapshot()
    }

    /// Requests a bounded flush. A stuck sink is detached rather than delaying daemon shutdown.
    pub fn shutdown(mut self) -> ObservabilityCountersV1 {
        {
            let mut queue = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            queue.stopping = true;
            self.shared.available.notify_all();
        }
        if self.done.recv_timeout(MAX_SHUTDOWN_FLUSH_V1).is_ok() {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        } else {
            let unflushed = {
                let queue = self
                    .shared
                    .queue
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                (queue.primary.len() + queue.fallback.len()) as u64
            }
            .saturating_add(self.shared.in_flight.load(Ordering::Acquire));
            self.shared
                .counters
                .unflushed
                .fetch_add(unflushed, Ordering::Relaxed);
            self.worker.take();
            eprintln!("podwayd observability unflushed={}", unflushed);
        }
        self.shared.stopped.store(true, Ordering::Release);
        self.counters()
    }

    fn enqueue_primary(&self, event: String, wait: bool) -> bool {
        enqueue(&self.shared, event, false, wait)
    }
    fn enqueue_fallback(&self, event: String) -> bool {
        enqueue(&self.shared, event, true, true)
    }
}
impl Drop for ObservabilityV1 {
    fn drop(&mut self) {
        if !self.shared.stopped.swap(true, Ordering::AcqRel) {
            let mut queue = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            queue.stopping = true;
            self.shared.available.notify_all();
        }
    }
}

fn enqueue(shared: &Shared, event: String, fallback: bool, wait: bool) -> bool {
    let deadline = Instant::now() + MAX_EMIT_WAIT_V1;
    loop {
        if let Ok(mut queue) = shared.queue.try_lock() {
            if queue.stopping {
                return false;
            }
            let target = if fallback {
                &mut queue.fallback
            } else {
                &mut queue.primary
            };
            let capacity = if fallback {
                FALLBACK_CAPACITY_V1
            } else {
                PRIMARY_CAPACITY_V1
            };
            if target.len() < capacity {
                target.push_back(event);
                shared.available.notify_one();
                return true;
            }
            return false;
        }
        if !wait || Instant::now() >= deadline {
            return false;
        }
        thread::yield_now();
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
            queue
                .primary
                .pop_front()
                .or_else(|| queue.fallback.pop_front())
                .or_else(|| {
                    if queue.stopping {
                        None
                    } else {
                        Some(String::new())
                    }
                })
        };
        match event {
            Some(event) if !event.is_empty() => {
                shared.in_flight.store(1, Ordering::Release);
                if shared.sink.write_event(&event).is_err() {
                    shared
                        .counters
                        .sink_failures
                        .fetch_add(1, Ordering::Relaxed);
                }
                shared.in_flight.store(0, Ordering::Release);
            }
            Some(_) => {
                if shared.sink.flush().is_err() {
                    shared
                        .counters
                        .sink_failures
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            None => {
                if shared.sink.flush().is_err() {
                    shared
                        .counters
                        .sink_failures
                        .fetch_add(1, Ordering::Relaxed);
                }
                return;
            }
        }
    }
}

fn format_event(seconds: u64, category: EventCategoryV1, severity: SeverityV1) -> String {
    let event = format!(
        "ts={} severity={} category={} detail={}\n",
        seconds,
        severity.name(),
        category.name(),
        REDACTED_V1
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
        let overflow = self
            .path
            .with_extension(format!("log.{}", RETAINED_ROTATIONS_V1));
        let _ = fs::remove_file(&overflow);
        let legacy_overflow = self
            .path
            .with_extension(format!("log.{}", RETAINED_ROTATIONS_V1 + 1));
        let _ = fs::remove_file(&legacy_overflow);
        for index in (1..RETAINED_ROTATIONS_V1).rev() {
            let from = self.path.with_extension(format!("log.{}", index));
            let to = self.path.with_extension(format!("log.{}", index + 1));
            if from.exists() {
                let _ = fs::remove_file(&to);
                fs::rename(from, to)?;
            }
        }
        let first = self.path.with_extension("log.1");
        let _ = fs::remove_file(&first);
        if self.path.exists() {
            fs::rename(&self.path, first)?;
        }
        state.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        state.bytes = 0;
        self.prune_retention();
        Ok(())
    }
    fn prune_retention(&self) {
        let cutoff = self
            .clock
            .unix_seconds()
            .saturating_sub(RETENTION_DAYS_V1 * 24 * 60 * 60);
        for index in 1..=RETAINED_ROTATIONS_V1 {
            let candidate = self.path.with_extension(format!("log.{}", index));
            let is_stale = fs::metadata(&candidate)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|time| time.as_secs() < cutoff)
                .unwrap_or(false);
            if is_stale {
                let _ = fs::remove_file(candidate);
            }
        }
    }
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
