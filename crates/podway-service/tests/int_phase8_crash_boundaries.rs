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
};

use podway_core::UnixMillis;
use podway_service::{
    DaemonContractVerifierV1, DurabilityFailpointV1, FixedServiceClockV1, InstallSpecV1,
    LaunchctlOutputV1, LaunchctlRunnerV1, LocalPlatformPathV1,
    MacosServiceCommandRunnerV1 as ProductionMacosServiceCommandRunnerV1, SERVICE_LABEL_V1,
    ServiceClockV1, ServiceCommandRunnerV1, ServiceErrorV1, ServiceFilesystemErrorV1,
    ServiceFilesystemV1, ServiceLabelV1, ServiceManagerContractV1, ServiceManagerV1,
    ServiceOutcomeKindV1, ServicePathErrorV1, ServiceRuntimePathsV1, ServiceStatusV1,
    StdServiceFilesystemV1,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug)]
struct MatchingDaemonContractVerifierV1;

impl DaemonContractVerifierV1 for MatchingDaemonContractVerifierV1 {
    fn verify(&self, _: &Path, _: &str, _: &str) -> Result<(), ServiceErrorV1> {
        Ok(())
    }
}

struct MacosServiceCommandRunnerV1;

impl MacosServiceCommandRunnerV1 {
    #[allow(clippy::new_ret_no_self)]
    fn new<F, L, C>(
        filesystem: F,
        launchctl: L,
        clock: C,
        user_id: u32,
    ) -> Result<
        ProductionMacosServiceCommandRunnerV1<F, L, C, MatchingDaemonContractVerifierV1>,
        ServicePathErrorV1,
    >
    where
        F: ServiceFilesystemV1,
        L: LaunchctlRunnerV1,
        C: ServiceClockV1,
    {
        ProductionMacosServiceCommandRunnerV1::new_with_contract_verifier(
            filesystem,
            launchctl,
            clock,
            user_id,
            MatchingDaemonContractVerifierV1,
        )
    }
}

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
type PublicationBytes = (Vec<u8>, Vec<u8>);

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
    fn read_file_bounded(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, ServiceFilesystemErrorV1> {
        StdServiceFilesystemV1.read_file_bounded(path, maximum_bytes)
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
    fn list_directory_bounded(
        &self,
        path: &Path,
        maximum_entries: usize,
    ) -> Result<Vec<PathBuf>, ServiceFilesystemErrorV1> {
        StdServiceFilesystemV1.list_directory_bounded(path, maximum_entries)
    }

    fn remove_directory(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        StdServiceFilesystemV1.remove_directory(path)
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
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        if arguments
            .first()
            .is_some_and(|argument| argument == "print")
        {
            return Ok(LaunchctlOutputV1 {
                exit_status: 0,
                stdout: format!(
                    "{} = {{\n\tpid = 123\n}}\n",
                    arguments.get(1).expect("print target")
                ),
                stderr: String::new(),
            });
        }
        Ok(LaunchctlOutputV1::success())
    }
}
struct OrphanLoadedLaunchctl {
    loaded: Arc<AtomicU64>,
    bootouts: Arc<AtomicU64>,
    bootstraps: Arc<AtomicU64>,
}

impl LaunchctlRunnerV1 for OrphanLoadedLaunchctl {
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        match arguments.first().map(String::as_str) {
            Some("print") if self.loaded.load(Ordering::SeqCst) != 0 => Ok(LaunchctlOutputV1 {
                exit_status: 0,
                stdout: format!(
                    "{} = {{\n\tpid = 123\n}}\n",
                    arguments.get(1).expect("print target")
                ),
                stderr: String::new(),
            }),
            Some("print") => Ok(LaunchctlOutputV1 {
                exit_status: 113,
                stdout: String::new(),
                stderr: format!(
                    "Bad request.\nCould not find service \"{SERVICE_LABEL_V1}\" in domain for user gui: 501"
                ),
            }),
            Some("bootout") => {
                self.bootouts.fetch_add(1, Ordering::SeqCst);
                self.loaded.store(0, Ordering::SeqCst);
                Ok(LaunchctlOutputV1::success())
            }
            Some("bootstrap") => {
                self.bootstraps.fetch_add(1, Ordering::SeqCst);
                self.loaded.store(1, Ordering::SeqCst);
                Ok(LaunchctlOutputV1::success())
            }
            _ => Ok(LaunchctlOutputV1::success()),
        }
    }
}

