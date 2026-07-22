use std::{
    fs, io,
    path::PathBuf,
    sync::{
        Arc, Barrier, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use podway_daemon::observability::{
    FALLBACK_CAPACITY_V1, ObservabilityFinalizationV1, PRIMARY_CAPACITY_V1, RETAINED_ROTATIONS_V1,
    ROTATION_BYTES_V1,
};
use podway_daemon::{
    ClockErrorV1, ClockV1, EventOperationV1, EventOutcomeV1, EventRecordV1, LogSinkV1,
    ObservabilityV1, RotatingFileSinkV1,
};

struct FixedClock(u64);
impl ClockV1 for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ClockErrorV1> {
        Ok(self.0)
    }
}
struct ErrorClock;
impl ClockV1 for ErrorClock {
    fn unix_seconds(&self) -> Result<u64, ClockErrorV1> {
        Err(ClockErrorV1::BeforeUnixEpoch)
    }
}
struct PanicClock;
impl ClockV1 for PanicClock {
    fn unix_seconds(&self) -> Result<u64, ClockErrorV1> {
        panic!("clock panic")
    }
}

#[derive(Default)]
struct Sink {
    events: Mutex<Vec<String>>,
}
impl LogSinkV1 for Sink {
    fn write_event(&self, event: &str) -> io::Result<()> {
        self.events.lock().unwrap().push(event.to_owned());
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}
struct WriteErrorSink {
    events: Mutex<Vec<String>>,
}
impl LogSinkV1 for WriteErrorSink {
    fn write_event(&self, event: &str) -> io::Result<()> {
        self.events.lock().unwrap().push(event.to_owned());
        Err(io::Error::other("write error"))
    }
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}
struct PanicWriteSink;
impl LogSinkV1 for PanicWriteSink {
    fn write_event(&self, _: &str) -> io::Result<()> {
        panic!("write panic")
    }
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}
struct PanicFlushSink;
impl LogSinkV1 for PanicFlushSink {
    fn write_event(&self, _: &str) -> io::Result<()> {
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        panic!("flush panic")
    }
}
struct PermitSink {
    permits: Arc<(Mutex<usize>, Condvar)>,
    events: Arc<Mutex<Vec<String>>>,
}
impl LogSinkV1 for PermitSink {
    fn write_event(&self, event: &str) -> io::Result<()> {
        let (lock, ready) = &*self.permits;
        let mut permits = lock.lock().unwrap();
        while *permits == 0 {
            permits = ready.wait(permits).unwrap();
        }
        *permits -= 1;
        self.events.lock().unwrap().push(event.to_owned());
        Ok(())
    }
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}

const ADMISSION_OK: EventRecordV1 =
    EventRecordV1::new(EventOperationV1::JobAdmission, EventOutcomeV1::Succeeded);
const SERVICE_FAILED: EventRecordV1 = EventRecordV1::new(
    EventOperationV1::TransportServiceRequest,
    EventOutcomeV1::Failed,
);

#[test]
fn event_records_are_closed_bounded_and_private() {
    let sink = Arc::new(Sink::default());
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FixedClock(42)));
    observability.emit(ADMISSION_OK);
    observability.shutdown();
    assert_eq!(
        sink.events.lock().unwrap().as_slice(),
        ["ts=42 operation=job_admission outcome=succeeded\n"]
    );
}

#[test]
fn saturated_outcome_is_stably_serialized_and_uses_priority_queue() {
    let sink = Arc::new(Sink::default());
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FixedClock(7)));
    observability.emit(EventRecordV1::new(
        EventOperationV1::QueueSaturation,
        EventOutcomeV1::Saturated,
    ));
    observability.shutdown();
    assert_eq!(
        sink.events.lock().unwrap().as_slice(),
        ["ts=7 operation=queue_saturation outcome=saturated\n"]
    );
}

#[test]
fn clock_errors_and_panics_are_counted_separately() {
    let error = ObservabilityV1::start(Arc::new(Sink::default()), Arc::new(ErrorClock));
    error.emit(ADMISSION_OK);
    let error = error.shutdown().counters();
    assert_eq!(
        (error.clock_errors, error.clock_panics, error.unflushed),
        (1, 0, 1)
    );

    let panic = ObservabilityV1::start(Arc::new(Sink::default()), Arc::new(PanicClock));
    panic.emit(ADMISSION_OK);
    let panic = panic.shutdown().counters();
    assert_eq!(
        (panic.clock_errors, panic.clock_panics, panic.unflushed),
        (0, 1, 1)
    );
}
#[test]
fn sink_write_errors_preserve_output_and_are_accounted() {
    let sink = Arc::new(WriteErrorSink {
        events: Mutex::new(Vec::new()),
    });
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FixedClock(42)));
    observability.emit(ADMISSION_OK);
    let report = observability.shutdown();
    let counters = report.counters();
    assert_eq!(
        sink.events.lock().unwrap().as_slice(),
        ["ts=42 operation=job_admission outcome=succeeded\n"]
    );
    assert_eq!(
        (
            counters.accepted,
            counters.written,
            counters.write_failures,
            counters.sink_failures,
            counters.unflushed
        ),
        (1, 0, 1, 1, 1)
    );
}
#[test]
fn sink_write_panics_are_accounted_without_panicking_shutdown() {
    let observability = ObservabilityV1::start(Arc::new(PanicWriteSink), Arc::new(FixedClock(1)));
    observability.emit(ADMISSION_OK);
    let report = observability.shutdown();
    assert_eq!(
        report.finalization(),
        ObservabilityFinalizationV1::Completed
    );
    assert_eq!(
        (
            report.counters().accepted,
            report.counters().written,
            report.counters().write_failures,
            report.counters().sink_failures,
            report.counters().unflushed,
        ),
        (1, 0, 1, 1, 1)
    );
}

