//! Bounded, non-authoritative daemon diagnostics.
//!
//! Event records are closed, typed, and carry only bounded correlation identifiers.

use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    fmt,
    fs::File,
    io::{self, Write},
    os::{
        fd::OwnedFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::Path,
    sync::{
        Arc, Condvar, Mutex, TryLockError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use nix::{
    dir::Dir,
    errno::Errno,
    fcntl::{AtFlags, OFlag, open, openat, renameat},
    sys::stat::{Mode, SFlag, fchmod, fstat, fstatat, mkdirat},
    unistd::{UnlinkatFlags, geteuid, unlinkat},
};
use serde::Serialize;

pub const MAX_EVENT_BYTES_V1: usize = 8 * 1024;
pub const PRIMARY_CAPACITY_V1: usize = 4096;
pub const FALLBACK_CAPACITY_V1: usize = 256;
pub const MAX_WRITER_CYCLE_V1: Duration = Duration::from_millis(50);
pub const MAX_SHUTDOWN_FLUSH_V1: Duration = Duration::from_secs(2);
const MAX_EMIT_QUEUE_WAIT_V1: Duration = Duration::from_millis(5);
pub const ROTATION_BYTES_V1: u64 = 1024 * 1024;
pub const RETAINED_FILES_V1: usize = 10;
pub const RETAINED_ROTATIONS_V1: usize = RETAINED_FILES_V1 - 1;
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventRecordV1 {
    operation: EventOperationV1,
    outcome: EventOutcomeV1,
    command: Option<String>,
    workspace_uuid: Option<String>,
    session_id: Option<String>,
    request_id: Option<String>,
    job_id: Option<String>,
    stage: Option<&'static str>,
    error_kind: Option<&'static str>,
    integrity_check: Option<&'static str>,
    reason: Option<&'static str>,
    diagnostic_id: Option<String>,
}
impl EventRecordV1 {
    pub const fn new(operation: EventOperationV1, outcome: EventOutcomeV1) -> Self {
        Self {
            operation,
            outcome,
            command: None,
            workspace_uuid: None,
            session_id: None,
            request_id: None,
            job_id: None,
            stage: None,
            error_kind: None,
            integrity_check: None,
            reason: None,
            diagnostic_id: None,
        }
    }
    pub const fn operation(&self) -> EventOperationV1 {
        self.operation
    }
    pub const fn outcome(&self) -> EventOutcomeV1 {
        self.outcome
    }

    pub fn with_command(mut self, command: impl ToString) -> Self {
        self.command = Some(command.to_string());
        self
    }

    pub fn with_workspace_uuid(mut self, workspace_uuid: impl ToString) -> Self {
        self.workspace_uuid = Some(workspace_uuid.to_string());
        self
    }

    pub fn with_session_id(mut self, session_id: impl ToString) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }

    pub fn with_request_id(mut self, request_id: impl ToString) -> Self {
        self.request_id = Some(request_id.to_string());
        self
    }

    pub fn with_job_id(mut self, job_id: impl ToString) -> Self {
        self.job_id = Some(job_id.to_string());
        self
    }

    pub fn with_failure(
        mut self,
        stage: &'static str,
        error_kind: &'static str,
        integrity_check: Option<&'static str>,
        reason: Option<&'static str>,
    ) -> Self {
        self.stage = Some(stage);
        self.error_kind = Some(error_kind);
        self.integrity_check = integrity_check;
        self.reason = reason;
        self
    }

    pub fn with_diagnostic_id(mut self, diagnostic_id: impl ToString) -> Self {
        self.diagnostic_id = Some(diagnostic_id.to_string());
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockErrorV1 {
    BeforeUnixEpoch,
}

/// Wall-clock boundary. A clock failure is a value; only a clock panic is caught as a panic.
pub trait ClockV1: Send + Sync + 'static {
    fn unix_seconds(&self) -> Result<u64, ClockErrorV1>;

    fn unix_millis(&self) -> Result<u64, ClockErrorV1> {
        self.unix_seconds()
            .map(|seconds| seconds.saturating_mul(1000))
    }
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

    fn unix_millis(&self) -> Result<u64, ClockErrorV1> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
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
    primary: VecDeque<(u64, EventRecordV1)>,
    fallback: VecDeque<(u64, EventRecordV1)>,
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
    daemon_id: String,
    next_event_sequence: AtomicU64,
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
        Self::start_with_daemon_id(sink, clock, "unknown")
    }
    pub fn start_with_daemon_id(
        sink: Arc<dyn LogSinkV1>,
        clock: Arc<dyn ClockV1>,
        daemon_id: impl Into<String>,
    ) -> Self {
        Self::start_inner(Some(sink), clock, daemon_id.into())
    }
    pub fn start_degraded(clock: Arc<dyn ClockV1>) -> Self {
        let observability = Self::start_inner(None, clock, "unknown".to_owned());
        observability
            .emitter
            .shared
            .counters
            .add(&observability.emitter.shared.counters.sink_failures, 1);
        observability
    }
    fn start_inner(
        sink: Option<Arc<dyn LogSinkV1>>,
        clock: Arc<dyn ClockV1>,
        daemon_id: String,
    ) -> Self {
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
            daemon_id,
            next_event_sequence: AtomicU64::new(1),
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
        let sequence = self
            .shared
            .next_event_sequence
            .fetch_add(1, Ordering::Relaxed);
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
                    queue.primary.push_back((sequence, event));
                    self.shared.counters.record_enqueue();
                    self.shared.available.notify_one();
                } else if event.outcome.uses_fallback()
                    && queue.fallback.len() < FALLBACK_CAPACITY_V1.saturating_sub(1)
                {
                    queue.fallback.push_back((sequence, event));
                    self.shared.counters.record_enqueue();
                    self.shared.available.notify_one();
                } else if queue.fallback.len() < FALLBACK_CAPACITY_V1
                    && self
                        .shared
                        .queue_saturated
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    let saturation_sequence = self
                        .shared
                        .next_event_sequence
                        .fetch_add(1, Ordering::Relaxed);
                    queue.fallback.push_back((
                        saturation_sequence,
                        EventRecordV1::new(
                            EventOperationV1::QueueSaturation,
                            EventOutcomeV1::Saturated,
                        ),
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
            let dequeued = event.is_some();
            (event, dequeued)
        };
        if dequeued {
            shared.account(Counters::record_dequeue_for_write);
        }
        match event {
            Some((sequence, event)) => {
                shared.in_flight.store(1, Ordering::Release);
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    shared.clock.unix_millis()
                })) {
                    Ok(Ok(timestamp_millis)) => {
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            shared
                                .sink
                                .as_ref()
                                .expect("writer sink")
                                .write_event(&format_event(
                                    timestamp_millis,
                                    &shared.daemon_id,
                                    sequence,
                                    &event,
                                ))
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
#[derive(Serialize)]
struct FormattedEventV1<'a> {
    schema: &'static str,
    ts: u64,
    daemon_id: &'a str,
    seq: u64,
    operation: &'static str,
    outcome: &'static str,
    command: Option<&'a str>,
    workspace_uuid: Option<&'a str>,
    session_id: Option<&'a str>,
    request_id: Option<&'a str>,
    job_id: Option<&'a str>,
    stage: Option<&'static str>,
    error_kind: Option<&'static str>,
    integrity_check: Option<&'static str>,
    reason: Option<&'static str>,
    diagnostic_id: Option<&'a str>,
}

fn format_event(
    timestamp_millis: u64,
    daemon_id: &str,
    sequence: u64,
    event: &EventRecordV1,
) -> String {
    let mut formatted = serde_json::to_string(&FormattedEventV1 {
        schema: "podway.daemon-log/v1",
        ts: timestamp_millis,
        daemon_id,
        seq: sequence,
        operation: event.operation.name(),
        outcome: event.outcome.name(),
        command: event.command.as_deref(),
        workspace_uuid: event.workspace_uuid.as_deref(),
        session_id: event.session_id.as_deref(),
        request_id: event.request_id.as_deref(),
        job_id: event.job_id.as_deref(),
        stage: event.stage,
        error_kind: event.error_kind,
        integrity_check: event.integrity_check,
        reason: event.reason,
        diagnostic_id: event.diagnostic_id.as_deref(),
    })
    .expect("closed daemon log event must serialize");
    formatted.push('\n');
    debug_assert!(formatted.len() <= MAX_EVENT_BYTES_V1);
    formatted
}

/// Local file sink with fixed-size rotation. The active file counts toward total retention.
pub struct RotatingFileSinkV1 {
    directory: OwnedFd,
    name: OsString,
    state: Mutex<FileState>,
}
struct FileState {
    file: File,
    bytes: u64,
}
impl RotatingFileSinkV1 {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "log path has no parent"))?;
        let name = path
            .file_name()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "log path has no file name")
            })?
            .to_os_string();
        let directory = open_private_log_directory(parent)?;
        retain_exact_rotations(&directory, &name, RETAINED_ROTATIONS_V1)?;
        let file = open_private_log(&directory, &name)?;
        let bytes = file.metadata()?.len();
        Ok(Self {
            directory,
            name,
            state: Mutex::new(FileState { file, bytes }),
        })
    }
    fn rotate(&self, state: &mut FileState) -> io::Result<()> {
        state.file.flush()?;
        remove_if_present(
            &self.directory,
            &rotated_log_name(&self.name, RETAINED_ROTATIONS_V1 + 1),
        )?;
        remove_if_present(
            &self.directory,
            &rotated_log_name(&self.name, RETAINED_ROTATIONS_V1),
        )?;
        for index in (1..RETAINED_ROTATIONS_V1).rev() {
            let from = rotated_log_name(&self.name, index);
            let to = rotated_log_name(&self.name, index + 1);
            match renameat(
                &self.directory,
                from.as_os_str(),
                &self.directory,
                to.as_os_str(),
            ) {
                Ok(()) => {}
                Err(Errno::ENOENT) => {}
                Err(error) => return Err(nix_io_error(error)),
            }
        }
        let first = rotated_log_name(&self.name, 1);
        remove_if_present(&self.directory, &first)?;
        match renameat(
            &self.directory,
            self.name.as_os_str(),
            &self.directory,
            first.as_os_str(),
        ) {
            Ok(()) => {}
            Err(Errno::ENOENT) => {}
            Err(error) => return Err(nix_io_error(error)),
        }
        state.file = open_private_log(&self.directory, &self.name)?;
        state.bytes = 0;
        retain_exact_rotations(&self.directory, &self.name, RETAINED_ROTATIONS_V1)?;
        Ok(())
    }
}