#[test]
fn install_replaces_an_orphan_loaded_label_before_publishing_a_receipt() {
    let root = unique_root();
    let paths = paths(&root);
    let binary = binary(&root, "podwayd");
    let launchctl = OrphanLoadedLaunchctl {
        loaded: Arc::new(AtomicU64::new(1)),
        bootouts: Arc::new(AtomicU64::new(0)),
        bootstraps: Arc::new(AtomicU64::new(0)),
    };
    let bootouts = launchctl.bootouts.clone();
    let bootstraps = launchctl.bootstraps.clone();
    let clock = FixedServiceClockV1::new(UnixMillis::new(1));
    let manager = ServiceManagerV1::new(
        MacosServiceCommandRunnerV1::new(StdServiceFilesystemV1, launchctl, clock, 501)
            .expect("runner"),
        clock,
        paths.clone(),
    );

    manager
        .install(install_spec(&binary, &paths))
        .expect("orphan replacement install");
    assert_eq!(bootouts.load(Ordering::SeqCst), 1);
    assert_eq!(bootstraps.load(Ordering::SeqCst), 1);
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(paths.metadata_index_path().as_path()).expect("receipt"))
            .expect("parse receipt");
    assert_eq!(receipt["publication_state"], "receipt_durable");
    fs::remove_dir_all(root).expect("remove fixture");
}
#[test]
fn already_loaded_bootstrap_requires_exact_documented_bytes() {
    let exact = LaunchctlOutputV1 {
        exit_status: 5,
        stdout: String::new(),
        stderr: "Bootstrap failed: 5: Input/output error".to_owned(),
    };
    assert!(exact.already_loaded_bootstrap());
    for mut near_match in [
        LaunchctlOutputV1 {
            exit_status: 5,
            stdout: String::new(),
            stderr: "Bootstrap failed: 5: Input/output error\n".to_owned(),
        },
        LaunchctlOutputV1 {
            exit_status: 5,
            stdout: String::new(),
            stderr: " Bootstrap failed: 5: Input/output error".to_owned(),
        },
        LaunchctlOutputV1 {
            exit_status: 5,
            stdout: "unexpected".to_owned(),
            stderr: "Bootstrap failed: 5: Input/output error".to_owned(),
        },
        LaunchctlOutputV1 {
            exit_status: 5,
            stdout: String::new(),
            stderr: "bootstrap failed: 5: Input/output error".to_owned(),
        },
        LaunchctlOutputV1 {
            exit_status: 0,
            stdout: String::new(),
            stderr: "Bootstrap failed: 5: Input/output error".to_owned(),
        },
    ] {
        assert!(!near_match.already_loaded_bootstrap());
        near_match.stderr.push(' ');
        assert!(!near_match.already_loaded_bootstrap());
    }
}

struct CrashAfterBootstrapLaunchctl {
    marker: PathBuf,
}

impl LaunchctlRunnerV1 for CrashAfterBootstrapLaunchctl {
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        if arguments
            .first()
            .is_some_and(|argument| argument == "print")
        {
            return Ok(LaunchctlOutputV1 {
                exit_status: 113,
                stdout: String::new(),
                stderr: format!(
                    "Bad request.\nCould not find service \"{SERVICE_LABEL_V1}\" in domain for user gui: 501"
                ),
            });
        }
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
            return Ok(LaunchctlOutputV1 {
                exit_status: 5,
                stdout: String::new(),
                stderr: "Bootstrap failed: 5: Input/output error".to_owned(),
            });
        }
        if arguments
            .first()
            .is_some_and(|argument| argument == "print")
        {
            return Ok(LaunchctlOutputV1 {
                exit_status: 0,
                stdout: "gui/501/dev.podway.podwayd = {\npid = 123\n".to_owned(),
                stderr: String::new(),
            });
        }
        Ok(LaunchctlOutputV1::success())
    }
}

