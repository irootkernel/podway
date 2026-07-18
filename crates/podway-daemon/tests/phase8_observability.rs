use std::{
    fs, io,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use podway_daemon::observability::{
    FALLBACK_CAPACITY_V1, ObservabilityFinalizationV1, PRIMARY_CAPACITY_V1, RETAINED_ROTATIONS_V1,
    ROTATION_BYTES_V1,
};
use podway_daemon::{
    ClockV1, EventCategoryV1, LogSinkV1, ObservabilityV1, RotatingFileSinkV1, SeverityV1,
};

struct FakeClock(AtomicU64);
impl FakeClock {
    fn new(seconds: u64) -> Self {
        Self(AtomicU64::new(seconds))
    }
}
impl ClockV1 for FakeClock {
    fn unix_seconds(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
struct FakeSink {
    events: Mutex<Vec<String>>,
    fail: bool,
}
impl LogSinkV1 for FakeSink {
    fn write_event(&self, event: &str) -> io::Result<()> {
        if self.fail {
            return Err(io::Error::other("fake sink failure"));
        }
        self.events.lock().unwrap().push(event.to_owned());
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}

struct BlockingSink {
    gate: Arc<(Mutex<bool>, Condvar)>,
}
impl LogSinkV1 for BlockingSink {
    fn write_event(&self, _: &str) -> io::Result<()> {
        let (locked, ready) = &*self.gate;
        let mut released = locked.lock().unwrap();
        while !*released {
            released = ready.wait(released).unwrap();
        }
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}
struct BlockingClock {
    gate: Arc<(Mutex<bool>, Condvar)>,
}
impl ClockV1 for BlockingClock {
    fn unix_seconds(&self) -> u64 {
        let (locked, ready) = &*self.gate;
        let mut released = locked.lock().unwrap();
        while !*released {
            released = ready.wait(released).unwrap();
        }
        1
    }
}
struct CountedBlockingSink {
    gate: Arc<(Mutex<(bool, bool)>, Condvar)>,
    writes: AtomicU64,
}
impl LogSinkV1 for CountedBlockingSink {
    fn write_event(&self, _: &str) -> io::Result<()> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        let (locked, ready) = &*self.gate;
        let mut state = locked.lock().unwrap();
        state.1 = true;
        ready.notify_all();
        while !state.0 {
            state = ready.wait(state).unwrap();
        }
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}
struct FirstWriteBlockingSink {
    gate: Arc<(Mutex<(bool, bool)>, Condvar)>,
    events: Mutex<Vec<String>>,
}
impl LogSinkV1 for FirstWriteBlockingSink {
    fn write_event(&self, event: &str) -> io::Result<()> {
        let (locked, ready) = &*self.gate;
        let mut state = locked.lock().unwrap();
        if !state.1 {
            state.1 = true;
            ready.notify_all();
            while !state.0 {
                state = ready.wait(state).unwrap();
            }
        }
        drop(state);
        self.events.lock().unwrap().push(event.to_owned());
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}
struct PanicSink;
impl LogSinkV1 for PanicSink {
    fn write_event(&self, _: &str) -> io::Result<()> {
        panic!("test worker panic");
    }
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}
struct BlockingFlushSink {
    gate: Arc<(Mutex<bool>, Condvar)>,
}
impl LogSinkV1 for BlockingFlushSink {
    fn write_event(&self, _: &str) -> io::Result<()> {
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        let (locked, ready) = &*self.gate;
        let mut released = locked.lock().unwrap();
        while !*released {
            released = ready.wait(released).unwrap();
        }
        Ok(())
    }
}
struct FlushFailSink;
impl LogSinkV1 for FlushFailSink {
    fn write_event(&self, _: &str) -> io::Result<()> {
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        Err(io::Error::other("fake final flush failure"))
    }
}

fn fake_filesystem_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "podway-observability-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path.join("podwayd.log")
}

#[test]
fn events_are_categorical_and_redacted() {
    let clock = Arc::new(FakeClock::new(42));
    let sink = Arc::new(FakeSink::default());
    let observability = ObservabilityV1::start(sink.clone(), clock);
    observability.emit(EventCategoryV1::Admission, SeverityV1::Info);
    observability.shutdown();
    let events = sink.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events.as_slice(),
        ["ts=42 severity=INFO category=admission detail=[redacted]\n"]
    );
    assert!(!events[0].contains("request-value"));
}

#[test]
fn saturated_queues_count_drops_without_blocking_domain_work() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let sink = Arc::new(BlockingSink { gate: gate.clone() });
    let observability = ObservabilityV1::start(sink, Arc::new(FakeClock::new(1)));
    let started = Instant::now();
    for _ in 0..PRIMARY_CAPACITY_V1 + FALLBACK_CAPACITY_V1 + 8 {
        observability.emit(
            EventCategoryV1::TerminalOrRequeueOrSaturation,
            SeverityV1::Error,
        );
    }
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "producer admission must not wait for stderr or the blocked writer"
    );
    let counters = observability.counters();
    assert!(counters.primary_dropped > 0);
    assert!(counters.fallback_dropped > 0);
    assert_eq!(
        counters.stderr_dropped, 0,
        "producer has no stderr fallback"
    );
    let (locked, ready) = &*gate;
    *locked.lock().unwrap() = true;
    ready.notify_all();
    observability.shutdown();
}

#[test]
fn sink_failures_are_counted_and_never_returned_to_producer() {
    let observability = ObservabilityV1::start(
        Arc::new(FakeSink {
            events: Mutex::new(Vec::new()),
            fail: true,
        }),
        Arc::new(FakeClock::new(1)),
    );
    observability.emit(EventCategoryV1::ServiceOutcome, SeverityV1::Error);
    let counters = observability.shutdown();
    assert!(counters.sink_failures >= 1);
}
#[test]
fn sink_open_degradation_is_typed_and_counts_every_event() {
    let observability = ObservabilityV1::start_degraded(Arc::new(FakeClock::new(1)));
    let emitter = observability.emitter();
    emitter.emit(EventCategoryV1::Lifecycle, SeverityV1::Warn);
    emitter.emit(EventCategoryV1::ServiceOutcome, SeverityV1::Error);
    let counters = observability.shutdown();
    assert_eq!(counters.bootstrap_failures, 1);
    assert_eq!(counters.degraded_dropped, 2);
    assert_eq!(counters.sink_failures, 2);
}

#[test]
fn worker_panic_clears_gauges_and_counts_the_lost_event() {
    let observability = ObservabilityV1::start(Arc::new(PanicSink), Arc::new(FakeClock::new(1)));
    observability.emit(EventCategoryV1::ServiceOutcome, SeverityV1::Error);
    let counters = observability.shutdown();
    assert_eq!(counters.worker_panics, 1);
    assert_eq!(counters.worker_disconnects, 1);
    assert_eq!(counters.worker_join_failures, 1);
    assert_eq!(counters.unflushed, 1);
    assert_eq!(counters.writing, 0);
    assert_eq!(counters.queued, 0);
}

#[test]
fn rotation_retains_five_and_fake_clock_prunes_stale_files() {
    let path = fake_filesystem_path("rotation");
    let sink = RotatingFileSinkV1::open(&path, Arc::new(FakeClock::new(0))).unwrap();
    for index in 1..=RETAINED_ROTATIONS_V1 + 1 {
        fs::write(path.with_extension(format!("log.{index}")), "stale").unwrap();
    }
    let event = "x".repeat(8 * 1024);
    for _ in 0..=(ROTATION_BYTES_V1 as usize / event.len()) {
        sink.write_event(&event).unwrap();
    }
    assert!(path.exists());
    for index in 1..=RETAINED_ROTATIONS_V1 {
        assert!(path.with_extension(format!("log.{index}")).exists());
    }
    assert!(
        !path
            .with_extension(format!("log.{}", RETAINED_ROTATIONS_V1 + 1))
            .exists()
    );

    let stale_path = fake_filesystem_path("retention");
    fs::write(stale_path.with_extension("log.1"), "stale").unwrap();
    let _stale_sink =
        RotatingFileSinkV1::open(&stale_path, Arc::new(FakeClock::new(u64::MAX))).unwrap();
    assert!(
        !stale_path.with_extension("log.1").exists(),
        "retention must prune stale rotations during sink open"
    );
    let _ = fs::remove_dir_all(path.parent().unwrap());
    let _ = fs::remove_dir_all(stale_path.parent().unwrap());
}

#[test]
fn shutdown_flushes_queued_events() {
    let sink = Arc::new(FakeSink::default());
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FakeClock::new(9)));
    observability.emit(EventCategoryV1::Lifecycle, SeverityV1::Warn);
    let counters = observability.shutdown();
    assert_eq!(counters.unflushed, 0);
    assert_eq!(sink.events.lock().unwrap().len(), 1);
}
#[test]
fn blocked_final_flush_is_bounded_and_classified() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let observability = ObservabilityV1::start(
        Arc::new(BlockingFlushSink { gate: gate.clone() }),
        Arc::new(FakeClock::new(1)),
    );
    observability.emit(EventCategoryV1::Lifecycle, SeverityV1::Info);
    let started = Instant::now();
    let report = observability.shutdown_report();
    let counters = report.counters();
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(report.finalization(), ObservabilityFinalizationV1::Detached);
    assert_eq!(counters.shutdown_timeouts, 1);
    assert_eq!(counters.flushing, 1);
    let (locked, ready) = &*gate;
    *locked.lock().unwrap() = true;
    ready.notify_all();
}