fn open_private_log_directory(path: &Path) -> io::Result<OwnedFd> {
    let normalized;
    let path = if let Ok(suffix) = path.strip_prefix("/var") {
        normalized = Path::new("/private/var").join(suffix);
        normalized.as_path()
    } else if let Ok(suffix) = path.strip_prefix("/tmp") {
        normalized = Path::new("/private/tmp").join(suffix);
        normalized.as_path()
    } else {
        path
    };
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "log parent must be absolute",
        ));
    }
    let components = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::RootDir => None,
            std::path::Component::Normal(component) => Some(Ok(component)),
            _ => Some(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log parent must be normalized",
            ))),
        })
        .collect::<io::Result<Vec<_>>>()?;
    if components.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "log parent must be below the filesystem root",
        ));
    }
    let mut directory = open(
        "/",
        OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .map_err(nix_io_error)?;
    for component in components {
        match mkdirat(&directory, component, Mode::from_bits_truncate(0o700)) {
            Ok(()) | Err(Errno::EEXIST) => {}
            Err(error) => return Err(nix_io_error(error)),
        }
        directory = openat(
            &directory,
            component,
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map_err(nix_io_error)?;
    }
    let metadata = fstat(&directory).map_err(nix_io_error)?;
    if SFlag::from_bits_truncate(metadata.st_mode) & SFlag::S_IFMT != SFlag::S_IFDIR
        || metadata.st_uid != geteuid().as_raw()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "log parent is not an effective-user-owned real directory",
        ));
    }
    fchmod(&directory, Mode::from_bits_truncate(0o700)).map_err(nix_io_error)?;
    Ok(directory)
}