fn install_spec(binary: &Path, paths: &ServiceRuntimePathsV1) -> InstallSpecV1 {
    InstallSpecV1::new(
        LocalPlatformPathV1::new(binary).expect("absolute binary"),
        ServiceLabelV1::podwayd(),
        paths.clone(),
        "podway",
        format!("sha256:{}", "a".repeat(64)),
    )
}

fn unique_root() -> PathBuf {
    let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!("/tmp/pw8-{}-{sequence}", std::process::id()))
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

fn binary(root: &Path, name: &str) -> PathBuf {
    let binary = root.join(format!("bin/{name}"));
    fs::create_dir_all(binary.parent().expect("binary parent")).expect("create binary parent");
    let mut bytes = vec![0_u8; 40];
    bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
    bytes[4..8].copy_from_slice(&0x0100_000c_u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&1_u32.to_le_bytes());
    bytes[20..24].copy_from_slice(&8_u32.to_le_bytes());
    bytes[32..36].copy_from_slice(&0x32_u32.to_le_bytes());
    bytes[36..40].copy_from_slice(&8_u32.to_le_bytes());
    bytes.extend_from_slice(name.as_bytes());
    fs::write(&binary, bytes).expect("write binary");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("chmod binary");
    binary
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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

fn assert_mode_0600(path: &Path, description: &str) {
    assert_eq!(
        fs::metadata(path).expect(description).permissions().mode() & 0o777,
        0o600,
        "{description} must remain private"
    );
}

fn assert_service_temporaries_are_bounded_and_private(paths: &ServiceRuntimePathsV1) {
    for path in [
        paths.launch_agent_path().as_path(),
        paths.metadata_index_path().as_path(),
    ] {
        let parent = path.parent().expect("service file parent");
        let temporary_prefix = format!(
            ".{}.",
            path.file_name()
                .expect("service file name")
                .to_string_lossy()
        );
        let mut temporary_count = 0;
        for entry in fs::read_dir(parent).expect("read service file parent") {
            let entry = entry.expect("directory entry");
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&temporary_prefix) && name.ends_with(".tmp") {
                temporary_count += 1;
                let metadata = entry.metadata().expect("temporary metadata");
                assert!(
                    metadata.is_file(),
                    "temporary service state must be regular"
                );
                assert_eq!(
                    metadata.permissions().mode() & 0o777,
                    0o600,
                    "temporary service state must remain private"
                );
                assert!(
                    metadata.len() <= 256 * 1024,
                    "temporary service state must remain bounded"
                );
            }
        }
        assert!(
            temporary_count <= 1,
            "one interrupted write may leave at most one temporary per destination"
        );
    }
}
fn expected_publication_generations(root: &Path) -> (PublicationBytes, PublicationBytes) {
    let paths = paths(root);
    let old_binary = binary(root, "podwayd-old");
    let new_binary = binary(root, "podwayd-new");
    let old_clock = FixedServiceClockV1::new(UnixMillis::new(1));
    let old_runner = MacosServiceCommandRunnerV1::new(
        StdServiceFilesystemV1,
        SuccessfulLaunchctl,
        old_clock,
        501,
    )
    .expect("old-state runner");
    ServiceManagerV1::new(old_runner, old_clock, paths.clone())
        .install(install_spec(&old_binary, &paths))
        .expect("publish complete old state");
    let old = (
        fs::read(paths.launch_agent_path().as_path()).expect("canonical old plist"),
        fs::read(paths.metadata_index_path().as_path()).expect("canonical old metadata"),
    );

    let new_clock = FixedServiceClockV1::new(UnixMillis::new(2));
    let new_runner = MacosServiceCommandRunnerV1::new(
        StdServiceFilesystemV1,
        SuccessfulLaunchctl,
        new_clock,
        501,
    )
    .expect("new-state runner");
    ServiceManagerV1::new(new_runner, new_clock, paths.clone())
        .install(install_spec(&new_binary, &paths))
        .expect("publish complete new state");
    let new = (
        fs::read(paths.launch_agent_path().as_path()).expect("canonical new plist"),
        fs::read(paths.metadata_index_path().as_path()).expect("canonical new metadata"),
    );

    fs::write(paths.launch_agent_path().as_path(), &old.0).expect("restore canonical old plist");
    fs::write(paths.metadata_index_path().as_path(), &old.1)
        .expect("restore canonical old metadata");
    fs::set_permissions(
        paths.launch_agent_path().as_path(),
        fs::Permissions::from_mode(0o600),
    )
    .expect("restore plist mode");
    fs::set_permissions(
        paths.metadata_index_path().as_path(),
        fs::Permissions::from_mode(0o600),
    )
    .expect("restore metadata mode");
    (old, new)
}

