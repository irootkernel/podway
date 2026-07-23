//! Bounded, non-authoritative daemon diagnostics.
//!
//! Event records are closed, typed, and carry no caller-provided strings or identifiers.

use std::{
    collections::VecDeque,
    fmt,
    fs::{self, File},
    io::{self, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, TryLockError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use nix::{
    fcntl::{OFlag, open},
    sys::stat::Mode,
    unistd::geteuid,
};

pub const MAX_EVENT_BYTES_V1: usize = 8 * 1024;
pub const PRIMARY_CAPACITY_V1: usize = 4096;
pub const FALLBACK_CAPACITY_V1: usize = 256;
pub const MAX_WRITER_CYCLE_V1: Duration = Duration::from_millis(50);
pub const MAX_SHUTDOWN_FLUSH_V1: Duration = Duration::from_secs(2);
const MAX_EMIT_QUEUE_WAIT_V1: Duration = Duration::from_millis(5);
pub const ROTATION_BYTES_V1: u64 = 10 * 1024 * 1024;
pub const RETAINED_ROTATIONS_V1: usize = 5;
const ADMISSION_RUNNING_V1: u64 = 0;
const ADMISSION_STOPPING_V1: u64 = 1;
const ADMISSION_FROZEN_V1: u64 = 2;

/// The concrete daemon operation being observed. This intentionally has no catch-all variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventOperationV1 {
    DaemonStart,
    DaemonStop,
    SchedulerCreated,
    SchedulerRetired,
    ConnectionAccepted,
    PeerAdmission,
    AdmissionSaturation,
    JobAdmission,
    IdempotentReplay,
    JobClaim,
    JobTerminal,
    QueueSaturation,
    IntegrityCheck,
    ArtifactMove,
    TransportServiceRequest,
    ServiceDispatch,
    JobWait,
    HandlerJoin,
    ConnectionSetup,
    ResponseWrite,
}
impl EventOperationV1 {
    const fn name(self) -> &'static str {
        match self {
            Self::DaemonStart => "daemon_start",
            Self::DaemonStop => "daemon_stop",
            Self::SchedulerCreated => "scheduler_created",
            Self::SchedulerRetired => "scheduler_retired",
            Self::ConnectionAccepted => "connection_accepted",
            Self::PeerAdmission => "peer_admission",
            Self::AdmissionSaturation => "admission_saturation",
            Self::JobAdmission => "job_admission",
            Self::IdempotentReplay => "idempotent_replay",
            Self::JobClaim => "job_claim",
            Self::JobTerminal => "job_terminal",
            Self::QueueSaturation => "queue_saturation",
            Self::IntegrityCheck => "integrity_check",
            Self::ArtifactMove => "artifact_move",
            Self::TransportServiceRequest => "transport_service_request",
            Self::ServiceDispatch => "service_dispatch",
            Self::JobWait => "job_wait",
            Self::HandlerJoin => "handler_join",
            Self::ConnectionSetup => "connection_setup",
            Self::ResponseWrite => "response_write",
        }
    }
}

/// A bounded outcome for an [`EventOperationV1`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventOutcomeV1 {
    Succeeded,
    Rejected,
    Retried,
    Failed,
    Saturated,
}
impl EventOutcomeV1 {
    const fn name(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Rejected => "rejected",
            Self::Retried => "retried",
            Self::Failed => "failed",
            Self::Saturated => "saturated",
        }
    }
    const fn uses_fallback(self) -> bool {
        matches!(self, Self::Rejected | Self::Failed | Self::Saturated)
    }
}

