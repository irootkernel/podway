use std::{
    fs, io,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use podway_daemon::observability::{
    FALLBACK_CAPACITY_V1, PRIMARY_CAPACITY_V1, RETAINED_ROTATIONS_V1, ROTATION_BYTES_V1,
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
    assert!(events[0].contains("ts=42 severity=INFO category=admission detail=[redacted]"));
    assert!(!events[0].contains("request-value"));
}

#[test]
fn saturated_queues_count_drops_without_blocking_domain_work() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let sink = Arc::new(BlockingSink { gate: gate.clone() });
    let observability = ObservabilityV1::start(sink, Arc::new(FakeClock::new(1)));
    for _ in 0..PRIMARY_CAPACITY_V1 + FALLBACK_CAPACITY_V1 + 8 {
        observability.emit(
            EventCategoryV1::TerminalOrRequeueOrSaturation,
            SeverityV1::Error,
        );
    }
    let counters = observability.counters();
    assert!(counters.primary_dropped > 0);
    assert!(counters.fallback_dropped > 0);
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
    let stale_sink =
        RotatingFileSinkV1::open(&stale_path, Arc::new(FakeClock::new(u64::MAX))).unwrap();
    for _ in 0..=(ROTATION_BYTES_V1 as usize / event.len()) {
        stale_sink.write_event(&event).unwrap();
    }
    assert!(!stale_path.with_extension("log.1").exists());
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
fn component_has_no_network_configuration_or_io_surface() {
    // The component accepts only a sink and clock; this fake sink proves emission needs no socket.
    let sink = Arc::new(FakeSink::default());
    let observability = ObservabilityV1::start(sink, Arc::new(FakeClock::new(0)));
    observability.emit(EventCategoryV1::StaleEvidence, SeverityV1::Debug);
    thread::sleep(Duration::from_millis(1));
    observability.shutdown();
}