fn assert_complete_publication_bytes(observed: &PublicationBytes, point: &str) {
    let plist = std::str::from_utf8(&observed.0)
        .unwrap_or_else(|error| panic!("{point} left a non-UTF-8 plist: {error}"));
    assert!(
        plist.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")
            && plist.ends_with("</plist>\n"),
        "{point} left a truncated plist"
    );
    let metadata: serde_json::Value = serde_json::from_slice(&observed.1)
        .unwrap_or_else(|error| panic!("{point} left truncated or malformed metadata: {error}"));
    let object = metadata
        .as_object()
        .unwrap_or_else(|| panic!("{point} metadata must be a complete object"));
    for field in [
        "version",
        "label",
        "daemon_binary",
        "daemon_identity",
        "installed_at",
        "updated_at",
        "publication_state",
        "generation",
    ] {
        assert!(
            object.contains_key(field),
            "{point} metadata omitted canonical field {field}"
        );
    }
    let generation = metadata["generation"]
        .as_str()
        .unwrap_or_else(|| panic!("{point} metadata generation must be a string"));
    let daemon_identity = metadata["daemon_identity"]
        .as_str()
        .unwrap_or_else(|| panic!("{point} daemon identity must be a string"));
    assert!(
        plist.contains("<key>PodwayGeneration</key>")
            && plist.contains("<key>PodwayDaemonSha256</key>")
            && generation.len() == 64
            && daemon_identity.len() == 64,
        "{point} must leave complete bounded generation and daemon identity fields"
    );
}

fn assert_crash_publication_state(
    paths: &ServiceRuntimePathsV1,
    observed: &PublicationBytes,
    old: &PublicationBytes,
    new: &PublicationBytes,
    point: &str,
) {
    assert_complete_publication_bytes(observed, point);
    let clock = FixedServiceClockV1::new(UnixMillis::new(3));
    let runner =
        MacosServiceCommandRunnerV1::new(StdServiceFilesystemV1, SuccessfulLaunchctl, clock, 501)
            .expect("crash-state observer");
    let manager = ServiceManagerV1::new(runner, clock, paths.clone());

    if observed == old || observed == new {
        manager
            .status()
            .expect("complete canonical publication must authenticate through status");
        return;
    }

    let error = manager
        .status()
        .expect_err("incoherent publication must be rejected before retry");
    assert!(
        matches!(
            error,
            ServiceErrorV1::OperationFailureV1 { source, .. }
                if matches!(source.as_ref(), ServiceErrorV1::InvalidMetadataV1 { .. })
        ),
        "{point} must reject an incoherent publication as invalid metadata"
    );
}

fn run_publication_crash_child(root: &Path, destination: &str, point: &str, write_invocation: u64) {
    let paths = paths(root);
    let old_binary = binary(root, "podwayd-old");
    let new_binary = binary(root, "podwayd-new");
    let old_clock = FixedServiceClockV1::new(UnixMillis::new(1));
    let old_runner = MacosServiceCommandRunnerV1::new(
        StdServiceFilesystemV1,
        SuccessfulLaunchctl,
        old_clock,
        501,
    )
    .expect("old-state runner");
    ServiceManagerV1::new(old_runner, old_clock, paths.clone())
        .install(install_spec(&old_binary, &paths))
        .expect("publish complete old state");

    let destination = match destination {
        "plist" => paths.launch_agent_path().as_path(),
        "metadata" => paths.metadata_index_path().as_path(),
        _ => panic!("unknown durability destination"),
    };
    StdServiceFilesystemV1::inject_durability_failpoint_for_testing(
        destination,
        write_invocation,
        failpoint(point),
    );
    let clock = FixedServiceClockV1::new(UnixMillis::new(2));
    let runner =
        MacosServiceCommandRunnerV1::new(StdServiceFilesystemV1, SuccessfulLaunchctl, clock, 501)
            .expect("replacement runner");
    let _ = runner.run(podway_service::ServiceCommandV1::Install {
        requested_at: UnixMillis::new(2),
        spec: install_spec(&new_binary, &paths),
    });
    panic!("durability failpoint must terminate the child");
}