/// The only event accepted by the observability queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventRecordV1 {
    operation: EventOperationV1,
    outcome: EventOutcomeV1,
}
impl EventRecordV1 {
    pub const fn new(operation: EventOperationV1, outcome: EventOutcomeV1) -> Self {
        Self { operation, outcome }
    }
    pub const fn operation(self) -> EventOperationV1 {
        self.operation
    }
    pub const fn outcome(self) -> EventOutcomeV1 {
        self.outcome
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockErrorV1 {
    BeforeUnixEpoch,
}

/// Wall-clock boundary. A clock failure is a value; only a clock panic is caught as a panic.
pub trait ClockV1: Send + Sync + 'static {
    fn unix_seconds(&self) -> Result<u64, ClockErrorV1>;
}
#[derive(Default)]
pub struct SystemClockV1;
impl ClockV1 for SystemClockV1 {
    fn unix_seconds(&self) -> Result<u64, ClockErrorV1> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .map_err(|_| ClockErrorV1::BeforeUnixEpoch)
    }
}

pub trait LogSinkV1: Send + Sync + 'static {
    fn write_event(&self, event: &str) -> io::Result<()>;
    fn flush(&self) -> io::Result<()>;
}

/// All cumulative fields saturate. Gauges are bounded at zero and are never decremented below it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ObservabilityCountersV1 {
    pub accepted: u64,
    pub written: u64,
    pub primary_dropped: u64,
    pub fallback_dropped: u64,
    pub stopped_dropped: u64,
    pub degraded_dropped: u64,
    pub unflushed: u64,
    pub queued: u64,
    pub writing: u64,
    pub flushing: u64,
    pub write_failures: u64,
    pub flush_failures: u64,
    pub clock_errors: u64,
    pub clock_panics: u64,
    pub sink_failures: u64,
    /// Set exactly once when any cumulative counter first saturates.
    pub counters_saturated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservabilityFinalizationV1 {
    Completed,
    Detached,
    Indeterminate,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservabilityShutdownReportV1 {
    counters: ObservabilityCountersV1,
    finalization: ObservabilityFinalizationV1,
}
impl ObservabilityShutdownReportV1 {
    pub const fn counters(&self) -> ObservabilityCountersV1 {
        self.counters
    }
    pub const fn finalization(&self) -> ObservabilityFinalizationV1 {
        self.finalization
    }
}

#[derive(Default)]
struct Counters {
    sequence: AtomicU64,
    writer: Mutex<()>,
    accepted: AtomicU64,
    written: AtomicU64,
    primary_dropped: AtomicU64,
    fallback_dropped: AtomicU64,
    stopped_dropped: AtomicU64,
    degraded_dropped: AtomicU64,
    unflushed: AtomicU64,
    queued: AtomicU64,
    writing: AtomicU64,
    flushing: AtomicU64,
    write_failures: AtomicU64,
    flush_failures: AtomicU64,
    clock_errors: AtomicU64,
    clock_panics: AtomicU64,
    sink_failures: AtomicU64,
    saturated: AtomicBool,
}
impl Counters {
    fn begin(&self) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
    }
    fn end(&self) {
        self.sequence.fetch_add(1, Ordering::Release);
    }
    fn mutate(&self, mutation: impl FnOnce(&Self)) {
        let _writer = self
            .writer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        self.begin();
        mutation(self);
        self.end();
    }
    fn add_in_transaction(&self, counter: &AtomicU64, amount: u64) {
        let mut current = counter.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_add(amount);
            match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => {
                    if next == u64::MAX && current != u64::MAX {
                        self.saturated.store(true, Ordering::Release);
                    }
                    break;
                }
                Err(actual) => current = actual,
            }
        }
    }
    fn set_gauge_in_transaction(&self, gauge: &AtomicU64, value: u64) {
        gauge.store(value, Ordering::Release);
    }
    fn increment_gauge_in_transaction(&self, gauge: &AtomicU64) {
        gauge
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_add(1))
            })
            .ok();
    }
    fn decrement_gauge_in_transaction(&self, gauge: &AtomicU64) {
        gauge
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
    }
    fn add(&self, counter: &AtomicU64, amount: u64) {
        self.mutate(|counters| counters.add_in_transaction(counter, amount));
    }
    fn record_enqueue(&self) {
        self.mutate(|counters| {
            counters.increment_gauge_in_transaction(&counters.queued);
            counters.add_in_transaction(&counters.accepted, 1);
        });
    }
    fn record_dequeue_for_write(&self) {
        self.mutate(|counters| {
            counters.decrement_gauge_in_transaction(&counters.queued);
            counters.increment_gauge_in_transaction(&counters.writing);
        });
    }
    fn record_write_success(&self) {
        self.mutate(|counters| {
            counters.decrement_gauge_in_transaction(&counters.writing);
            counters.add_in_transaction(&counters.written, 1);
        });
    }
    fn snapshot(&self) -> ObservabilityCountersV1 {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                thread::yield_now();
                continue;
            }
            let snapshot = ObservabilityCountersV1 {
                accepted: self.accepted.load(Ordering::Relaxed),
                written: self.written.load(Ordering::Relaxed),
                primary_dropped: self.primary_dropped.load(Ordering::Relaxed),
                fallback_dropped: self.fallback_dropped.load(Ordering::Relaxed),
                stopped_dropped: self.stopped_dropped.load(Ordering::Relaxed),
                degraded_dropped: self.degraded_dropped.load(Ordering::Relaxed),
                unflushed: self.unflushed.load(Ordering::Relaxed),
                queued: self.queued.load(Ordering::Relaxed),
                writing: self.writing.load(Ordering::Relaxed),
                flushing: self.flushing.load(Ordering::Relaxed),
                write_failures: self.write_failures.load(Ordering::Relaxed),
                flush_failures: self.flush_failures.load(Ordering::Relaxed),
                clock_errors: self.clock_errors.load(Ordering::Relaxed),
                clock_panics: self.clock_panics.load(Ordering::Relaxed),
                sink_failures: self.sink_failures.load(Ordering::Relaxed),
                counters_saturated: self.saturated.load(Ordering::Acquire),
            };
            if before == self.sequence.load(Ordering::Acquire) {
                return snapshot;
            }
        }
    }
}

