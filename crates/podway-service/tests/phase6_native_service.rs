use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use podway_core::UnixMillis;
use podway_service::{
    FixedServiceClockV1, InstallSpecV1, LaunchctlRunnerV1, LocalPlatformPathV1, LogQueryV1,
    MacosServiceCommandRunnerV1, SERVICE_LOG_MAX_BYTES_V1, SERVICE_METADATA_MAX_BYTES_V1,
    SERVICE_PLIST_MAX_BYTES_V1, ServiceErrorV1, ServiceFilesystemV1, ServiceLabelV1,
    ServiceLogStreamV1, ServiceManagerContractV1, ServiceManagerV1, ServiceOutcomeKindV1,
    ServiceRuntimePathsV1, ServiceStatusV1, StdServiceFilesystemV1, SystemLaunchctlRunnerV1,
    UninstallOptionsV1,
};

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn unique_root() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "podway-phase6-native-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}

fn unique_runtime() -> PathBuf {
    let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!("/tmp/pw6-{}-{sequence}", std::process::id()))
}

fn write_executable(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
    fs::write(path, bytes).expect("write executable fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("chmod executable fixture");
}
fn native_arm64_macho(marker: u8) -> Vec<u8> {
    let mut bytes = vec![0_u8; 40];
    bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
    bytes[4..8].copy_from_slice(&0x0100_000c_u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&1_u32.to_le_bytes());
    bytes[20..24].copy_from_slice(&8_u32.to_le_bytes());
    bytes[32..36].copy_from_slice(&0x32_u32.to_le_bytes());
    bytes[36..40].copy_from_slice(&8_u32.to_le_bytes());
    bytes.push(marker);
    bytes
}

fn spec(binary: &Path, paths: &ServiceRuntimePathsV1) -> InstallSpecV1 {
    InstallSpecV1::new(
        LocalPlatformPathV1::new(binary).expect("absolute binary path"),
        ServiceLabelV1::podwayd(),
        paths.clone(),
    )
}
#[test]
fn bounded_service_reads_reject_symlink_fifo_directory_and_over_limit_without_following() {
    let root = unique_root();
    fs::create_dir_all(&root).expect("fixture root");
    let target = root.join("target");
    fs::write(&target, b"target").expect("target");
    for path in [
        root.join("service.json"),
        root.join("dev.podway.podwayd.plist"),
    ] {
        std::os::unix::fs::symlink(&target, &path).expect("symlink fixture");
        assert!(
            StdServiceFilesystemV1
                .read_file_bounded(&path, SERVICE_METADATA_MAX_BYTES_V1)
                .is_err()
        );
        fs::remove_file(&path).expect("remove symlink");
        fs::create_dir(&path).expect("directory fixture");
        assert!(
            StdServiceFilesystemV1
                .read_file_bounded(&path, SERVICE_METADATA_MAX_BYTES_V1)
                .is_err()
        );
        fs::remove_dir(&path).expect("remove directory");
        assert!(
            Command::new("mkfifo")
                .arg(&path)
                .status()
                .expect("mkfifo")
                .success()
        );
        assert!(
            StdServiceFilesystemV1
                .read_file_bounded(&path, SERVICE_METADATA_MAX_BYTES_V1)
                .is_err()
        );
        fs::remove_file(&path).expect("remove fifo");
    }
    let oversized = root.join("dev.podway.podwayd.plist");
    fs::write(&oversized, vec![b'x'; SERVICE_PLIST_MAX_BYTES_V1 + 1]).expect("oversized plist");
    fs::set_permissions(&oversized, fs::Permissions::from_mode(0o600)).expect("private plist");
    assert!(
        StdServiceFilesystemV1
            .read_file_bounded(&oversized, SERVICE_PLIST_MAX_BYTES_V1)
            .is_err()
    );
    let oversized_metadata = root.join("service.json");
    fs::write(
        &oversized_metadata,
        vec![b'x'; SERVICE_METADATA_MAX_BYTES_V1 + 1],
    )
    .expect("oversized metadata");
    fs::set_permissions(&oversized_metadata, fs::Permissions::from_mode(0o600))
        .expect("private metadata");
    assert!(
        StdServiceFilesystemV1
            .read_file_bounded(&oversized_metadata, SERVICE_METADATA_MAX_BYTES_V1)
            .is_err()
    );
    let outside = unique_root();
    fs::create_dir_all(&outside).expect("outside root");
    let outside_metadata = outside.join("service.json");
    fs::write(&outside_metadata, b"outside metadata").expect("outside metadata");
    let redirected = root.join("redirected");
    std::os::unix::fs::symlink(&outside, &redirected).expect("ancestor symlink");
    assert!(
        StdServiceFilesystemV1
            .read_file_bounded(
                &redirected.join("service.json"),
                SERVICE_METADATA_MAX_BYTES_V1
            )
            .is_err(),
        "bounded reads must reject a symlinked ancestor"
    );
    assert!(
        StdServiceFilesystemV1
            .exists(&redirected.join("service.json"))
            .is_err(),
        "existence checks must reject a symlinked ancestor"
    );
    assert!(
        StdServiceFilesystemV1
            .is_executable(&redirected.join("service.json"))
            .is_err(),
        "executable checks must reject a symlinked ancestor"
    );
    assert_eq!(
        fs::read(&outside_metadata).expect("outside metadata"),
        b"outside metadata"
    );
    fs::remove_dir_all(outside).expect("cleanup outside");
    fs::remove_dir_all(root).expect("cleanup");
}
#[test]
fn service_mutations_reject_symlinked_ancestors_without_touching_outside_sentinel() {
    let root = unique_root();
    let outside = unique_root();
    fs::create_dir_all(&root).expect("fixture root");
    fs::create_dir_all(&outside).expect("outside root");
    let sentinel = outside.join("sentinel");
    fs::write(&sentinel, b"outside bytes").expect("outside sentinel");
    let redirected = root.join("redirected");
    std::os::unix::fs::symlink(&outside, &redirected).expect("ancestor symlink");

    let filesystem = StdServiceFilesystemV1;
    assert!(
        filesystem
            .create_directory(&redirected.join("directory"), 0o700)
            .is_err()
    );
    assert!(
        filesystem
            .write_atomically(&redirected.join("service.json"), b"replacement", 0o600)
            .is_err()
    );

    assert_eq!(
        fs::read(&sentinel).expect("outside sentinel"),
        b"outside bytes"
    );
    assert!(!outside.join("directory").exists());
    assert!(!outside.join("service.json").exists());
    fs::remove_dir_all(root).expect("cleanup root");
    fs::remove_dir_all(outside).expect("cleanup outside");
}

#[test]
fn production_adapters_cover_native_launchagent_lifecycle_without_real_launchctl() {
    let root = unique_root();
    let home = root.join("home");
    let runtime = unique_runtime();
    let paths = ServiceRuntimePathsV1::from_directories(
        home.join("Library/LaunchAgents"),
        home.join("Library/Application Support/Podway"),
        home.join("Library/Logs/Podway"),
        &runtime,
    )
    .expect("fixture service paths");

    let launchctl_log = root.join("launchctl.argv");
    let fake_launchctl = root.join("bin/launchctl");
    let launchctl_state = root.join("launchctl.loaded");
    let first_binary = root.join("bin/podwayd-v1");
    let second_binary = root.join("bin/podwayd-v2");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$1\" in\nbootstrap) printf '%s' 'replaced-before-launchd-resolves-source' > '{}'; chmod 700 '{}'; touch '{}';;\nbootout) rm -f '{}';;\nprint) if [ -f '{}' ]; then printf 'gui/501/dev.podway.podwayd = {{\\npid = 4242\\n'; else printf '%s\\n' 'Bad request.\nCould not find service \"dev.podway.podwayd\" in domain for user gui: 501' >&2; exit 113; fi;;\nesac\nexit 0\n",
        launchctl_log.display(),
        first_binary.display(),
        first_binary.display(),
        launchctl_state.display(),
        launchctl_state.display(),
        launchctl_state.display(),
    );
    write_executable(&fake_launchctl, script.as_bytes());
    write_executable(&first_binary, &native_arm64_macho(0));
    write_executable(&second_binary, &native_arm64_macho(1));

    let clock = FixedServiceClockV1::new(UnixMillis::new(7_000));
    let runner = MacosServiceCommandRunnerV1::new(
        StdServiceFilesystemV1,
        SystemLaunchctlRunnerV1::new(&fake_launchctl),
        clock,
        501,
    )
    .expect("non-root service runner");
    let manager = ServiceManagerV1::new(runner, clock, paths.clone());

    let installed = manager
        .install(spec(&first_binary, &paths))
        .expect("native install");
    assert_eq!(installed.kind(), ServiceOutcomeKindV1::ChangedV1);
    assert_eq!(
        fs::read(&first_binary).expect("bootstrap replacement"),
        b"replaced-before-launchd-resolves-source"
    );
    manager
        .status()
        .expect("receipt must describe staged bootstrap bytes");
    write_executable(&first_binary, &native_arm64_macho(0));
    let plist = fs::read_to_string(paths.launch_agent_path().as_path()).expect("installed plist");
    assert!(plist.contains(".podway-daemons-v1/"));
    assert!(!plist.contains(first_binary.to_str().expect("UTF-8 fixture")));
    assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
    assert!(plist.contains("<key>SuccessfulExit</key>\n    <false/>"));
    assert!(plist.contains("<string>--service</string>"));
    let receipt: serde_json::Value = serde_json::from_slice(
        &fs::read(paths.metadata_index_path().as_path()).expect("installed receipt"),
    )
    .expect("installed receipt JSON");
    assert_eq!(receipt["publication_state"], "receipt_durable");
    assert!(
        plist.contains(receipt["generation"].as_str().expect("receipt generation")),
        "the loaded plist must contain the receipt-durable generation"
    );
    assert!(!plist.contains("/.podway/"));
    assert_eq!(
        fs::metadata(paths.launch_agent_path().as_path())
            .expect("plist metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(paths.runtime_directory().as_path())
            .expect("runtime metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    let idempotent = manager
        .install(spec(&first_binary, &paths))
        .expect("idempotent install");
    assert_eq!(
        idempotent.kind(),
        ServiceOutcomeKindV1::AlreadyInDesiredStateV1
    );
    write_executable(&first_binary, &native_arm64_macho(9));
    manager
        .status()
        .expect("the staged daemon must remain valid after source replacement");
    assert_eq!(
        manager
            .install(spec(&first_binary, &paths))
            .expect("install must stage a replacement source generation")
            .kind(),
        ServiceOutcomeKindV1::ChangedV1
    );
    write_executable(&first_binary, &native_arm64_macho(10));
    assert_eq!(
        manager
            .update(spec(&first_binary, &paths))
            .expect("update must stage same-path replacement")
            .kind(),
        ServiceOutcomeKindV1::ChangedV1
    );
    manager
        .status()
        .expect("repair must restore accepted generation");
    let socket = paths.socket_path().as_path();
    fs::write(socket, b"stale").expect("stale socket fixture");
    let updated = manager
        .update(spec(&second_binary, &paths))
        .expect("binary update");
    assert_eq!(updated.kind(), ServiceOutcomeKindV1::ChangedV1);
    assert!(!socket.exists(), "update must remove a stale socket");
    let updated_plist =
        fs::read_to_string(paths.launch_agent_path().as_path()).expect("updated plist");
    assert!(updated_plist.contains(".podway-daemons-v1/"));
    assert!(!updated_plist.contains(second_binary.to_str().expect("UTF-8 fixture")));

    assert_eq!(
        manager.stop().expect("explicit stop").kind(),
        ServiceOutcomeKindV1::StoppedV1
    );
    std::os::unix::fs::symlink(root.join("missing-socket-target"), socket)
        .expect("dangling stale socket fixture");
    assert!(
        fs::symlink_metadata(socket)
            .expect("dangling socket metadata")
            .file_type()
            .is_symlink(),
        "fixture must be a dangling symlink"
    );
    assert_eq!(
        manager.start().expect("explicit start").kind(),
        ServiceOutcomeKindV1::ChangedV1
    );
    assert!(
        fs::symlink_metadata(socket).is_err(),
        "start must unlink a dangling stale socket before bootstrap"
    );
    fs::write(socket, b"stale-again").expect("second stale socket fixture");
    let log_path = paths.log_path().as_path();
    fs::write(
        log_path,
        vec![b'x'; usize::try_from(SERVICE_LOG_MAX_BYTES_V1 + 1).expect("log size")],
    )
    .expect("oversized log fixture");
    assert_eq!(
        manager.restart().expect("ordered restart").kind(),
        ServiceOutcomeKindV1::ChangedV1
    );
    assert!(!socket.exists(), "restart must remove a stale socket");
    let rotated_log = PathBuf::from(format!("{}.1", log_path.display()));
    assert!(rotated_log.exists(), "restart must rotate an oversized log");
    assert_eq!(
        fs::metadata(log_path).expect("fresh log metadata").len(),
        0,
        "rotation must leave a fresh active log"
    );
    assert_eq!(
        fs::metadata(log_path)
            .expect("fresh log permissions")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    match manager.status().expect("native status") {
        ServiceStatusV1::RunningV1(running) => assert_eq!(running.process_id(), Some(4242)),
        other => panic!("expected running service, got {other:?}"),
    }

    fs::write(log_path, b"one\ntwo\nthree\n").expect("daemon log fixture");
    let location = manager
        .logs(LogQueryV1::new(ServiceLogStreamV1::DaemonV1).with_lines(Some(2)))
        .expect("log location");
    assert_eq!(location.path().as_path(), log_path);
    let registry = paths.workspace_registry_path().as_path();
    fs::write(registry, b"{\"workspaces\":[]}").expect("registry fixture");

    assert_eq!(
        manager.uninstall().expect("preserving uninstall").kind(),
        ServiceOutcomeKindV1::ChangedV1
    );
    assert!(
        registry.exists(),
        "uninstall must preserve workspace registry"
    );
    assert!(log_path.exists(), "ordinary uninstall must preserve logs");
    assert!(!paths.launch_agent_path().as_path().exists());
    assert!(!paths.metadata_index_path().as_path().exists());
    assert!(
        !paths
            .metadata_index_path()
            .as_path()
            .parent()
            .expect("metadata parent")
            .join(".podway-daemons-v1")
            .exists(),
        "uninstall must remove the empty staged daemon directory"
    );

    manager
        .install(spec(&second_binary, &paths))
        .expect("reinstall before purge");
    fs::write(log_path, b"purge me\n").expect("purge log fixture");
    fs::write(format!("{}.1", log_path.display()), b"rotation").expect("rotation fixture");
    let unrelated_log = log_path
        .parent()
        .expect("service log has parent")
        .join("unrelated.log");
    fs::write(&unrelated_log, b"preserve").expect("unrelated log fixture");
    manager
        .uninstall_with_options(UninstallOptionsV1::new(true))
        .expect("purging uninstall");
    assert!(!log_path.exists(), "explicit purge must remove logs");
    assert!(
        !PathBuf::from(format!("{}.1", log_path.display())).exists(),
        "explicit purge must remove only bounded owned rotations"
    );
    assert!(
        unrelated_log.exists(),
        "explicit purge must preserve unrelated sibling logs"
    );
    assert!(
        registry.exists(),
        "purge logs must still preserve workspace registry"
    );

    let argv = fs::read_to_string(&launchctl_log).expect("launchctl argv log");
    assert!(argv.contains("bootstrap gui/501"));
    assert!(argv.contains("bootout gui/501/dev.podway.podwayd"));
    assert!(argv.contains("print gui/501/dev.podway.podwayd"));
    assert!(!argv.contains("system/"));

    fn assert_no_temp_files(path: &Path) {
        if !path.exists() {
            return;
        }
        for entry in fs::read_dir(path).expect("read fixture directory") {
            let entry = entry.expect("fixture entry");
            let child = entry.path();
            assert!(!entry.file_name().to_string_lossy().contains(".tmp"));
            if child.is_dir() {
                assert_no_temp_files(&child);
            }
        }
    }
    assert_no_temp_files(&root);
    fs::remove_dir_all(&runtime).expect("remove native service runtime");
    fs::remove_dir_all(root).expect("remove native service fixture");
}
fn process_exists(process_id: &str) -> bool {
    Command::new("/bin/kill")
        .args(["-0", process_id])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn wait_for_process_exit(process_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while process_exists(process_id) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn system_launchctl_runner_bounds_native_process_groups_and_pipes() {
    let root = unique_root();
    let fake_launchctl = root.join("bin/launchctl");
    write_executable(
        &fake_launchctl,
        br#"#!/bin/sh
case "$1" in
success) printf 'stdout'; printf 'stderr' >&2 ;;
nonzero) printf 'out'; printf 'err' >&2; exit 7 ;;
signal) kill -TERM "$$" ;;
timeout) sleep 5 ;;
timeout-id) echo "$$" > "$2"; sleep 5 ;;
stdout-overflow) while :; do printf x; done ;;
stderr-overflow) while :; do printf y >&2; done ;;
simultaneous)
  i=0
  while [ "$i" -lt 128 ]; do
    printf o
    printf e >&2
    i=$((i + 1))
  done
  ;;
