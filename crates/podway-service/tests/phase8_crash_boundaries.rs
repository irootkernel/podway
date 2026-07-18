use std::{
    fs,
    fs::OpenOptions,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use podway_core::UnixMillis;
use podway_service::{
    DurabilityFailpointV1, FixedServiceClockV1, InstallSpecV1, LaunchctlOutputV1,
    LaunchctlRunnerV1, LocalPlatformPathV1, MacosServiceCommandRunnerV1, SERVICE_LABEL_V1,
    ServiceCommandRunnerV1, ServiceErrorV1, ServiceFilesystemErrorV1, ServiceFilesystemV1,
    ServiceLabelV1, ServiceManagerContractV1, ServiceManagerV1, ServiceOutcomeKindV1,
    ServiceRuntimePathsV1, StdServiceFilesystemV1,
};

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct CrashBetweenDeclaredRemovals {
    removals: AtomicU64,
}

impl ServiceFilesystemV1 for CrashBetweenDeclaredRemovals {
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
        StdServiceFilesystemV1.write_atomically(path, contents, mode)
    }
    fn remove_file(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        StdServiceFilesystemV1.remove_file(path)?;
        if self.removals.fetch_add(1, Ordering::SeqCst) == 0 {
            std::process::exit(87);
        }
        Ok(())
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
                .expect("create exactly one durable bootstrap marker");
            marker
                .write_all(b"bootstrap-completed\n")
                .expect("write bootstrap marker");
            marker.sync_all().expect("sync bootstrap marker");
            std::process::exit(88);
        }
        Ok(LaunchctlOutputV1::success())
    }
}

struct AlreadyBootstrappedLaunchctl {
    marker: PathBuf,
    bootstrap_attempts: Arc<AtomicU64>,
}

impl LaunchctlRunnerV1 for AlreadyBootstrappedLaunchctl {
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        if arguments
            .first()
            .is_some_and(|argument| argument == "bootstrap")
        {
            self.bootstrap_attempts.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                fs::read(&self.marker).expect("durable prior bootstrap marker"),
                b"bootstrap-completed\n",
                "retry may only observe the documented already-bootstrapped state"
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

fn binary(root: &Path) -> PathBuf {
    let binary = root.join("bin/podwayd");
    fs::create_dir_all(binary.parent().expect("binary parent")).expect("create binary parent");
    fs::write(&binary, b"#!/bin/sh\nexit 0\n").expect("write binary");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("chmod binary");
    binary
}

fn failpoint(name: &str) -> DurabilityFailpointV1 {
    match name {
        "after-temporary-write" => DurabilityFailpointV1::AfterTemporaryWrite,
        "after-file-sync-mode" => DurabilityFailpointV1::AfterFileSyncAndMode,
        "before-rename" => DurabilityFailpointV1::BeforeRename,
        "after-rename" => DurabilityFailpointV1::AfterRename,
        "after-parent-sync" => DurabilityFailpointV1::AfterParentDirectorySync,
        _ => panic!("unknown durability failpoint"),
    }
}

fn run_publication_crash_child(root: &Path, point: &str) {
    let paths = paths(root);
    let binary = binary(root);
    StdServiceFilesystemV1::inject_durability_failpoint_for_testing(failpoint(point));
    let clock = FixedServiceClockV1::new(UnixMillis::new(1));
    let runner =
        MacosServiceCommandRunnerV1::new(StdServiceFilesystemV1, SuccessfulLaunchctl, clock, 501)
            .expect("runner");
    let _ = runner.run(podway_service::ServiceCommandV1::Install {
        requested_at: UnixMillis::new(1),
        spec: install_spec(&binary, &paths),
    });
    panic!("durability failpoint must terminate the child");
}

#[test]
fn atomic_service_publication_crash_child_leaves_no_partial_state() {
    if let (Some(root), Some(point)) = (
        std::env::var_os("PODWAY_SERVICE_CRASH_CHILD_ROOT"),
        std::env::var_os("PODWAY_SERVICE_DURABILITY_FAILPOINT"),
    ) {
        run_publication_crash_child(Path::new(&root), &point.to_string_lossy());
        return;
    }

    for point in [
        "after-temporary-write",
        "after-file-sync-mode",
        "before-rename",
        "after-rename",
        "after-parent-sync",
    ] {
        let root = unique_root();
        let child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "atomic_service_publication_crash_child_leaves_no_partial_state",
                "--nocapture",
            ])
            .env("PODWAY_SERVICE_CRASH_CHILD_ROOT", &root)
            .env("PODWAY_SERVICE_DURABILITY_FAILPOINT", point)
            .status()
            .expect("spawn crash child");
        assert_eq!(
            child.code(),
            Some(86),
            "{point} must terminate at its real boundary"
        );

        let paths = paths(&root);
        let plist = paths.launch_agent_path().as_path();
        if plist.exists() {
            let contents = fs::read(plist).expect("published plist");
            assert!(
                contents
                    .windows(SERVICE_LABEL_V1.len())
                    .any(|part| part == SERVICE_LABEL_V1.as_bytes()),
                "{point} may publish only the complete successor"
            );
            assert_eq!(
                fs::metadata(plist)
                    .expect("plist mode")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let binary = binary(&root);
        let clock = FixedServiceClockV1::new(UnixMillis::new(2));
        let runner = MacosServiceCommandRunnerV1::new(
            StdServiceFilesystemV1,
            SuccessfulLaunchctl,
            clock,
            501,
        )
        .expect("retry runner");
        let manager = ServiceManagerV1::new(runner, clock, paths.clone());
        manager
            .install(install_spec(&binary, &paths))
            .expect("retry convergence");
        assert_eq!(
            fs::metadata(plist)
                .expect("converged plist mode")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(paths.metadata_index_path().as_path())
                .expect("converged metadata mode")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }
}

fn run_removal_crash_child(root: &Path) {
    let paths = paths(root);
    let binary = binary(root);
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
    fs::write(paths.metadata_index_path().as_path(), b"complete metadata").expect("write metadata");
    fs::write(root.join("worktree-state"), b"must survive uninstall")
        .expect("write worktree state");
    fs::create_dir_all(paths.log_path().as_path().parent().expect("log parent"))
        .expect("create log parent");
    fs::write(paths.log_path().as_path(), b"must preserve logs").expect("write service log");

    let clock = FixedServiceClockV1::new(UnixMillis::new(1));
    let runner = MacosServiceCommandRunnerV1::new(
        CrashBetweenDeclaredRemovals {
            removals: AtomicU64::new(0),
        },
        SuccessfulLaunchctl,
        clock,
        501,
    )
    .expect("runner");
    let _ = runner.run(podway_service::ServiceCommandV1::Uninstall {
        requested_at: UnixMillis::new(1),
        paths,
    });
    let _ = binary;
    panic!("removal failpoint must terminate after the first declared removal");
}

#[test]
fn service_removal_crash_child_preserves_complete_prior_state() {
    if let Some(root) = std::env::var_os("PODWAY_SERVICE_REMOVAL_CRASH_CHILD_ROOT") {
        run_removal_crash_child(Path::new(&root));
        return;
    }

    let root = unique_root();
    let child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "service_removal_crash_child_preserves_complete_prior_state",
            "--nocapture",
        ])
        .env("PODWAY_SERVICE_REMOVAL_CRASH_CHILD_ROOT", &root)
        .status()
        .expect("spawn crash child");
    assert_eq!(
        child.code(),
        Some(87),
        "child must crash between declared removals"
    );

    let paths = paths(&root);
    assert!(
        !paths.launch_agent_path().as_path().exists(),
        "first actual removal completed"
    );
    assert_eq!(
        fs::read(paths.metadata_index_path().as_path()).expect("unremoved complete metadata"),
        b"complete metadata"
    );
    assert_eq!(
        fs::read(root.join("worktree-state")).expect("worktree state"),
        b"must survive uninstall"
    );
    assert_eq!(
        fs::read(paths.log_path().as_path()).expect("preserved log"),
        b"must preserve logs"
    );

    let clock = FixedServiceClockV1::new(UnixMillis::new(2));
    let runner =
        MacosServiceCommandRunnerV1::new(StdServiceFilesystemV1, SuccessfulLaunchctl, clock, 501)
            .expect("retry runner");
    let manager = ServiceManagerV1::new(runner, clock, paths.clone());
    manager.uninstall().expect("removal retry convergence");
    assert!(!paths.metadata_index_path().as_path().exists());
    assert_eq!(
        fs::read(root.join("worktree-state")).expect("worktree state"),
        b"must survive uninstall"
    );
    assert_eq!(
        fs::read(paths.log_path().as_path()).expect("preserved log"),
        b"must preserve logs"
    );
    fs::remove_dir_all(root).expect("remove fixture");
}

fn run_bootstrap_crash_child(root: &Path) {
    let paths = paths(root);
    let binary = binary(root);
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
        "bootstrap failpoint must terminate the child"
    );