#[test]
fn atomic_service_publication_crash_child_leaves_no_partial_state() {
    if let (Some(root), Some(destination), Some(point), Some(write_invocation)) = (
        std::env::var_os("PODWAY_SERVICE_CRASH_CHILD_ROOT"),
        std::env::var_os("PODWAY_SERVICE_DURABILITY_DESTINATION"),
        std::env::var_os("PODWAY_SERVICE_DURABILITY_FAILPOINT"),
        std::env::var_os("PODWAY_SERVICE_DURABILITY_WRITE"),
    ) {
        run_publication_crash_child(
            Path::new(&root),
            &destination.to_string_lossy(),
            &point.to_string_lossy(),
            write_invocation
                .to_string_lossy()
                .parse()
                .expect("durability write invocation"),
        );
        return;
    }

    for destination in ["plist", "metadata"] {
        let write_invocations = if destination == "metadata" {
            1..=2
        } else {
            1..=1
        };
        for write_invocation in write_invocations {
            for point in [
                "after-temporary-write",
                "after-file-sync-mode",
                "before-rename",
                "after-rename",
                "after-parent-sync",
            ] {
                let root = unique_root();
                let (old, new) = expected_publication_generations(&root);
                let child = Command::new(std::env::current_exe().expect("test executable"))
                    .args([
                        "--exact",
                        "int_phase8_crash_boundaries::atomic_service_publication_crash_child_leaves_no_partial_state",
                        "--nocapture",
                    ])
                    .env("PODWAY_SERVICE_CRASH_CHILD_ROOT", &root)
                    .env("PODWAY_SERVICE_DURABILITY_FAILPOINT", point)
                    .env("PODWAY_SERVICE_DURABILITY_DESTINATION", destination)
                    .env(
                        "PODWAY_SERVICE_DURABILITY_WRITE",
                        write_invocation.to_string(),
                    )
                    .output()
                    .expect("spawn crash child");
                assert_eq!(
                    child.status.code(),
                    Some(86),
                    "{destination}/{write_invocation}/{point} must terminate at its selected real boundary: stdout={} stderr={}",
                    String::from_utf8_lossy(&child.stdout),
                    String::from_utf8_lossy(&child.stderr),
                );
                let paths = paths(&root);
                let plist = paths.launch_agent_path().as_path();
                let metadata = paths.metadata_index_path().as_path();
                let observed = (
                    fs::read(plist).expect("complete old or new plist"),
                    fs::read(metadata).expect("complete old or new metadata"),
                );
                assert_crash_publication_state(&paths, &observed, &old, &new, point);
                assert_mode_0600(plist, "replacement plist mode");
                assert_mode_0600(metadata, "replacement metadata mode");
                assert_service_temporaries_are_bounded_and_private(&paths);
                let clock = FixedServiceClockV1::new(UnixMillis::new(3));
                let runner = MacosServiceCommandRunnerV1::new(
                    StdServiceFilesystemV1,
                    SuccessfulLaunchctl,
                    clock,
                    501,
                )
                .expect("retry runner");
                let manager = ServiceManagerV1::new(runner, clock, paths.clone());
                manager
                    .install(install_spec(&root.join("bin/podwayd-new"), &paths))
                    .expect("retry convergence");
                match manager
                    .status()
                    .expect("retry must restore a coherent publication")
                {
                    ServiceStatusV1::RunningV1(running) => {
                        let repaired = running
                            .metadata()
                            .expect("retry must publish authenticated metadata");
                        let installed = repaired.daemon_binary();
                        assert_eq!(
                            installed,
                            root.join("bin/podwayd-new"),
                            "{point} retry must retain the selected daemon path"
                        );
                        assert_eq!(
                            repaired.daemon_identity(),
                            sha256_hex(&fs::read(installed).expect("selected replacement daemon")),
                            "{point} retry must authenticate the selected replacement bytes"
                        );
                    }
                    other => panic!("{point} retry must report running, got {other:?}"),
                }
                assert_ne!(
                    fs::read(plist).expect("converged plist"),
                    old.0,
                    "{point} retry must replace the old plist"
                );
                assert_ne!(
                    fs::read(metadata).expect("converged metadata"),
                    old.1,
                    "{point} retry must replace the old metadata"
                );
                assert_mode_0600(plist, "converged plist mode");
                assert_mode_0600(metadata, "converged metadata mode");
                fs::remove_dir_all(root).expect("remove fixture");
            }
        }
    }
}

