use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use podway_core::UnixMillis;
use podway_service::{
    DaemonContractVerifierV1, FixedServiceClockV1, InstallSpecV1, LaunchctlRunnerV1,
    LocalPlatformPathV1, LogQueryV1,
    MacosServiceCommandRunnerV1 as ProductionMacosServiceCommandRunnerV1, ServiceClockV1,
    ServiceErrorV1, ServiceFilesystemV1, ServiceLabelV1, ServiceManagerContractV1,
    ServiceManagerV1, ServiceObservationV1, ServiceObserverV1, ServiceOperationV1,
    ServicePathErrorV1, ServiceRuntimePathsV1, StdServiceFilesystemV1, SystemLaunchctlRunnerV1,
    UninstallOptionsV1,
};

#[derive(Clone, Copy, Debug)]
struct MatchingDaemonContractVerifierV1;

impl DaemonContractVerifierV1 for MatchingDaemonContractVerifierV1 {
    fn verify(&self, _: &Path, _: &str, _: &str) -> Result<(), ServiceErrorV1> {
        Ok(())
    }
}

struct MacosServiceCommandRunnerV1;

impl MacosServiceCommandRunnerV1 {
    fn new_with_observer<F, L, C>(
        filesystem: F,
        launchctl: L,
        clock: C,
        user_id: u32,
        observer: Arc<dyn ServiceObserverV1>,
    ) -> Result<
        ProductionMacosServiceCommandRunnerV1<F, L, C, MatchingDaemonContractVerifierV1>,
        ServicePathErrorV1,
    >
    where
        F: ServiceFilesystemV1,
        L: LaunchctlRunnerV1,
        C: ServiceClockV1,
    {
        ProductionMacosServiceCommandRunnerV1::new_with_observer_and_contract_verifier(
            filesystem,
            launchctl,
            clock,
            user_id,
            MatchingDaemonContractVerifierV1,
            observer,
        )
    }
}

#[derive(Default)]
struct RecordingObserver(Mutex<Vec<ServiceObservationV1>>);

impl ServiceObserverV1 for RecordingObserver {
    fn observe(&self, observation: ServiceObservationV1) {
        self.0.lock().expect("observer lock").push(observation);
    }
}

fn unique_root() -> PathBuf {
    PathBuf::from(format!("/tmp/pw8o-{}", std::process::id()))
}

fn write_executable(path: &Path) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
    let mut bytes = vec![0_u8; 40];
    bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
    bytes[4..8].copy_from_slice(&0x0100_000c_u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&1_u32.to_le_bytes());
    bytes[20..24].copy_from_slice(&8_u32.to_le_bytes());
    bytes[32..36].copy_from_slice(&0x32_u32.to_le_bytes());
    bytes[36..40].copy_from_slice(&8_u32.to_le_bytes());
    fs::write(path, bytes).expect("write executable fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("chmod fixture");
}

fn spec(binary: &Path, paths: &ServiceRuntimePathsV1) -> InstallSpecV1 {
    InstallSpecV1::new(
        LocalPlatformPathV1::new(binary).expect("absolute binary"),
        ServiceLabelV1::podwayd(),
        paths.clone(),
        "podway",
        format!("sha256:{}", "a".repeat(64)),
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
    fs::write(
        &launchctl,
        b"#!/bin/sh\nif [ \"$1\" = print ]; then printf 'gui/501/dev.podway.podwayd = {\\npid = 4242\\n'; fi\nexit 0\n",
    )
    .expect("write launchctl fixture");
    fs::set_permissions(&launchctl, fs::Permissions::from_mode(0o700))
        .expect("chmod launchctl fixture");

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
    assert_eq!(
        events,
        vec![
            ServiceObservationV1::LaunchctlSideEffectRequested,
            ServiceObservationV1::LaunchctlSideEffectCompleted,
            ServiceObservationV1::LaunchctlSideEffectRequested,
            ServiceObservationV1::LaunchctlSideEffectCompleted,
            ServiceObservationV1::AtomicMetadataPublished,
            ServiceObservationV1::AtomicPlistPublished,
            ServiceObservationV1::LogRotationCompleted,
            ServiceObservationV1::LaunchctlSideEffectRequested,
            ServiceObservationV1::LaunchctlSideEffectCompleted,
            ServiceObservationV1::AtomicMetadataPublished,
            ServiceObservationV1::ServiceOutcome(ServiceOperationV1::Install),
            ServiceObservationV1::LaunchctlSideEffectRequested,
            ServiceObservationV1::LaunchctlSideEffectCompleted,
            ServiceObservationV1::LaunchctlSideEffectRequested,
            ServiceObservationV1::LaunchctlSideEffectCompleted,
            ServiceObservationV1::LogRotationCompleted,
            ServiceObservationV1::LaunchctlSideEffectRequested,
            ServiceObservationV1::LaunchctlSideEffectCompleted,
            ServiceObservationV1::ServiceOutcome(ServiceOperationV1::Restart),
            ServiceObservationV1::LaunchctlSideEffectRequested,
            ServiceObservationV1::LaunchctlSideEffectCompleted,
            ServiceObservationV1::LaunchctlSideEffectRequested,
            ServiceObservationV1::LaunchctlSideEffectCompleted,
            ServiceObservationV1::UninstallLogsPreserved,
            ServiceObservationV1::ServiceOutcome(ServiceOperationV1::Uninstall),
            ServiceObservationV1::Error(ServiceOperationV1::Logs),
        ]
    );

    fs::remove_dir_all(root).expect("remove fixture");
}
