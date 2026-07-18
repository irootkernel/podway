use std::{
    fs,
    fs::OpenOptions,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use podway_core::UnixMillis;
use podway_service::{
    FixedServiceClockV1, InstallSpecV1, LaunchctlOutputV1, LaunchctlRunnerV1, LocalPlatformPathV1,
    MacosServiceCommandRunnerV1, SERVICE_LABEL_V1, ServiceCommandRunnerV1, ServiceErrorV1,
    ServiceFilesystemErrorV1, ServiceFilesystemV1, ServiceLabelV1, ServiceManagerContractV1,
    ServiceManagerV1, ServiceOutcomeKindV1, ServiceRuntimePathsV1, StdServiceFilesystemV1,
};

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct CrashAtDurabilityBoundary;

impl ServiceFilesystemV1 for CrashAtDurabilityBoundary {
    fn exists(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
        StdServiceFilesystemV1.exists(path)
    }
    fn is_executable(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
        StdServiceFilesystemV1.is_executable(path)
    }
    fn create_directory(&self, path: &Path, mode: u32) -> Result<(), ServiceFilesystemErrorV1> {
        StdServiceFilesystemV1.create_directory(path, mode)
    }
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, ServiceFilesystemErrorV1> {
        StdServiceFilesystemV1.read_file(path)
    }
    fn write_atomically(
        &self,
        path: &Path,
        contents: &[u8],
        mode: u32,
    ) -> Result<(), ServiceFilesystemErrorV1> {
        if std::env::var_os("PODWAY_SERVICE_CRASH_MODE").is_some_and(|mode| mode == "publish") {
            std::process::exit(86);
        }
        StdServiceFilesystemV1.write_atomically(path, contents, mode)
    }
    fn remove_file(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        if std::env::var_os("PODWAY_SERVICE_CRASH_MODE").is_some_and(|mode| mode == "remove") {
            std::process::exit(87);
        }
        StdServiceFilesystemV1.remove_file(path)
    }
    fn remove_directory_contents(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        StdServiceFilesystemV1.remove_directory_contents(path)
    }
    fn rotate_file(
        &self,
        path: &Path,
        maximum_bytes: u64,
        retained_files: u8,
    ) -> Result<(), ServiceFilesystemErrorV1> {
        StdServiceFilesystemV1.rotate_file(path, maximum_bytes, retained_files)
    }
}

struct SuccessfulLaunchctl;

impl LaunchctlRunnerV1 for SuccessfulLaunchctl {
    fn run(&self, _: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        Ok(LaunchctlOutputV1::success())
    }
}
struct CrashAfterBootstrapLaunchctl {
    marker: PathBuf,
}

impl LaunchctlRunnerV1 for CrashAfterBootstrapLaunchctl {
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        if arguments
            .first()
            .is_some_and(|argument| argument == "bootstrap")
        {
            let mut marker = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.marker)
                .expect("create durable bootstrap marker");
            marker
                .write_all(b"bootstrap-completed\n")
                .expect("write bootstrap marker");
            marker.sync_all().expect("sync bootstrap marker");
            std::process::exit(88);
        }
        Ok(LaunchctlOutputV1::success())
    }
}

struct ReconciliationLaunchctl {
    marker: PathBuf,
}

impl LaunchctlRunnerV1 for ReconciliationLaunchctl {
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        if arguments
            .first()
            .is_some_and(|argument| argument == "bootstrap")
        {
            assert_eq!(
                fs::read(&self.marker).expect("prior bootstrap marker"),
                b"bootstrap-completed\n",
                "retry must reconcile the completed side effect rather than repeat it"
            );
        }
        Ok(LaunchctlOutputV1::success())
    }
}

fn install_spec(binary: &Path, paths: &ServiceRuntimePathsV1) -> InstallSpecV1 {
    InstallSpecV1::new(
        LocalPlatformPathV1::new(binary).expect("absolute binary"),
        ServiceLabelV1::podwayd(),
        paths.clone(),
    )
}