#[test]
fn cloneable_emitter_keeps_runtime_observation_separate_from_owner_lifecycle() {
    let sink = Arc::new(FakeSink::default());
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FakeClock::new(0)));
    let runtime_emitter = observability.emitter();
    runtime_emitter.emit(EventCategoryV1::StaleEvidence, SeverityV1::Debug);
    observability.emit(EventCategoryV1::Lifecycle, SeverityV1::Info);
    let counters = observability.shutdown();
    assert_eq!(counters.queued, 0);
    assert_eq!(sink.events.lock().unwrap().len(), 2);
}
#[test]
fn producer_does_not_call_a_blocking_clock() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let observability = ObservabilityV1::start(
        Arc::new(FakeSink::default()),
        Arc::new(BlockingClock { gate: gate.clone() }),
    );
    let started = Instant::now();
    observability.emit(EventCategoryV1::Lifecycle, SeverityV1::Info);
    assert!(started.elapsed() < Duration::from_millis(100));
    let (locked, ready) = &*gate;
    *locked.lock().unwrap() = true;
    ready.notify_all();
    observability.shutdown();
}

#[test]
fn dropping_owner_accounts_for_blocked_and_queued_events() {
    let gate = Arc::new((Mutex::new((false, false)), Condvar::new()));
    let sink = Arc::new(CountedBlockingSink {
        gate: gate.clone(),
        writes: AtomicU64::new(0),
    });
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FakeClock::new(1)));
    let emitter = observability.emitter();
    emitter.emit(EventCategoryV1::Lifecycle, SeverityV1::Info);
    emitter.emit(EventCategoryV1::Scheduler, SeverityV1::Info);
    let (locked, ready) = &*gate;
    let mut state = locked.lock().unwrap();
    while !state.1 {
        state = ready.wait(state).unwrap();
    }
    drop(state);
    let started = Instant::now();
    drop(observability);
    assert!(started.elapsed() >= Duration::from_secs(1));
    let counters = emitter.counters();
    assert_eq!(counters.shutdown_timeouts, 1);
    assert_eq!(counters.unflushed, 0);
    assert_eq!(counters.writing, 1);
    assert_eq!(sink.writes.load(Ordering::Relaxed), 1);
    let mut state = locked.lock().unwrap();
    state.0 = true;
    ready.notify_all();
}

