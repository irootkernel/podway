#![forbid(unsafe_code)]

use std::{env, io, num::NonZeroUsize, path::PathBuf, process, sync::Arc, thread};

use nix::unistd::geteuid;
#[cfg(all(feature = "development-v2-admission", debug_assertions))]
use podway_daemon::managed_dev::ManagedDevPurposeV2;
use podway_daemon::{
    LogSinkV1, ObservabilityCountersV1, ObservabilityFinalizationV1, ObservabilityV1,
    RotatingFileSinkV1, SystemClockV1,
    managed_dev::ManagedDevRuntimeV2,
    runtime::{ProductionDaemonRuntimeConfigV1, ProductionDaemonRuntimeV1},
    server::{
        DaemonProcessIdentityV1, ResponseMetadataSourceV1, ServerTransportTimeoutsV1,
        SystemResponseMetadataSourceV1,
    },
};
use podway_protocol::{
    CommandNameV1, OutputEnvelopeInputV3, OutputEnvelopeV3, RequestIdV1, build_identity_v1,
};
use podway_service::ServiceRuntimePathsV1;
use podway_store::{SqliteStoreOptionsV1, WorkerIdV1};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::Signals,
};
use uuid::Uuid;

const MAXIMUM_IN_FLIGHT_CONNECTIONS_V1: usize = 64;

fn main() {
    if run().is_err() {
        write_bootstrap_stderr("failed", None, "process", Some("startup_failure"));
        process::exit(1);
    }
}

fn bootstrap_event(
    outcome: &'static str,
    daemon_id: Option<&str>,
    stage: &'static str,
    error_kind: Option<&'static str>,
) -> String {
    let timestamp = daemon_id.map(|_| {
        SystemResponseMetadataSourceV1::default()
            .generated_at()
            .into_inner()
    });
    serde_json::json!({
        "schema": "podway.daemon-bootstrap-log/v1",
        "ts": timestamp,
        "daemon_id": daemon_id,
        "seq": 0,
        "operation": "daemon_bootstrap",
        "outcome": outcome,
        "command": null,
        "workspace_uuid": null,
        "session_id": null,
        "request_id": null,
        "job_id": null,
        "stage": stage,
        "error_kind": error_kind,
        "integrity_check": null,
        "reason": null,
        "diagnostic_id": null,
        "message": null,
    })
    .to_string()
}

fn write_bootstrap_stderr(
    outcome: &'static str,
    daemon_id: Option<&str>,
    stage: &'static str,
    error_kind: Option<&'static str>,
) {
    eprintln!("{}", bootstrap_event(outcome, daemon_id, stage, error_kind));
}

fn write_bootstrap_file(
    path: &std::path::Path,
    outcome: &'static str,
    daemon_id: Option<&str>,
    stage: &'static str,
    error_kind: Option<&'static str>,
) {
    let Ok(sink) = RotatingFileSinkV1::open_bootstrap(path) else {
        return;
    };
    let mut event = bootstrap_event(outcome, daemon_id, stage, error_kind);
    event.push('\n');
    let _ = sink.write_event(&event).and_then(|()| sink.flush());
}

struct BootstrapFailureGuardV1 {
    path: PathBuf,
    daemon_id: String,
    armed: bool,
}