#[test]
fn snapshots_and_frozen_reports_are_coherent() {
    let sink = Arc::new(Sink::default());
    let observability = ObservabilityV1::start(sink, Arc::new(FixedClock(1)));
    let emitter = observability.emitter();
    for _ in 0..64 {
        emitter.emit(ADMISSION_OK);
        let counters = emitter.counters();
        assert_eq!(
            counters.accepted,
            counters.written + counters.unflushed + counters.queued + counters.writing
        );
    }
    let report = observability.shutdown();
    assert_eq!(
        report.finalization(),
        ObservabilityFinalizationV1::Completed
    );
    let frozen = report.counters();
    emitter.emit(SERVICE_FAILED);
    assert_eq!(emitter.counters(), frozen);
}
#[test]
fn post_shutdown_emits_are_rejected_without_mutating_frozen_counters() {
    let observability = ObservabilityV1::start(Arc::new(Sink::default()), Arc::new(FixedClock(1)));
    let emitter = observability.emitter();
    let frozen = observability.shutdown().counters();

    emitter.emit(ADMISSION_OK);
    emitter.emit(SERVICE_FAILED);

    assert_eq!(emitter.counters(), frozen);
    assert_eq!(
        (
            frozen.primary_dropped,
            frozen.fallback_dropped,
            frozen.stopped_dropped,
        ),
        (0, 0, 0)
    );
}

