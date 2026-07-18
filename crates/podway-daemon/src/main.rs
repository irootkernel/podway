#![forbid(unsafe_code)]

use std::{env, num::NonZeroUsize, path::PathBuf, process, sync::Arc, thread};

use nix::unistd::geteuid;
use podway_daemon::{
    EventCategoryV1, ObservabilityV1, RotatingFileSinkV1, SeverityV1, SystemClockV1,
    observability::ObservabilityShutdownReportV1,
    runtime::{ProductionDaemonRuntimeConfigV1, ProductionDaemonRuntimeV1},
    server::ServerTransportTimeoutsV1,
};
use podway_service::ServiceRuntimePathsV1;
use podway_store::{SqliteStoreOptionsV1, WorkerIdV1};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::Signals,
};

const MAXIMUM_IN_FLIGHT_CONNECTIONS_V1: usize = 64;

fn main() {
    if let Err(error) = run() {
        eprintln!("podwayd: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    match (arguments.next(), arguments.next()) {
        (Some(argument), None) if argument == "--version" => {
            println!("podwayd {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        (None, None) => {}
        (Some(argument), None) if argument == "--service" => {}
        _ => return Err("usage: podwayd [--service|--version]".into()),
    }
    run_service()
}

fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    let temporary = env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let paths = ServiceRuntimePathsV1::for_user(&home, temporary, geteuid().as_raw())?;
    let configuration = ProductionDaemonRuntimeConfigV1::new(
        WorkerIdV1::new(format!("podwayd-{}", process::id()))?,
        NonZeroUsize::new(MAXIMUM_IN_FLIGHT_CONNECTIONS_V1)
            .expect("the production connection limit is nonzero"),
        ServerTransportTimeoutsV1::default(),
    );
    let inspection_options = SqliteStoreOptionsV1::new(1)?;
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    let signal_control = signals.handle();
    let clock = Arc::new(SystemClockV1);
    let observability = match RotatingFileSinkV1::open(paths.log_path().as_path(), clock.clone()) {
        Ok(sink) => ObservabilityV1::start(Arc::new(sink), clock),
        Err(error) => {
            // Bootstrap has no functioning sink; this is explicit degraded-mode evidence.
            eprintln!("podwayd observability bootstrap sink-open failure: {error}");
            ObservabilityV1::start_degraded(clock)
        }
    };
    observability.emit(EventCategoryV1::Lifecycle, SeverityV1::Info);
    let runtime = match ProductionDaemonRuntimeV1::bind_with_observability(
        &paths,
        inspection_options,
        configuration,
        Some(observability.emitter()),
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            observability.emit(EventCategoryV1::ServiceOutcome, SeverityV1::Error);
            report_observability_shutdown(observability.shutdown_report());
            return Err(error.into());
        }
    };
    let shutdown = runtime.shutdown_handle();
    let relay = match thread::Builder::new()
        .name("podwayd-signal-relay".to_owned())
        .spawn(move || {
            if signals.forever().next().is_some() {
                shutdown.request_shutdown();
            }
        }) {
        Ok(relay) => relay,
        Err(source) => {
            signal_control.close();
            runtime.shutdown_handle().request_shutdown();
            let runtime_result = runtime.run();
            observability.emit(EventCategoryV1::ServiceOutcome, SeverityV1::Error);
            report_observability_shutdown(observability.shutdown_report());
            if let Err(cleanup) = runtime_result {
                return Err(std::io::Error::other(format!(
                    "cannot start signal relay ({source}); endpoint cleanup also failed: {cleanup}"
                ))
                .into());
            }
            return Err(source.into());
        }
    };

    let runtime_result = runtime.run();
    signal_control.close();
    let relay_result = relay.join();
    observability.emit(
        EventCategoryV1::ServiceOutcome,
        if runtime_result.is_ok() && relay_result.is_ok() {
            SeverityV1::Info
        } else {
            SeverityV1::Error
        },
    );
    report_observability_shutdown(observability.shutdown_report());

    runtime_result?;
    relay_result.map_err(|_| "signal relay panicked")?;
    Ok(())
}
fn report_observability_shutdown(report: ObservabilityShutdownReportV1) {
    let counters = report.counters();
    eprintln!(
        "podwayd observability finalization={:?} accepted={} written={} primary_dropped={} fallback_dropped={} stderr_dropped={} stopped_dropped={} degraded_dropped={} unflushed={} queued={} writing={} flushing={} write_failures={} flush_failures={} clock_failures={} sink_failures={} admission_contention={} admission_poisoned={} bootstrap_failures={} worker_panics={} worker_disconnects={} worker_join_failures={} shutdown_timeouts={} counters_saturated={}",
        report.finalization(),
        counters.accepted,
        counters.written,
        counters.primary_dropped,
        counters.fallback_dropped,
        counters.stderr_dropped,
        counters.stopped_dropped,
        counters.degraded_dropped,
        counters.unflushed,
        counters.queued,
        counters.writing,
        counters.flushing,
        counters.write_failures,
        counters.flush_failures,
        counters.clock_failures,
        counters.sink_failures,
        counters.admission_contention,
        counters.admission_poisoned,
        counters.bootstrap_failures,
        counters.worker_panics,
        counters.worker_disconnects,
        counters.worker_join_failures,
        counters.shutdown_timeouts,
        counters.counters_saturated,
    );
}