fn unique_root() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "podway-phase8-crash-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}

fn paths(root: &Path) -> ServiceRuntimePathsV1 {
    ServiceRuntimePathsV1::from_directories(
        root.join("home/Library/LaunchAgents"),
        root.join("home/Library/Application Support/Podway"),
        root.join("home/Library/Logs/Podway"),
        root.join("runtime"),
    )
    .expect("service paths")
}

fn run_crash_child(root: &Path) {
    let paths = paths(root);
    let binary = root.join("bin/podwayd");
    fs::create_dir_all(binary.parent().expect("binary parent")).expect("create binary parent");
    fs::write(&binary, b"#!/bin/sh\nexit 0\n").expect("write binary");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("chmod binary");
    let clock = FixedServiceClockV1::new(UnixMillis::new(1));
    let runner = MacosServiceCommandRunnerV1::new(
        CrashAtDurabilityBoundary,
        SuccessfulLaunchctl,
        clock,
        501,
    )
    .expect("runner");
    if std::env::var_os("PODWAY_SERVICE_CRASH_MODE").is_some_and(|mode| mode == "remove") {
        fs::create_dir_all(
            paths
                .launch_agent_path()
                .as_path()
                .parent()
                .expect("plist parent"),
        )
        .expect("create plist parent");
        fs::create_dir_all(
            paths
                .metadata_index_path()
                .as_path()
                .parent()
                .expect("metadata parent"),
        )
        .expect("create metadata parent");
        fs::write(paths.launch_agent_path().as_path(), b"complete plist").expect("write plist");
        fs::write(paths.metadata_index_path().as_path(), b"complete metadata")
            .expect("write metadata");
        let _ = runner.run(podway_service::ServiceCommandV1::Uninstall {
            requested_at: UnixMillis::new(1),
            paths,
        });
    } else {
        let spec = InstallSpecV1::new(
            LocalPlatformPathV1::new(binary).expect("absolute binary"),
            ServiceLabelV1::podwayd(),
            paths,
        );
        let _ = runner.run(podway_service::ServiceCommandV1::Install {
            requested_at: UnixMillis::new(1),
            spec,
        });
    }
    panic!("failpoint must terminate the child before the durable boundary");
}

#[test]
fn atomic_service_publication_crash_child_leaves_no_partial_state() {
    if let Some(root) = std::env::var_os("PODWAY_SERVICE_CRASH_CHILD_ROOT") {
        run_crash_child(Path::new(&root));
        return;
    }

    let root = unique_root();
    let child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "atomic_service_publication_crash_child_leaves_no_partial_state",
            "--nocapture",
        ])
        .env("PODWAY_SERVICE_CRASH_CHILD_ROOT", &root)
        .env("PODWAY_SERVICE_CRASH_MODE", "publish")
        .status()
        .expect("spawn crash child");
    assert_eq!(
        child.code(),
        Some(86),
        "failpoint must prove abrupt child termination"
    );

    let paths = paths(&root);
    assert!(
        !paths.launch_agent_path().as_path().exists(),
        "atomic publication crash must not expose a partial plist"
    );
    assert!(
        !paths.metadata_index_path().as_path().exists(),
        "atomic publication crash must not expose partial metadata"
    );
    fs::remove_dir_all(root).expect("remove fixture");
}
#[test]
fn service_removal_crash_child_preserves_complete_prior_state() {
    if let Some(root) = std::env::var_os("PODWAY_SERVICE_CRASH_CHILD_ROOT") {
        run_crash_child(Path::new(&root));
        return;
    }

    let root = unique_root();
    let child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "service_removal_crash_child_preserves_complete_prior_state",
            "--nocapture",
        ])
        .env("PODWAY_SERVICE_CRASH_CHILD_ROOT", &root)
        .env("PODWAY_SERVICE_CRASH_MODE", "remove")
        .status()
        .expect("spawn crash child");
    assert_eq!(
        child.code(),
        Some(87),
        "failpoint must prove abrupt child termination"
    );

    let paths = paths(&root);
    assert_eq!(
        fs::read(paths.launch_agent_path().as_path()).expect("complete plist remains"),
        b"complete plist"
    );
    assert_eq!(
        fs::read(paths.metadata_index_path().as_path()).expect("complete metadata remains"),
        b"complete metadata"
    );
    fs::remove_dir_all(root).expect("remove fixture");
}
fn run_bootstrap_crash_child(root: &Path) {
    let paths = paths(root);
    let binary = root.join("bin/podwayd");
    fs::create_dir_all(binary.parent().expect("binary parent")).expect("create binary parent");
    fs::write(&binary, b"#!/bin/sh\nexit 0\n").expect("write binary");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("chmod binary");

    let clock = FixedServiceClockV1::new(UnixMillis::new(1));
    let runner = MacosServiceCommandRunnerV1::new(
        StdServiceFilesystemV1,
        CrashAfterBootstrapLaunchctl {
            marker: root.join("bootstrap-side-effect"),
        },
        clock,
        501,
    )
    .expect("runner");
    let manager = ServiceManagerV1::new(runner, clock, paths.clone());
    let _ = manager.install(install_spec(&binary, &paths));
    panic!("bootstrap crash runner must terminate the child");
}