#[test]
fn fallback_entries_precede_primary_backlog_after_blocked_writer() {
    let gate = Arc::new((Mutex::new((false, false)), Condvar::new()));
    let sink = Arc::new(FirstWriteBlockingSink {
        gate: gate.clone(),
        events: Mutex::new(Vec::new()),
    });
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FakeClock::new(1)));
    observability.emit(EventCategoryV1::Lifecycle, SeverityV1::Info);
    let (locked, ready) = &*gate;
    let mut state = locked.lock().unwrap();
    while !state.1 {
        state = ready.wait(state).unwrap();
    }
    drop(state);
    for _ in 0..PRIMARY_CAPACITY_V1 {
        observability.emit(EventCategoryV1::Scheduler, SeverityV1::Info);
    }
    observability.emit(EventCategoryV1::ServiceOutcome, SeverityV1::Error);
    let mut state = locked.lock().unwrap();
    state.0 = true;
    ready.notify_all();
    drop(state);
    observability.shutdown();
    let events = sink.events.lock().unwrap();
    assert!(events[0].contains("severity=INFO category=lifecycle"));
    assert!(events[1].contains("severity=ERROR category=service_outcome"));
    assert!(events[2].contains("severity=INFO category=scheduler"));
}
#[test]
fn write_and_final_flush_losses_are_counted_in_completed_report() {
    let write_observability = ObservabilityV1::start(
        Arc::new(FakeSink {
            events: Mutex::new(Vec::new()),
            fail: true,
        }),
        Arc::new(FakeClock::new(1)),
    );
    write_observability.emit(EventCategoryV1::ServiceOutcome, SeverityV1::Error);
    let write_report = write_observability.shutdown_report();
    assert_eq!(
        write_report.finalization(),
        ObservabilityFinalizationV1::Completed
    );
    assert_eq!(write_report.counters().unflushed, 1);

    let flush_observability =
        ObservabilityV1::start(Arc::new(FlushFailSink), Arc::new(FakeClock::new(1)));
    let flush_report = flush_observability.shutdown_report();
    assert_eq!(
        flush_report.finalization(),
        ObservabilityFinalizationV1::Completed
    );
    assert_eq!(flush_report.counters().final_flush_losses, 1);
    assert_eq!(flush_report.counters().flush_failures, 1);
}

