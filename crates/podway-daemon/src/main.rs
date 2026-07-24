#![forbid(unsafe_code)]

use std::{env, io, num::NonZeroUsize, path::PathBuf, process, sync::Arc, thread};

#[cfg(debug_assertions)]
use nix::unistd::geteuid;
use podway_daemon::{
    ObservabilityCountersV1, ObservabilityFinalizationV1, ObservabilityV1, RotatingFileSinkV1,
    SystemClockV1,
    runtime::{ProductionDaemonRuntimeConfigV1, ProductionDaemonRuntimeV1},
    server::ServerTransportTimeoutsV1,
};
use podway_protocol::build_identity_v1;
use podway_service::ServiceRuntimePathsV1;
use podway_store::{SqliteStoreOptionsV1, WorkerIdV1};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::Signals,
};

const MAXIMUM_IN_FLIGHT_CONNECTIONS_V1: usize = 64;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let socket_path = match arguments.as_slice() {
        [argument] if argument == "--version" => {
            println!("podwayd {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        [json, version] if json == "--json" && version == "version" => {
            println!(
                "{}",
                serde_json::to_string(&build_identity_v1())
                    .expect("the static build identity always serializes")
            );
            return Ok(());
        }
        [] => None,
        [argument] if argument == "--service" => None,
        [service, socket, path] if service == "--service" && socket == "--socket" => {
            Some(PathBuf::from(path))
        }
        _ => {
            return Err(
                "usage: podwayd [--service [--socket <absolute-path>]|--version|--json version]"
                    .into(),
            );
        }
    };
    run_service(socket_path)
}

fn run_service(socket_path: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let paths = effective_service_paths()?;
    let paths = match socket_path {
        Some(socket_path) => paths.with_socket_path(socket_path)?,
        None => paths,
    };
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
    let observability = match RotatingFileSinkV1::open(paths.log_path().as_path()) {
        Ok(sink) => ObservabilityV1::start(Arc::new(sink), clock.clone()),
        Err(_) => ObservabilityV1::start_degraded(clock),
    };
    let runtime = match ProductionDaemonRuntimeV1::bind_with_observability(
        &paths,
        inspection_options,
        configuration,
        Some(observability.emitter()),
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            let mut failures = vec![("bind", error.to_string())];
            if let Err(error) = finalize_observability(observability) {
                failures.push(("observability", error.to_string()));
            }
            return compose_failures("daemon startup failed", failures);
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
            let observability_result = finalize_observability(observability);
            return relay_spawn_failure(
                source.to_string(),
                runtime_result
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                observability_result.map_err(|error| error.to_string()),
            );
        }
    };

    let runtime_result = runtime.run();
    signal_control.close();
    let relay_result = relay.join();
    let observability_result = finalize_observability(observability);

    runtime_completion(
        runtime_result
            .map(|_| ())
            .map_err(|error| error.to_string()),
        relay_result.map_err(|_| "signal relay panicked".to_owned()),
        observability_result.map_err(|error| error.to_string()),
    )
}

fn effective_service_paths() -> Result<ServiceRuntimePathsV1, podway_service::ServicePathErrorV1> {
    #[cfg(debug_assertions)]
    if let Some(account_root) = env::var_os("PODWAY_TEST_ACCOUNT_ROOT") {
        return ServiceRuntimePathsV1::for_account_home(account_root, geteuid().as_raw());
    }
    ServiceRuntimePathsV1::for_effective_user()
}

type StageResult = Result<(), String>;

fn relay_spawn_failure(
    source: String,
    runtime_cleanup: StageResult,
    observability_finalization: StageResult,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut failures = vec![("signal relay spawn", source)];
    if let Err(error) = runtime_cleanup {
        failures.push(("runtime cleanup", error));
    }
    if let Err(error) = observability_finalization {
        failures.push(("observability", error));
    }
    compose_failures("cannot start signal relay", failures)
}

fn runtime_completion(
    runtime: StageResult,
    relay: StageResult,
    observability_finalization: StageResult,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut failures = Vec::new();
    if let Err(error) = runtime {
        failures.push(("runtime", error));
    }
    if let Err(error) = relay {
        failures.push(("signal relay", error));
    }
    if let Err(error) = observability_finalization {
        failures.push(("observability", error));
    }
    compose_failures("daemon runtime failed", failures)
}
fn finalize_observability(observability: ObservabilityV1) -> std::io::Result<()> {
    let report = observability.shutdown();
    finalize_observability_report(report.finalization(), report.counters())
}