struct Queue {
    primary: VecDeque<EventRecordV1>,
    fallback: VecDeque<EventRecordV1>,
    stopping: bool,
}
struct Lifecycle {
    frozen_report: Option<ObservabilityShutdownReportV1>,
}
#[cfg(test)]
type EmissionBarrierV1 = Arc<(Mutex<(bool, bool)>, Condvar)>;

struct Shared {
    queue: Mutex<Queue>,
    available: Condvar,
    counters: Counters,
    sink: Option<Arc<dyn LogSinkV1>>,
    clock: Arc<dyn ClockV1>,
    in_flight: AtomicU64,
    emitting: AtomicU64,
    degraded: bool,
    queue_saturated: AtomicBool,
    lifecycle: Mutex<Lifecycle>,
    admission: AtomicU64,
    #[cfg(test)]
    emission_barrier: Mutex<Option<EmissionBarrierV1>>,
}
struct EmissionGuardV1 {
    shared: Arc<Shared>,
}
impl Drop for EmissionGuardV1 {
    fn drop(&mut self) {
        self.shared.emitting.fetch_sub(1, Ordering::Release);
    }
}
#[derive(Clone)]
pub struct ObservabilityEmitterV1 {
    shared: Arc<Shared>,
}
pub struct ObservabilityV1 {
    emitter: ObservabilityEmitterV1,
    done: Option<mpsc::Receiver<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ObservabilityV1 {
    pub fn start(sink: Arc<dyn LogSinkV1>, clock: Arc<dyn ClockV1>) -> Self {
        Self::start_inner(Some(sink), clock)
    }
    pub fn start_degraded(clock: Arc<dyn ClockV1>) -> Self {
        let observability = Self::start_inner(None, clock);
        observability
            .emitter
            .shared
            .counters
            .add(&observability.emitter.shared.counters.sink_failures, 1);
        observability
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
            in_flight: AtomicU64::new(0),
            emitting: AtomicU64::new(0),
            admission: AtomicU64::new(ADMISSION_RUNNING_V1),
            queue_saturated: AtomicBool::new(false),
            degraded,
            lifecycle: Mutex::new(Lifecycle {
                frozen_report: None,
            }),
            #[cfg(test)]
            emission_barrier: Mutex::new(None),
        });
        let emitter = ObservabilityEmitterV1 {
            shared: Arc::clone(&shared),
        };
        let (done, worker) = if degraded {
            (None, None)
        } else {
            let (sender, receiver) = mpsc::channel();
            let worker_shared = Arc::clone(&shared);
            (
                Some(receiver),
                Some(thread::spawn(move || {
                    writer_loop(worker_shared);
                    let _ = sender.send(());
                })),
            )
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
    pub fn emit(&self, event: EventRecordV1) {
        self.emitter.emit(event);
    }
    pub fn counters(&self) -> ObservabilityCountersV1 {
        self.emitter.counters()
    }
    pub fn shutdown(mut self) -> ObservabilityShutdownReportV1 {
        self.stop_and_wait()
    }
    fn stop_and_wait(&mut self) -> ObservabilityShutdownReportV1 {
        if let Some(report) = self.emitter.frozen_report() {
            return report;
        }
        self.emitter.request_stop();
        let finalization = match self.done.take() {
            Some(done) => match done.recv_timeout(MAX_SHUTDOWN_FLUSH_V1) {
                Ok(()) => match self.worker.take().map(thread::JoinHandle::join) {
                    Some(Ok(())) | None => {
                        self.clear_gauges();
                        ObservabilityFinalizationV1::Completed
                    }
                    Some(Err(_)) => {
                        self.record_lost_and_clear();
                        ObservabilityFinalizationV1::Indeterminate
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.worker.take();
                    ObservabilityFinalizationV1::Detached
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.worker.take();
                    self.record_lost_and_clear();
                    ObservabilityFinalizationV1::Indeterminate
                }
            },
            None => ObservabilityFinalizationV1::Completed,
        };
        self.emitter.freeze_report(finalization)
    }
    fn clear_gauges(&self) {
        let counters = &self.emitter.shared.counters;
        counters.mutate(|counters| {
            counters.set_gauge_in_transaction(&counters.writing, 0);
            counters.set_gauge_in_transaction(&counters.flushing, 0);
        });
        self.emitter.shared.in_flight.store(0, Ordering::Release);
    }
    fn record_lost_and_clear(&self) {
        let mut queue = self
            .emitter
            .shared
            .queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let lost = u64::try_from(queue.primary.len().saturating_add(queue.fallback.len()))
            .unwrap_or(u64::MAX)
            .saturating_add(self.emitter.shared.in_flight.swap(0, Ordering::AcqRel));
        queue.primary.clear();
        queue.fallback.clear();
        let counters = &self.emitter.shared.counters;
        counters.mutate(|counters| {
            counters.set_gauge_in_transaction(&counters.queued, 0);
            counters.set_gauge_in_transaction(&counters.writing, 0);
            counters.set_gauge_in_transaction(&counters.flushing, 0);
            counters.add_in_transaction(&counters.unflushed, lost);
        });
    }
}
impl Drop for ObservabilityV1 {
    fn drop(&mut self) {
        self.stop_and_wait();
    }
}

impl ObservabilityEmitterV1 {
    /// Performs one non-blocking queue try-lock. Contention loses this event immediately.
    /// After the shutdown report is frozen, emits are ignored without mutating its counters.
    pub fn emit(&self, event: EventRecordV1) {
        self.shared.emitting.fetch_add(1, Ordering::Acquire);
        let _emission = EmissionGuardV1 {
            shared: Arc::clone(&self.shared),
        };
        match self.shared.admission.load(Ordering::Acquire) {
            ADMISSION_FROZEN_V1 => return,
            ADMISSION_STOPPING_V1 => {
                self.shared
                    .counters
                    .add(&self.shared.counters.stopped_dropped, 1);
                return;
            }
            ADMISSION_RUNNING_V1 => {}
            _ => unreachable!("admission state is closed"),
        }
        if self.shared.degraded {
            self.shared
                .counters
                .add(&self.shared.counters.degraded_dropped, 1);
            return;
        }
        #[cfg(test)]
        self.pause_after_state_check();
        let deadline = Instant::now() + MAX_EMIT_QUEUE_WAIT_V1;
        let queue_lock = loop {
            match self.shared.queue.try_lock() {
                Ok(queue) => break Ok(queue),
                Err(error @ TryLockError::Poisoned(_)) => break Err(error),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => thread::yield_now(),
                Err(error @ TryLockError::WouldBlock) => break Err(error),
            }
        };
        match queue_lock {
            Ok(mut queue) => {
                if queue.stopping
                    || self.shared.admission.load(Ordering::Acquire) == ADMISSION_STOPPING_V1
                {
                    // An emitter that was admitted before its initial state
                    // check is linearized by the freeze (which waits for the
                    // in-flight emitting count), so its stop-window drop is
                    // counted even when admission has already advanced to
                    // frozen: the shutdown snapshot is taken only after this
                    // emitter finishes. Checking the stop marker first keeps
                    // that accounting; the frozen branch below is reached only
                    // by a freeze that never went through a stop request.
                    self.shared
                        .counters
                        .add(&self.shared.counters.stopped_dropped, 1);
                } else if self.shared.admission.load(Ordering::Acquire) == ADMISSION_FROZEN_V1 {
                    // Frozen admission drops the event without counting.
                } else if queue.primary.len() < PRIMARY_CAPACITY_V1 {
                    queue.primary.push_back(event);
                    self.shared.counters.record_enqueue();
                    self.shared.available.notify_one();
                } else if event.outcome.uses_fallback()
                    && queue.fallback.len() < FALLBACK_CAPACITY_V1.saturating_sub(1)
                {
                    queue.fallback.push_back(event);
                    self.shared.counters.record_enqueue();
                    self.shared.available.notify_one();
                } else if queue.fallback.len() < FALLBACK_CAPACITY_V1
                    && self
                        .shared
                        .queue_saturated
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    queue.fallback.push_back(EventRecordV1::new(
                        EventOperationV1::QueueSaturation,
                        EventOutcomeV1::Saturated,
                    ));
                    self.shared.counters.record_enqueue();
                    if event.outcome.uses_fallback() {
                        self.shared
                            .counters
                            .add(&self.shared.counters.fallback_dropped, 1);
                    } else {
                        self.shared
                            .counters
                            .add(&self.shared.counters.primary_dropped, 1);
                    }
                    self.shared.available.notify_one();
                } else if event.outcome.uses_fallback() {
                    self.shared
                        .counters
                        .add(&self.shared.counters.fallback_dropped, 1);
                } else {
                    self.shared
                        .counters
                        .add(&self.shared.counters.primary_dropped, 1);
                }
            }
            Err(TryLockError::Poisoned(_)) | Err(TryLockError::WouldBlock) => {
                let counter = if event.outcome.uses_fallback() {
                    &self.shared.counters.fallback_dropped
                } else {
                    &self.shared.counters.primary_dropped
                };
                self.shared.counters.add(counter, 1);
            }
        }
    }
    pub fn counters(&self) -> ObservabilityCountersV1 {
        self.frozen_report().map_or_else(
            || self.shared.counters.snapshot(),
            |report| report.counters(),
        )
    }
    fn frozen_report(&self) -> Option<ObservabilityShutdownReportV1> {
        self.shared
            .lifecycle
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .frozen_report
    }
    fn freeze_report(
        &self,
        finalization: ObservabilityFinalizationV1,
    ) -> ObservabilityShutdownReportV1 {
        let mut lifecycle = self
            .shared
            .lifecycle
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if lifecycle.frozen_report.is_none() {
            self.shared
                .admission
                .store(ADMISSION_FROZEN_V1, Ordering::Release);
            while self.shared.emitting.load(Ordering::Acquire) != 0 {
                thread::yield_now();
            }
            lifecycle.frozen_report = Some(ObservabilityShutdownReportV1 {
                counters: self.shared.counters.snapshot(),
                finalization,
            });
        }
        lifecycle.frozen_report.expect("frozen report must be set")
    }
    fn request_stop(&self) {
        self.shared
            .admission
            .store(ADMISSION_STOPPING_V1, Ordering::Release);
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        queue.stopping = true;
        self.shared.available.notify_all();
    }
    #[cfg(test)]
    fn pause_after_state_check(&self) {
        let barrier = self
            .shared
            .emission_barrier
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        if let Some(barrier) = barrier {
            let (state, available) = &*barrier;
            let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
            state.0 = true;
            available.notify_all();
            while !state.1 {
                state = available
                    .wait(state)
                    .unwrap_or_else(|poison| poison.into_inner());
            }
        }
    }
}
impl Shared {
    fn account(&self, mutation: impl FnOnce(&Counters)) {
        mutation(&self.counters);
    }
}

fn writer_loop(shared: Arc<Shared>) {
    loop {
        let (event, dequeued) = {
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
            let (event, primary_relieved) = match queue.fallback.pop_front() {
                Some(event) => (Some(event), false),
                None => match queue.primary.pop_front() {
                    Some(event) => (Some(event), true),
                    None => (None, false),
                },
            };
            if primary_relieved {
                shared.queue_saturated.store(false, Ordering::Release);
            }
            (event, event.is_some())
        };
        if dequeued {
            shared.account(Counters::record_dequeue_for_write);
        }
        match event {
            Some(event) => {
                shared.in_flight.store(1, Ordering::Release);
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    shared.clock.unix_seconds()
                })) {
                    Ok(Ok(seconds)) => {
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            shared
                                .sink
                                .as_ref()
                                .expect("writer sink")
                                .write_event(&format_event(seconds, event))
                        })) {
                            Ok(Ok(())) => shared.account(Counters::record_write_success),
                            Ok(Err(_)) | Err(_) => shared.account(|counters| {
                                counters.decrement_gauge_in_transaction(&counters.writing);
                                counters.add_in_transaction(&counters.write_failures, 1);
                                counters.add_in_transaction(&counters.sink_failures, 1);
                                counters.add_in_transaction(&counters.unflushed, 1);
                            }),
                        }
                    }
                    Ok(Err(_)) => shared.account(|counters| {
                        counters.decrement_gauge_in_transaction(&counters.writing);
                        counters.add_in_transaction(&counters.clock_errors, 1);
                        counters.add_in_transaction(&counters.unflushed, 1);
                    }),
                    Err(_) => shared.account(|counters| {
                        counters.decrement_gauge_in_transaction(&counters.writing);
                        counters.add_in_transaction(&counters.clock_panics, 1);
                        counters.add_in_transaction(&counters.unflushed, 1);
                    }),
                }
                shared.in_flight.store(0, Ordering::Release);
            }
            None => {
                shared.account(|counters| {
                    counters.increment_gauge_in_transaction(&counters.flushing);
                });
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    shared.sink.as_ref().expect("writer sink").flush()
                }));
                shared.account(|counters| {
                    counters.decrement_gauge_in_transaction(&counters.flushing);
                    if !matches!(result, Ok(Ok(()))) {
                        counters.add_in_transaction(&counters.flush_failures, 1);
                        counters.add_in_transaction(&counters.sink_failures, 1);
                    }
                });
                return;
            }
        }
    }
}
fn format_event(seconds: u64, event: EventRecordV1) -> String {
    let formatted = format!(
        "ts={seconds} operation={} outcome={}\n",
        event.operation.name(),
        event.outcome.name()
    );
    debug_assert!(formatted.len() <= MAX_EVENT_BYTES_V1);
    formatted
}