descendant) sleep 5 & echo "$!" > "$2"; wait ;;
term-ignoring-descendant)
  trap 'exit 0' TERM
  (trap '' TERM; sleep 5) &
  echo "$!" > "$2"
  wait
  ;;
escaped)
  /usr/bin/perl -MPOSIX=setsid -e 'setsid(); sleep 5' &
  echo "$!" > "$2"
  ;;
closed-stdio-descendant)
  (exec >/dev/null 2>&1; sleep 5) &
  echo "$!" > "$2"
  exit 0
  ;;
esac
"#,
    );

    let runner = SystemLaunchctlRunnerV1::new(&fake_launchctl).with_bounds(
        Duration::from_secs(2),
        1024,
        Duration::from_millis(100),
    );
    let timeout_runner = SystemLaunchctlRunnerV1::new(&fake_launchctl).with_bounds(
        Duration::from_millis(500),
        1024,
        Duration::from_millis(100),
    );

    let success = runner
        .run(&["success".to_owned()])
        .expect("successful launchctl output");
    assert_eq!(success.exit_status, 0);
    assert_eq!(success.stdout, "stdout");
    assert_eq!(success.stderr, "stderr");

    let nonzero = runner
        .run(&["nonzero".to_owned()])
        .expect("nonzero launchctl is a process result");
    assert_eq!(nonzero.exit_status, 7);
    assert_eq!(nonzero.stdout, "out");
    assert_eq!(nonzero.stderr, "err");

    let signal = runner
        .run(&["signal".to_owned()])
        .expect("signal termination is a process result");
    assert_eq!(signal.exit_status, -1);

    let timeout_started = Instant::now();
    let timeout = timeout_runner.run(&["timeout".to_owned()]);
    assert!(timeout_started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        timeout,
        Err(ServiceErrorV1::LaunchctlTimeoutV1 { timeout_ms: 500 })
    );
    let direct_child = root.join("direct-child.pid");
    assert_eq!(
        timeout_runner.run(&["timeout-id".to_owned(), direct_child.display().to_string(),]),
        Err(ServiceErrorV1::LaunchctlTimeoutV1 { timeout_ms: 500 })
    );
    let direct_child_id = fs::read_to_string(&direct_child)
        .expect("direct child pid")
        .trim()
        .to_owned();
    wait_for_process_exit(&direct_child_id);
    assert!(
        !process_exists(&direct_child_id),
        "timeout cleanup must reap the direct launchctl child"
    );

    assert_eq!(
        runner.run(&["stdout-overflow".to_owned()]),
        Err(ServiceErrorV1::OutputLimitExceededV1 { limit_bytes: 1024 })
    );
    assert_eq!(
        runner.run(&["stderr-overflow".to_owned()]),
        Err(ServiceErrorV1::OutputLimitExceededV1 { limit_bytes: 1024 })
    );

    let simultaneous = runner
        .run(&["simultaneous".to_owned()])
        .expect("simultaneous pipes are drained");
    assert_eq!(simultaneous.stdout, "o".repeat(128));
    assert_eq!(simultaneous.stderr, "e".repeat(128));

    let closed_stdio_descendant = root.join("closed-stdio-descendant.pid");
    assert!(matches!(
        runner.run(&[
            "closed-stdio-descendant".to_owned(),
            closed_stdio_descendant.display().to_string(),
        ]),
        Err(ServiceErrorV1::IoV1 { .. })
    ));
    let closed_stdio_descendant_id = fs::read_to_string(&closed_stdio_descendant)
        .expect("closed-stdio descendant pid")
        .trim()
        .to_owned();
    wait_for_process_exit(&closed_stdio_descendant_id);
    assert!(
        !process_exists(&closed_stdio_descendant_id),
        "successful child cleanup must terminate closed-stdio descendants"
    );

    let descendant_process = root.join("descendant.pid");
    let descendant_started = Instant::now();
    let descendant = timeout_runner.run(&[
        "descendant".to_owned(),
        descendant_process.display().to_string(),
    ]);
    assert!(descendant_started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        descendant,
        Err(ServiceErrorV1::LaunchctlTimeoutV1 { timeout_ms: 500 })
    );
    let descendant_id = fs::read_to_string(&descendant_process)
        .expect("descendant pid")
        .trim()
        .to_owned();
    wait_for_process_exit(&descendant_id);
    assert!(
        !process_exists(&descendant_id),
        "process-group cleanup must terminate descendants"
    );
    let term_ignoring_process = root.join("term-ignoring-descendant.pid");
    let term_ignoring = timeout_runner.run(&[
        "term-ignoring-descendant".to_owned(),
        term_ignoring_process.display().to_string(),
    ]);
    assert_eq!(
        term_ignoring,
        Err(ServiceErrorV1::LaunchctlTimeoutV1 { timeout_ms: 500 })
    );
    let term_ignoring_id = fs::read_to_string(&term_ignoring_process)
        .expect("TERM-ignoring descendant pid")
        .trim()
        .to_owned();
    wait_for_process_exit(&term_ignoring_id);
    assert!(
        !process_exists(&term_ignoring_id),
        "SIGKILL cleanup must terminate TERM-ignoring same-group descendants after the leader exits"
    );

    let escaped_process = root.join("escaped.pid");
    let escaped_started = Instant::now();
    let escaped =
        timeout_runner.run(&["escaped".to_owned(), escaped_process.display().to_string()]);
    assert!(escaped_started.elapsed() < Duration::from_secs(2));
    let escaped_id = fs::read_to_string(&escaped_process)
        .expect("escaped descendant pid")
        .trim()
        .to_owned();
    let cleanup = Command::new("/bin/kill")
        .args(["-KILL", &escaped_id])
        .status()
        .expect("escaped fixture cleanup command");
    assert!(
        cleanup.success() || !process_exists(&escaped_id),
        "escaped fixture cleanup must either kill or observe an exited process"
    );
    wait_for_process_exit(&escaped_id);
    assert!(
        !process_exists(&escaped_id),
        "escaped fixture process must be explicitly torn down after the bounded runner failure"
    );
    match escaped {
        Err(ServiceErrorV1::IoV1 { message, .. }) => {
            assert!(message.contains("retained an output pipe"));
        }
        other => panic!("escaped pipe holder must return bounded I/O error, got {other:?}"),
    }

    fs::remove_dir_all(root).expect("remove runner fixture");
}