#[test]
fn failure_only_saturation_storm_reserves_marker_capacity_and_accounts_trigger() {
    let permits = Arc::new((Mutex::new(0), Condvar::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let observability = ObservabilityV1::start(
        Arc::new(PermitSink {
            permits: Arc::clone(&permits),
            events: Arc::clone(&events),
        }),
        Arc::new(FixedClock(1)),
    );

    observability.emit(SERVICE_FAILED);
    while observability.counters().writing != 1 {
        thread::yield_now();
    }
    for _ in 0..PRIMARY_CAPACITY_V1 {
        observability.emit(SERVICE_FAILED);
    }
    for _ in 0..FALLBACK_CAPACITY_V1 + 8 {
        observability.emit(SERVICE_FAILED);
    }

    let attempted = 1 + PRIMARY_CAPACITY_V1 + FALLBACK_CAPACITY_V1 + 8;
    let counters = observability.counters();
    assert_eq!(counters.fallback_dropped, 9);
    assert_eq!(
        counters.accepted + counters.primary_dropped + counters.fallback_dropped,
        attempted as u64 + 1,
    );

    let (lock, ready) = &*permits;
    *lock.lock().unwrap() = counters.accepted as usize;
    ready.notify_all();
    let report = observability.shutdown();
    let markers = events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.as_str() == "ts=1 operation=queue_saturation outcome=saturated\n")
        .count();
    assert_eq!(markers, 1);
    assert_eq!(report.counters().accepted, report.counters().written);
    assert_eq!(report.counters().unflushed, 0);
}

#[test]
fn concurrent_counter_writers_and_snapshots_preserve_conservation_and_gauge_bounds() {
    const WRITERS: usize = 4;
    const EVENTS_PER_WRITER: usize = 512;

    let observability = ObservabilityV1::start(Arc::new(Sink::default()), Arc::new(FixedClock(1)));
    let emitter = observability.emitter();
    let start = Arc::new(Barrier::new(WRITERS + 1));
    let complete = Arc::new(AtomicUsize::new(0));
    let attempted = Arc::new(AtomicUsize::new(0));
    let mut writers = Vec::with_capacity(WRITERS);

    for _ in 0..WRITERS {
        let emitter = emitter.clone();
        let start = Arc::clone(&start);
        let complete = Arc::clone(&complete);
        let attempted = Arc::clone(&attempted);
        writers.push(thread::spawn(move || {
            start.wait();
            for _ in 0..EVENTS_PER_WRITER {
                attempted.fetch_add(1, Ordering::Release);
                emitter.emit(ADMISSION_OK);
            }
            complete.fetch_add(1, Ordering::Release);
        }));
    }

    start.wait();
    while complete.load(Ordering::Acquire) != WRITERS {
        let counters = emitter.counters();
        let accounted = counters.accepted + counters.primary_dropped + counters.fallback_dropped;
        assert!(accounted <= attempted.load(Ordering::Acquire) as u64);
        assert_eq!(
            counters.accepted,
            counters.written + counters.unflushed + counters.queued + counters.writing
        );
        assert!(counters.writing <= 1);
        assert!(counters.flushing <= 1);
    }
    for writer in writers {
        writer.join().expect("counter writer must not panic");
    }

    let report = observability.shutdown();
    let counters = report.counters();
    assert_eq!(
        counters.accepted + counters.primary_dropped + counters.fallback_dropped,
        (WRITERS * EVENTS_PER_WRITER) as u64
    );
    assert_eq!(
        counters.accepted,
        counters.written + counters.unflushed + counters.queued + counters.writing
    );
}

#[test]
fn each_queue_saturation_episode_emits_one_marker_after_real_primary_relief() {
    let permits = Arc::new((Mutex::new(0), Condvar::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let observability = ObservabilityV1::start(
        Arc::new(PermitSink {
            permits: Arc::clone(&permits),
            events: Arc::clone(&events),
        }),
        Arc::new(FixedClock(1)),
    );

    observability.emit(ADMISSION_OK);
    while observability.counters().writing != 1 {
        thread::yield_now();
    }

    for _ in 0..PRIMARY_CAPACITY_V1 {
        observability.emit(ADMISSION_OK);
    }
    observability.emit(SERVICE_FAILED);
    observability.emit(ADMISSION_OK);
    observability.emit(SERVICE_FAILED);

    let (lock, ready) = &*permits;
    *lock.lock().unwrap() = 1;
    ready.notify_all();
    while observability.counters().queued != (PRIMARY_CAPACITY_V1 + 2) as u64 {
        thread::yield_now();
    }

    observability.emit(ADMISSION_OK);

    for queued in [
        (PRIMARY_CAPACITY_V1 + 1) as u64,
        PRIMARY_CAPACITY_V1 as u64,
        (PRIMARY_CAPACITY_V1 - 1) as u64,
    ] {
        *lock.lock().unwrap() += 1;
        ready.notify_all();
        while observability.counters().queued != queued {
            thread::yield_now();
        }
    }

    observability.emit(ADMISSION_OK);
    observability.emit(ADMISSION_OK);

    *lock.lock().unwrap() = PRIMARY_CAPACITY_V1 * 2 + FALLBACK_CAPACITY_V1;
    ready.notify_all();
    observability.shutdown();

    let markers = events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.as_str() == "ts=1 operation=queue_saturation outcome=saturated\n")
        .count();
    assert_eq!(markers, 2);
}

#[test]
fn flush_panics_are_accounted_without_panicking_shutdown() {
    let observability = ObservabilityV1::start(Arc::new(PanicFlushSink), Arc::new(FixedClock(1)));
    let report = observability.shutdown();
    assert_eq!(
        report.finalization(),
        ObservabilityFinalizationV1::Completed
    );
    assert_eq!(report.counters().flush_failures, 1);
}

fn temporary_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "podway-observability-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path.join("podwayd.log")
}

#[test]
fn rotation_owns_exactly_canonical_numbered_files() {
    let path = temporary_path("retention");
    for suffix in ["0", "00", "01", "6", "99", "000000000000000000000"] {
        fs::write(
            path.with_extension(format!("log.{suffix}")),
            "obsolete rotation",
        )
        .unwrap();
    }
    fs::write(path.with_extension("log.keep"), "neighbor").unwrap();
    let sink = RotatingFileSinkV1::open(&path).unwrap();
    let event = "x".repeat(8 * 1024);
    for _ in 0..=(ROTATION_BYTES_V1 as usize / event.len()) * (RETAINED_ROTATIONS_V1 + 1) {
        sink.write_event(&event).unwrap();
    }
    for index in 1..=RETAINED_ROTATIONS_V1 {
        assert!(path.with_extension(format!("log.{index}")).exists());
    }
    for suffix in ["0", "00", "01", "6", "99", "000000000000000000000"] {
        assert!(!path.with_extension(format!("log.{suffix}")).exists());
    }
    assert!(path.with_extension("log.keep").exists());
    let _ = fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn degraded_mode_bootstrap_failure_is_accounted_and_post_freeze_is_non_mutating() {
    let observability = ObservabilityV1::start_degraded(Arc::new(FixedClock(1)));
    let emitter = observability.emitter();
    emitter.emit(ADMISSION_OK);
    let report = observability.shutdown();
    assert_eq!(
        (
            report.counters().sink_failures,
            report.counters().degraded_dropped
        ),
        (1, 1)
    );
    let frozen = report.counters();
    emitter.emit(ADMISSION_OK);
    assert_eq!(emitter.counters(), frozen);
}