impl BootstrapFailureGuardV1 {
    fn new(path: PathBuf, daemon_id: String) -> Self {
        Self {
            path,
            daemon_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BootstrapFailureGuardV1 {
    fn drop(&mut self) {
        if self.armed {
            write_bootstrap_file(
                &self.path,
                "failed",
                Some(&self.daemon_id),
                "process",
                Some("startup_failure"),
            );
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(all(feature = "development-v2-admission", debug_assertions))]
    {
        const PROBE_ARGUMENT: &str = "--podway-development-v2-admission-probe";
        const PROBE_ENVIRONMENT: &str = "PODWAY_DEVELOPMENT_V2_ADMISSION_PROBE";
        const PROBE_TOKEN: &str = "podway-development-v2-admission-v1";
        let probe_arguments = env::args_os().skip(1).collect::<Vec<_>>();
        if probe_arguments.as_slice() == [PROBE_ARGUMENT]
            && env::var_os(PROBE_ENVIRONMENT).as_deref() == Some(std::ffi::OsStr::new(PROBE_TOKEN))
        {
            println!("{PROBE_TOKEN}");
            return Ok(());
        }
    }

    #[cfg(debug_assertions)]
    {
        const PROBE_ARGUMENT: &str = "--podway-test-isolation-probe";
        const PROBE_TOKEN: &str = "podway-test-isolation-v1";
        let probe_arguments = env::args_os().skip(1).collect::<Vec<_>>();
        if probe_arguments.as_slice() == [PROBE_ARGUMENT]
            && env::var_os("PODWAY_TEST_ISOLATION_PROBE").as_deref()
                == Some(std::ffi::OsStr::new(PROBE_TOKEN))
        {
            println!("{PROBE_TOKEN}");
            return Ok(());
        }
    }

    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let (dev_mode, socket_path) = match arguments.as_slice() {
        [argument] if argument == "version" || argument == "--version" => {
            println!("podwayd {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        [version, json] if version == "version" && json == "--json" => {
            println!(
                "{}",
                serde_json::json!({
                    "name": "podwayd",
                    "version": format!("v{}", env!("CARGO_PKG_VERSION")),
                })
            );
            return Ok(());
        }
        [version, first, second]
            if version == "version"
                && ((first == "--json" && second == "--identity")
                    || (first == "--identity" && second == "--json")) =>
        {
            let generated_at = SystemResponseMetadataSourceV1::default().generated_at();
            let result = serde_json::to_value(build_identity_v1())
                .expect("the static build identity always serializes")
                .as_object()
                .cloned()
                .expect("the static build identity is an object");
            let output = OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
                request_id: RequestIdV1::new(Uuid::new_v4().to_string())?,
                command: CommandNameV1::new("version")?,
                generated_at,
                workspace: None,
                job: None,
                session: None,
                result,
                warnings: Vec::new(),
            })?;
            println!(
                "{}",
                serde_json::to_string(&output)
                    .expect("the validated version envelope always serializes")
            );
            return Ok(());
        }
        [] => (false, None),
        [argument] if argument == "--service" => (false, None),
        [argument] if argument == "--dev" => (true, None),
        [service, socket, path] if service == "--service" && socket == "--socket" => {
            (false, Some(PathBuf::from(path)))
        }
        _ => {
            return Err(
                "usage: podwayd [--dev|--service [--socket <absolute-path>]|version [--json [--identity]]]"
                    .into(),
            );
        }
    };
    run_service(dev_mode, socket_path)
}

fn run_service(
    dev_mode: bool,
    socket_path: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (paths, managed_dev) = effective_service_paths(dev_mode)?;
    let paths = match socket_path {
        Some(socket_path) => paths.with_socket_path(socket_path)?,
        None => paths,
    };
    let process_identity = DaemonProcessIdentityV1::new(
        RequestIdV1::new(Uuid::new_v4().to_string())?,
        process::id(),
        SystemResponseMetadataSourceV1::default().generated_at(),
        env::current_exe()?.canonicalize()?,
        paths.socket_path().as_path(),
        paths.socket_path().as_path(),
    )?;
    let daemon_id = process_identity.process_id().as_str().to_owned();
    let bootstrap_log_path = paths.bootstrap_log_path().as_path().to_path_buf();
    write_bootstrap_file(
        &bootstrap_log_path,
        "succeeded",
        Some(&daemon_id),
        "process",
        None,
    );
    let mut bootstrap_failure = BootstrapFailureGuardV1::new(bootstrap_log_path, daemon_id.clone());
    let mut configuration = ProductionDaemonRuntimeConfigV1::new(
        WorkerIdV1::new(format!("podwayd-{}", process::id()))?,
        NonZeroUsize::new(MAXIMUM_IN_FLIGHT_CONNECTIONS_V1)
            .expect("the production connection limit is nonzero"),
        ServerTransportTimeoutsV1::default(),
    )
    .with_process_identity(process_identity);
    if let Some(managed_dev) = &managed_dev {
        configuration = configuration.with_managed_dev_workspace_root(managed_dev.sandbox());
    }
    #[cfg(all(feature = "development-v2-admission", debug_assertions))]
    if dev_mode {
        configuration = configuration.with_dev_mode();
        if managed_dev
            .as_ref()
            .is_some_and(|runtime| runtime.purpose() == ManagedDevPurposeV2::Contributor)
        {
            configuration = configuration
                .with_development_v2_admission(&paths, &env::current_exe()?.canonicalize()?);
        }
    }
    #[cfg(not(all(feature = "development-v2-admission", debug_assertions)))]
    if dev_mode {
        configuration = configuration.with_dev_mode();
    }
    let inspection_options = SqliteStoreOptionsV1::new(1)?;
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    let signal_control = signals.handle();
    let clock = Arc::new(SystemClockV1);
    let observability = match RotatingFileSinkV1::open(paths.log_path().as_path()) {
        Ok(sink) => ObservabilityV1::start_with_daemon_id(Arc::new(sink), clock.clone(), daemon_id),
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

    let outcome = runtime_completion(
        runtime_result
            .map(|_| ())
            .map_err(|error| error.to_string()),
        relay_result.map_err(|_| "signal relay panicked".to_owned()),
        observability_result.map_err(|error| error.to_string()),
    );
    if outcome.is_ok() {
        bootstrap_failure.disarm();
    }
    outcome
}

fn effective_service_paths(
    dev_mode: bool,
) -> Result<(ServiceRuntimePathsV1, Option<ManagedDevRuntimeV2>), Box<dyn std::error::Error>> {
    if dev_mode
        && let Some(dev_home) = env::var_os("PODWAY_DEV_HOME").map(PathBuf::from)
        && let Some(runtime) =
            ManagedDevRuntimeV2::discover(&dev_home, &env::current_exe()?.canonicalize()?)?
    {
        let paths = ServiceRuntimePathsV1::for_dev_home(
            runtime.account_root(),
            runtime.dev_home(),
            geteuid().as_raw(),
        )?;
        return Ok((paths, Some(runtime)));
    }
    #[cfg(debug_assertions)]
    if let Some(account_root) = env::var_os("PODWAY_TEST_ACCOUNT_ROOT") {
        if dev_mode {
            let account_root = PathBuf::from(account_root);
            let dev_home = env::var_os("PODWAY_DEV_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| account_root.join(".podway/dev"));
            return Ok((
                ServiceRuntimePathsV1::for_dev_home(account_root, dev_home, geteuid().as_raw())?,
                None,
            ));
        }
        return Ok((
            ServiceRuntimePathsV1::for_account_home(account_root, geteuid().as_raw())?,
            None,
        ));
    }
    if dev_mode {
        let dev_home = env::var_os("PODWAY_DEV_HOME").map(PathBuf::from);
        return Ok((
            ServiceRuntimePathsV1::for_effective_user_dev(dev_home.as_deref())?,
            None,
        ));
    }
    Ok((ServiceRuntimePathsV1::for_effective_user()?, None))
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