fn open_private_log(directory: &OwnedFd, name: &OsStr) -> io::Result<File> {
    let descriptor = openat(
        directory,
        name,
        OFlag::O_APPEND | OFlag::O_CLOEXEC | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_WRONLY,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .map_err(nix_io_error)?;
    let file = File::from(descriptor);
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "opened log is not an effective-user-owned regular file",
        ));
    }
    fchmod(&file, Mode::from_bits_truncate(0o600)).map_err(nix_io_error)?;
    Ok(file)
}

fn rotated_log_name(name: &OsStr, index: usize) -> OsString {
    let mut rotated = name.to_os_string();
    rotated.push(format!(".{index}"));
    rotated
}

fn remove_if_present(directory: &OwnedFd, name: &OsStr) -> io::Result<()> {
    match unlinkat(directory, name, UnlinkatFlags::NoRemoveDir) {
        Ok(()) => Ok(()),
        Err(Errno::ENOENT) => Ok(()),
        Err(error) => Err(nix_io_error(error)),
    }
}

fn retain_exact_rotations(
    directory: &OwnedFd,
    name: &OsStr,
    retained_rotations: usize,
) -> io::Result<()> {
    let name = name.to_string_lossy();
    let prefix = format!("{name}.");
    let mut entries = Dir::openat(
        directory,
        ".",
        OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .map_err(nix_io_error)?;
    for entry in entries.iter() {
        let entry = entry.map_err(nix_io_error)?;
        let candidate = OsStr::from_bytes(entry.file_name().to_bytes());
        let suffix = entry
            .file_name()
            .to_string_lossy()
            .strip_prefix(&prefix)
            .map(str::to_owned);
        let Some(suffix) = suffix else { continue };
        if !suffix.is_empty()
            && suffix.bytes().all(|byte| byte.is_ascii_digit())
            && suffix.parse::<usize>().is_ok_and(|index| {
                index == 0 || index > retained_rotations || suffix.starts_with('0')
            })
        {
            let metadata = fstatat(directory, candidate, AtFlags::AT_SYMLINK_NOFOLLOW)
                .map_err(nix_io_error)?;
            if SFlag::from_bits_truncate(metadata.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG
                && metadata.st_uid == geteuid().as_raw()
            {
                remove_if_present(directory, candidate)?;
            }
        }
    }
    Ok(())
}

fn nix_io_error(error: Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
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