fn run_removal_crash_child(root: &Path) {
    let paths = paths(root);
    let binary = binary(root, "podwayd");
    let setup = MacosServiceCommandRunnerV1::new(
        StdServiceFilesystemV1,
        SuccessfulLaunchctl,
        FixedServiceClockV1::new(UnixMillis::new(1)),
        501,
    )
    .expect("setup runner");
    setup
        .run(podway_service::ServiceCommandV1::Install {
            requested_at: UnixMillis::new(1),
            spec: InstallSpecV1::new(
                LocalPlatformPathV1::new(&binary).expect("fixture binary path"),
                ServiceLabelV1::podwayd(),
                paths.clone(),
                "podway",
                format!("sha256:{}", "a".repeat(64)),
            ),
        })
        .expect("complete service state must be installed before removal crash");
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
            "int_phase8_crash_boundaries::service_removal_crash_child_preserves_complete_prior_state",
            "--nocapture",
        ])
        .env("PODWAY_SERVICE_REMOVAL_CRASH_CHILD_ROOT", &root)
        .output()
        .expect("spawn crash child");
    assert_eq!(
        child.status.code(),
        Some(87),
        "child must crash between declared removals: stdout={} stderr={}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr),
    );

    let paths = paths(&root);
    assert!(
        !paths.launch_agent_path().as_path().exists(),
        "first actual removal completed"
    );
    let remaining_metadata: serde_json::Value = serde_json::from_slice(
        &fs::read(paths.metadata_index_path().as_path()).expect("unremoved complete metadata"),
    )
    .expect("unremoved metadata must remain authenticated JSON");
    assert_eq!(remaining_metadata["artifact_role"], "production_daemon");
    assert_eq!(remaining_metadata["publication_state"], "receipt_durable");
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
    let binary = binary(root, "podwayd");
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
    let first = manager.install(install_spec(&binary, &paths));
    // The crash child shares the production singleton lock with concurrent service tests.
    // Retry only lock contention; every other pre-bootstrap failure remains immediately visible.
    if first.as_ref().is_err_and(lifecycle_lock_timed_out) {
        let retry = manager.install(install_spec(&binary, &paths));
        panic!("bootstrap crash runner must terminate the child after lock retry: {retry:?}");
    }
    panic!("bootstrap crash runner must terminate the child: {first:?}");
}