#[test]
fn frozen_timeout_report_preserves_indeterminate_work_snapshot() {
    let gate = Arc::new((Mutex::new((false, false)), Condvar::new()));
    let sink = Arc::new(CountedBlockingSink {
        gate: gate.clone(),
        writes: AtomicU64::new(0),
    });
    let observability = ObservabilityV1::start(sink, Arc::new(FakeClock::new(1)));
    observability.emit(EventCategoryV1::Lifecycle, SeverityV1::Info);
    observability.emit(EventCategoryV1::Scheduler, SeverityV1::Info);
    let (locked, ready) = &*gate;
    let mut state = locked.lock().unwrap();
    while !state.1 {
        state = ready.wait(state).unwrap();
    }
    drop(state);
    let report = observability.shutdown_report();
    assert_eq!(report.finalization(), ObservabilityFinalizationV1::Detached);
    assert_eq!(report.counters().unflushed, 0);
    assert_eq!(report.counters().writing, 1);
    assert_eq!(report.counters().queued, 1);
    let mut state = locked.lock().unwrap();
    state.0 = true;
    ready.notify_all();
}

#[test]
fn retention_prunes_arbitrary_numeric_rotations_but_never_active_destination() {
    let path = fake_filesystem_path("arbitrary-retention");
    fs::write(&path, "active").unwrap();
    let arbitrary = path.with_extension("log.42");
    fs::write(&arbitrary, "stale").unwrap();
    let _sink = RotatingFileSinkV1::open(&path, Arc::new(FakeClock::new(u64::MAX))).unwrap();
    assert!(path.exists());
    assert!(!arbitrary.exists());
    let _ = fs::remove_dir_all(path.parent().unwrap());
}