    let paths = paths(&root);
    let binary = root.join("bin/podwayd");
    let plist_before_retry =
        fs::read(paths.launch_agent_path().as_path()).expect("complete plist after bootstrap");
    assert!(
        plist_before_retry
            .windows(SERVICE_LABEL_V1.len())
            .any(|part| part == SERVICE_LABEL_V1.as_bytes())
    );
    assert!(!paths.metadata_index_path().as_path().exists());
    let marker = root.join("bootstrap-side-effect");
    assert_eq!(
        fs::read(&marker).expect("durable bootstrap marker"),
        b"bootstrap-completed\n"
    );

    let clock = FixedServiceClockV1::new(UnixMillis::new(2));
    let launchctl = AlreadyBootstrappedLaunchctl {
        marker: marker.clone(),
        bootstrap_attempts: Arc::new(AtomicU64::new(0)),
    };
    let bootstrap_attempts = launchctl.bootstrap_attempts.clone();
    let runner = MacosServiceCommandRunnerV1::new(StdServiceFilesystemV1, launchctl, clock, 501)
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
        fs::read(paths.launch_agent_path().as_path()).expect("plist after retry"),
        plist_before_retry
    );
    assert!(paths.metadata_index_path().as_path().exists());
    assert_eq!(
        fs::read(&marker).expect("one bootstrap side effect"),
        b"bootstrap-completed\n"
    );
    assert_eq!(
        bootstrap_attempts.load(Ordering::SeqCst),
        1,
        "retry must model exactly one already-bootstrapped observation"
    );
    assert_eq!(
        manager
            .install(install_spec(&binary, &paths))
            .expect("idempotent retry")
            .kind(),
        ServiceOutcomeKindV1::AlreadyInDesiredStateV1
    );
    assert_eq!(
        bootstrap_attempts.load(Ordering::SeqCst),
        1,
        "idempotent install must not issue another bootstrap"
    );
    fs::remove_dir_all(root).expect("remove fixture");
}