fn finalize_observability_report(
    finalization: ObservabilityFinalizationV1,
    counters: ObservabilityCountersV1,
) -> io::Result<()> {
    let failed = finalization != ObservabilityFinalizationV1::Completed
        || counters.primary_dropped != 0
        || counters.fallback_dropped != 0
        || counters.stopped_dropped != 0
        || counters.degraded_dropped != 0
        || counters.unflushed != 0
        || counters.queued != 0
        || counters.writing != 0
        || counters.flushing != 0
        || counters.write_failures != 0
        || counters.flush_failures != 0
        || counters.clock_errors != 0
        || counters.clock_panics != 0
        || counters.sink_failures != 0
        || counters.counters_saturated;
    if failed {
        Err(io::Error::other(format!(
            "observability finalization failed: finalization={finalization:?}; counters={counters:?}"
        )))
    } else {
        Ok(())
    }
}
fn compose_failures(
    context: &str,
    failures: Vec<(&'static str, String)>,
) -> Result<(), Box<dyn std::error::Error>> {
    if failures.is_empty() {
        return Ok(());
    }
    let details = failures
        .into_iter()
        .map(|(stage, error)| format!("{stage}: {error}"))
        .collect::<Vec<_>>()
        .join("; ");
    Err(io::Error::other(format!("{context}: {details}")).into())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_spawn_failure_retains_source_cleanup_and_observability_failures() {
        let error = relay_spawn_failure(
            "resource temporarily unavailable".to_owned(),
            Err("shutdown cleanup timed out".to_owned()),
            Err("observability finalization failed: unflushed events".to_owned()),
        )
        .expect_err("simultaneous relay-spawn cleanup failures must compose");

        assert_eq!(
            error.to_string(),
            "cannot start signal relay: signal relay spawn: resource temporarily unavailable; runtime cleanup: shutdown cleanup timed out; observability: observability finalization failed: unflushed events"
        );
    }

    #[test]
    fn runtime_completion_retains_runtime_relay_and_observability_failures() {
        let error = runtime_completion(
            Err("listener closed unexpectedly".to_owned()),
            Err("signal relay panicked".to_owned()),
            Err("observability finalization failed: flush timed out".to_owned()),
        )
        .expect_err("simultaneous runtime completion failures must compose");

        assert_eq!(
            error.to_string(),
            "daemon runtime failed: runtime: listener closed unexpectedly; signal relay: signal relay panicked; observability: observability finalization failed: flush timed out"
        );
    }
    #[test]
    fn compose_failures_retains_each_labeled_stage_and_detail() {
        let error = compose_failures(
            "daemon runtime failed",
            vec![
                ("runtime", "listener closed".to_owned()),
                ("signal relay", "signal relay panicked".to_owned()),
                ("observability", "flush timed out".to_owned()),
            ],
        )
        .expect_err("multiple failures must compose into an error");

        assert_eq!(
            error.to_string(),
            "daemon runtime failed: runtime: listener closed; signal relay: signal relay panicked; observability: flush timed out"
        );
    }

    #[test]
    fn observability_finalization_error_includes_typed_finalization_and_counters() {
        let counters = ObservabilityCountersV1 {
            unflushed: 3,
            ..ObservabilityCountersV1::default()
        };

        let error = finalize_observability_report(ObservabilityFinalizationV1::Detached, counters)
            .expect_err("detached finalization with unflushed events must fail");

        let text = error.to_string();
        assert!(text.contains("finalization=Detached"));
        assert!(text.contains("unflushed: 3"));
    }
    #[test]
    fn observability_sink_bootstrap_failure_fails_finalization() {
        let counters = ObservabilityCountersV1 {
            sink_failures: 1,
            ..ObservabilityCountersV1::default()
        };

        let error = finalize_observability_report(ObservabilityFinalizationV1::Completed, counters)
            .expect_err("sink bootstrap failure must remain visible at finalization");
        assert!(error.to_string().contains("sink_failures: 1"));
    }
}