#[test]
fn bootstrap_side_effect_crash_child_reconciles_to_one_installed_state() {
    if let Some(root) = std::env::var_os("PODWAY_SERVICE_BOOTSTRAP_CRASH_CHILD_ROOT") {
        run_bootstrap_crash_child(Path::new(&root));
        return;
    }

    let root = unique_root();
    let child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "bootstrap_side_effect_crash_child_reconciles_to_one_installed_state",
            "--nocapture",
        ])
        .env("PODWAY_SERVICE_BOOTSTRAP_CRASH_CHILD_ROOT", &root)
        .status()
        .expect("spawn bootstrap crash child");
    assert_eq!(
        child.code(),
        Some(88),
        "bootstrap failpoint must prove abrupt child termination"
    );

    let paths = paths(&root);
    let binary = root.join("bin/podwayd");
    let plist_before_retry =
        fs::read(paths.launch_agent_path().as_path()).expect("complete plist after bootstrap");
    assert!(
        plist_before_retry
            .windows(SERVICE_LABEL_V1.len())
            .any(|window| window == SERVICE_LABEL_V1.as_bytes()),
        "the published plist must be complete and compatible"
    );
    assert!(
        !paths.metadata_index_path().as_path().exists(),
        "child must crash before metadata publication"
    );
    let marker = root.join("bootstrap-side-effect");
    assert_eq!(
        fs::read(&marker).expect("durable bootstrap side-effect marker"),
        b"bootstrap-completed\n"
    );

    let clock = FixedServiceClockV1::new(UnixMillis::new(2));
    let runner = MacosServiceCommandRunnerV1::new(
        StdServiceFilesystemV1,
        ReconciliationLaunchctl {
            marker: marker.clone(),
        },
        clock,
        501,
    )
    .expect("reconciliation runner");
    let manager = ServiceManagerV1::new(runner, clock, paths.clone());
    assert_eq!(
        manager
            .install(install_spec(&binary, &paths))
            .expect("reconciled install")
            .kind(),
        ServiceOutcomeKindV1::ChangedV1
    );
    assert_eq!(
        fs::read(paths.launch_agent_path().as_path()).expect("plist after reconciliation"),
        plist_before_retry,
        "reconciliation must retain the one compatible plist state"
    );
    assert!(paths.metadata_index_path().as_path().exists());
    assert_eq!(
        fs::read(&marker).expect("single bootstrap side-effect marker"),
        b"bootstrap-completed\n",
        "reconciliation must not duplicate the completed bootstrap side effect"
    );
    assert_eq!(
        manager
            .install(install_spec(&binary, &paths))
            .expect("idempotent install after reconciliation")
            .kind(),
        ServiceOutcomeKindV1::AlreadyInDesiredStateV1
    );

    fs::remove_dir_all(root).expect("remove fixture");
}