fn lifecycle_lock_timed_out(error: &ServiceErrorV1) -> bool {
    matches!(
        error,
        ServiceErrorV1::OperationFailureV1 { source, .. }
            if matches!(source.as_ref(), ServiceErrorV1::TimeoutV1 { .. })
    )
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
            "int_phase8_crash_boundaries::bootstrap_side_effect_crash_child_reconciles_to_one_installed_state",
            "--nocapture",
        ])
        .env("PODWAY_SERVICE_BOOTSTRAP_CRASH_CHILD_ROOT", &root)
        .output()
        .expect("spawn bootstrap crash child");
    assert_eq!(
        child.status.code(),
        Some(88),
        "bootstrap failpoint must terminate the child: stdout={} stderr={}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr),
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
    let prepared_metadata = fs::read(paths.metadata_index_path().as_path())
        .expect("prepared metadata must survive the bootstrap-side-effect crash");
    let prepared_metadata: serde_json::Value = serde_json::from_slice(&prepared_metadata)
        .expect("prepared metadata must remain parseable");
    assert_eq!(prepared_metadata["publication_state"], "prepared");
    let prepared_generation = prepared_metadata["generation"]
        .as_str()
        .expect("prepared generation")
        .to_owned();
    assert!(
        std::str::from_utf8(&plist_before_retry)
            .expect("prepared plist UTF-8")
            .contains(&prepared_generation),
        "loaded prepared plist must authenticate the prepared receipt generation"
    );
    let prepared_status = MacosServiceCommandRunnerV1::new(
        StdServiceFilesystemV1,
        SuccessfulLaunchctl,
        FixedServiceClockV1::new(UnixMillis::new(2)),
        501,
    )
    .expect("prepared status runner");
    assert!(matches!(
        ServiceManagerV1::new(
            prepared_status,
            FixedServiceClockV1::new(UnixMillis::new(2)),
            paths.clone(),
        )
        .status(),
        Err(ServiceErrorV1::OperationFailureV1 { source, .. })
            if matches!(source.as_ref(), ServiceErrorV1::InvalidMetadataV1 { .. })
    ));
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
        plist_before_retry,
        "receipt publication state must not alter the loaded plist generation"
    );
    let receipt = fs::read(paths.metadata_index_path().as_path())
        .expect("receipt metadata must exist after reconciliation");
    let receipt: serde_json::Value =
        serde_json::from_slice(&receipt).expect("receipt metadata must remain parseable");
    assert_eq!(receipt["publication_state"], "receipt_durable");
    assert_eq!(
        receipt["generation"], prepared_generation,
        "prepared and receipt-durable metadata must authenticate the same plist"
    );
    assert_eq!(
        fs::read(&marker).expect("one bootstrap side effect"),
        b"bootstrap-completed\n"
    );
    assert_eq!(
        bootstrap_attempts.load(Ordering::SeqCst),
        0,
        "loaded reconciliation must not issue a duplicate bootstrap"
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
        0,
        "idempotent install must not issue another bootstrap"
    );
    fs::remove_dir_all(root).expect("remove fixture");
}
#[test]
fn atomic_publication_survives_young_temporary_collisions_without_reclaiming_foreign_entries() {
    let root = unique_root();
    let parent = root.join("service");
    fs::create_dir_all(&parent).expect("create fixture directory");
    let destination = parent.join("service.json");
    let process_id = std::process::id();
    for sequence in 0..96_u64 {
        fs::write(
            parent.join(format!(
                ".service.json.{process_id}.123456789.{}.{sequence}.tmp",
                123456789_u64 ^ sequence
            )),
            b"young temporary",
        )
        .expect("create young owned temporary");
    }
    let near_match = parent.join(format!(".service.json.{process_id}.1.2.3.tmp.bak"));
    fs::write(&near_match, b"foreign").expect("create near-match temporary");
    let symlink = parent.join(format!(".service.json.{process_id}.1.2.3.tmp"));
    std::os::unix::fs::symlink(&near_match, &symlink).expect("create foreign symlink");
    let stale = parent.join(format!(".service.json.{process_id}.1.5.4.tmp"));
    fs::write(&stale, b"stale temporary").expect("create stale owned temporary");
    let touch_status = Command::new("/usr/bin/touch")
        .args(["-t", "200001010000"])
        .arg(&stale)
        .status()
        .expect("age stale temporary");
    assert!(touch_status.success(), "age stale temporary");

    StdServiceFilesystemV1
        .write_atomically(&destination, b"accepted generation", 0o600)
        .expect("fresh collision-resistant temporary must publish");
    assert_eq!(
        fs::read(&destination).expect("published bytes"),
        b"accepted generation"
    );
    assert!(
        near_match.exists(),
        "near-match temporary must be preserved"
    );
    assert!(symlink.exists(), "symlink temporary must be preserved");
    assert!(!stale.exists(), "stale owned temporary must be reclaimed");
    let retained_owned = fs::read_dir(&parent)
        .expect("read temporary directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(&format!(".service.json.{process_id}."))
                && name.ends_with(".tmp")
                && entry.file_type().is_ok_and(|kind| kind.is_file())
        })
        .count();
    assert_eq!(
        retained_owned, 32,
        "fresh crash leftovers must be pruned to the bounded retention target"
    );
    fs::remove_dir_all(root).expect("remove fixture");
}
