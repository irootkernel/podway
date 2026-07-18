use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use podway_core::UnixMillis;
use podway_service::{
    FixedServiceClockV1, InstallSpecV1, LocalPlatformPathV1, LogQueryV1,
    MacosServiceCommandRunnerV1, ServiceLabelV1, ServiceManagerContractV1, ServiceManagerV1,
    ServiceObservationV1, ServiceObserverV1, ServiceRuntimePathsV1, StdServiceFilesystemV1,
    SystemLaunchctlRunnerV1, UninstallOptionsV1,
};

#[derive(Default)]
struct RecordingObserver(Mutex<Vec<ServiceObservationV1>>);

impl ServiceObserverV1 for RecordingObserver {
    fn observe(&self, observation: ServiceObservationV1) {
        self.0.lock().expect("observer lock").push(observation);
    }
}

fn unique_root() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "podway-phase8-observability-{}-{nanos}",
        std::process::id()
    ))
}

fn write_executable(path: &Path) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
    fs::write(path, b"#!/bin/sh\nexit 0\n").expect("write executable fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("chmod fixture");
}

fn spec(binary: &Path, paths: &ServiceRuntimePathsV1) -> InstallSpecV1 {
    InstallSpecV1::new(
        LocalPlatformPathV1::new(binary).expect("absolute binary"),
        ServiceLabelV1::podwayd(),
        paths.clone(),
    )
}

#[test]
fn service_observer_emits_only_stable_categories_at_production_boundaries() {
    let root = unique_root();
    let home = root.join("home");
    let runtime = root.join("runtime");
    let paths = ServiceRuntimePathsV1::from_directories(
        home.join("Library/LaunchAgents"),
        home.join("Library/Application Support/Podway"),
        home.join("Library/Logs/Podway"),
        &runtime,
    )
    .expect("service paths");
    let binary = root.join("bin/podwayd");
    let launchctl = root.join("bin/launchctl");
    write_executable(&binary);
    write_executable(&launchctl);

    let observer = Arc::new(RecordingObserver::default());
    let clock = FixedServiceClockV1::new(UnixMillis::new(1));
    let runner = MacosServiceCommandRunnerV1::new_with_observer(
        StdServiceFilesystemV1,
        SystemLaunchctlRunnerV1::new(&launchctl),
        clock,
        501,
        observer.clone(),
    )
    .expect("runner");
    let manager = ServiceManagerV1::new(runner, clock, paths.clone());

    manager.install(spec(&binary, &paths)).expect("install");
    fs::write(paths.socket_path().as_path(), b"stale").expect("stale socket");
    manager.restart().expect("restart");
    manager
        .uninstall_with_options(UninstallOptionsV1::new(false))
        .expect("uninstall preserves logs");
    assert!(
        manager.logs(LogQueryV1::default()).is_err(),
        "missing log must remain a returned service error"
    );

    let events = observer.0.lock().expect("observer lock").clone();
    for expected in [
        ServiceObservationV1::AtomicPlistPublished,
        ServiceObservationV1::AtomicMetadataPublished,
        ServiceObservationV1::LaunchctlSideEffectRequested,
        ServiceObservationV1::LaunchctlSideEffectCompleted,
        ServiceObservationV1::LogRotationCompleted,
        ServiceObservationV1::StaleSocketRemoved,
        ServiceObservationV1::UninstallLogsPreserved,
        ServiceObservationV1::ServiceOutcome,
        ServiceObservationV1::Error,
    ] {
        assert!(events.contains(&expected), "missing {expected:?}");
    }

    fs::remove_dir_all(root).expect("remove fixture");
}