/// Local file sink with fixed-size rotation. Only this sink's five numbered files are owned.
pub struct RotatingFileSinkV1 {
    path: PathBuf,
    state: Mutex<FileState>,
}
struct FileState {
    file: File,
    bytes: u64,
}
impl RotatingFileSinkV1 {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        ensure_private_log_parent(&path)?;
        retain_exact_rotations(&path)?;
        let file = open_private_log(&path)?;
        let bytes = file.metadata()?.len();
        Ok(Self {
            path,
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
            match fs::rename(from, to) {
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
        state.file = open_private_log(&self.path)?;
        state.bytes = 0;
        retain_exact_rotations(&self.path)?;
        Ok(())
    }
}

fn ensure_private_log_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "log path has no parent"))?;
    fs::create_dir_all(parent)?;
    let metadata = fs::symlink_metadata(parent)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != geteuid().as_raw()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "log parent is not an effective-user-owned real directory",
        ));
    }
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
}

fn open_private_log(path: &Path) -> io::Result<File> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.uid() != geteuid().as_raw())
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "log is not an effective-user-owned regular file",
        ));
    }
    let descriptor = open(
        path,
        OFlag::O_APPEND | OFlag::O_CLOEXEC | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_WRONLY,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .map_err(|error| io::Error::from_raw_os_error(error as i32))?;
    let file = File::from(descriptor);
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "opened log is not an effective-user-owned regular file",
        ));
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}
fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
fn retain_exact_rotations(path: &Path) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "log path has no file name"))?
        .to_string_lossy();
    let prefix = format!("{name}.");
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let candidate = entry.path();
        let suffix = entry
            .file_name()
            .to_string_lossy()
            .strip_prefix(&prefix)
            .map(str::to_owned);
        let Some(suffix) = suffix else { continue };
        if !suffix.is_empty()
            && suffix.bytes().all(|byte| byte.is_ascii_digit())
            && !matches!(suffix.as_str(), "1" | "2" | "3" | "4" | "5")
            && entry.metadata()?.is_file()
        {
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
impl fmt::Debug for ObservabilityEmitterV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObservabilityEmitterV1")
            .field("counters", &self.counters())
            .finish()
    }
}
impl fmt::Debug for ObservabilityV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObservabilityV1")
            .field("counters", &self.counters())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestSink;
    impl LogSinkV1 for TestSink {
        fn write_event(&self, _: &str) -> io::Result<()> {
            Ok(())
        }
        fn flush(&self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn cumulative_counters_saturate_without_wrapping_near_u64_max() {
        let counters = Counters::default();

        counters.record_enqueue();
        let queued = counters.snapshot();
        assert_eq!(
            queued.accepted,
            queued.written + queued.queued + queued.writing
        );

        counters.record_dequeue_for_write();
        let writing = counters.snapshot();
        assert_eq!(
            writing.accepted,
            writing.written + writing.queued + writing.writing
        );

        counters.record_write_success();
        let written = counters.snapshot();
        assert_eq!(
            written.accepted,
            written.written + written.queued + written.writing
        );

        counters.accepted.store(u64::MAX - 1, Ordering::Relaxed);
        counters.written.store(u64::MAX - 1, Ordering::Relaxed);

        counters.record_enqueue();
        let saturated_enqueue = counters.snapshot();
        assert_eq!(saturated_enqueue.accepted, u64::MAX);
        assert_eq!(saturated_enqueue.written, u64::MAX - 1);
        assert_eq!(saturated_enqueue.queued, 1);
        assert_eq!(saturated_enqueue.writing, 0);
        assert_eq!(
            saturated_enqueue.accepted,
            saturated_enqueue.written + saturated_enqueue.queued + saturated_enqueue.writing
        );
        assert!(saturated_enqueue.counters_saturated);

        counters.record_dequeue_for_write();
        let saturated_dequeue = counters.snapshot();
        assert_eq!(saturated_dequeue.accepted, u64::MAX);
        assert_eq!(saturated_dequeue.written, u64::MAX - 1);
        assert_eq!(saturated_dequeue.queued, 0);
        assert_eq!(saturated_dequeue.writing, 1);
        assert_eq!(
            saturated_dequeue.accepted,
            saturated_dequeue.written + saturated_dequeue.queued + saturated_dequeue.writing
        );
        assert!(saturated_dequeue.counters_saturated);

        counters.record_write_success();
        let saturated_written = counters.snapshot();
        assert_eq!(saturated_written.accepted, u64::MAX);
        assert_eq!(saturated_written.written, u64::MAX);
        assert_eq!(saturated_written.queued, 0);
        assert_eq!(saturated_written.writing, 0);
        assert_eq!(saturated_written.accepted, saturated_written.written);
        assert!(saturated_written.counters_saturated);
    }

    #[test]
    fn queue_contention_drops_without_admission_or_lifecycle_locking() {
        let observability = ObservabilityV1::start(Arc::new(TestSink), Arc::new(SystemClockV1));
        let emitter = observability.emitter();
        let queue = emitter
            .shared
            .queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());

        emitter.emit(EventRecordV1::new(
            EventOperationV1::DaemonStart,
            EventOutcomeV1::Succeeded,
        ));

        drop(queue);
        let report = observability.shutdown();
        let counters = report.counters();
        assert_eq!(counters.accepted, 0);
        assert_eq!(counters.primary_dropped, 1);
        assert_eq!(counters.written, 0);
    }
    #[test]
    fn shutdown_linearizes_an_emitter_after_its_initial_state_check() {
        let observability = ObservabilityV1::start(Arc::new(TestSink), Arc::new(SystemClockV1));
        let emitter = observability.emitter();
        let barrier = Arc::new((Mutex::new((false, false)), Condvar::new()));
        *emitter
            .shared
            .emission_barrier
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(Arc::clone(&barrier));
        let emitting = thread::spawn({
            let emitter = emitter.clone();
            move || {
                emitter.emit(EventRecordV1::new(
                    EventOperationV1::DaemonStart,
                    EventOutcomeV1::Succeeded,
                ))
            }
        });
        let (state, available) = &*barrier;
        let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
        while !state.0 {
            state = available
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
        let (started, shutdown_started) = mpsc::channel();
        let shutting_down = thread::spawn(move || {
            started.send(()).expect("shutdown start receiver");
            observability.shutdown()
        });
        shutdown_started
            .recv()
            .expect("shutdown thread must start before release");
        let deadline = Instant::now() + Duration::from_secs(5);
        // Shutdown may have advanced past stopping to frozen admission before
        // this thread observes it (the freeze then waits on the in-flight
        // emitter); closed admission means any non-running state.
        while emitter.shared.admission.load(Ordering::Acquire) == ADMISSION_RUNNING_V1 {
            assert!(
                Instant::now() < deadline,
                "shutdown must close admission before the emitter is released"
            );
            thread::yield_now();
        }
        state.1 = true;
        available.notify_all();
        drop(state);
        emitting.join().expect("emitter must not panic");
        let report = shutting_down.join().expect("shutdown must not panic");
        let counters = report.counters();
        assert_eq!(counters.accepted, 0);
        assert_eq!(counters.written, 0);
        assert_eq!(counters.stopped_dropped, 1);
        assert_eq!(
            counters.accepted,
            counters.written + counters.unflushed + counters.queued + counters.writing
        );
    }
}
