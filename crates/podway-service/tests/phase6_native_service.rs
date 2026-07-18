use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use podway_core::UnixMillis;
use podway_service::{
    FixedServiceClockV1, InstallSpecV1, LocalPlatformPathV1, LogQueryV1,
    MacosServiceCommandRunnerV1, SERVICE_LOG_MAX_BYTES_V1, ServiceLabelV1, ServiceLogStreamV1,
    ServiceManagerContractV1, ServiceManagerV1, ServiceOutcomeKindV1, ServiceRuntimePathsV1,
    ServiceStatusV1, StdServiceFilesystemV1, SystemLaunchctlRunnerV1, UninstallOptionsV1,
};

fn unique_root() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "podway-phase6-native-{}-{nanos}",
        std::process::id()
    ))
}

fn write_executable(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
    fs::write(path, bytes).expect("write executable fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("chmod executable fixture");
}

fn spec(binary: &Path, paths: &ServiceRuntimePathsV1) -> InstallSpecV1 {
    InstallSpecV1::new(
        LocalPlatformPathV1::new(binary).expect("absolute binary path"),
        ServiceLabelV1::podwayd(),
        paths.clone(),
    )
}

#[test]
fn production_adapters_cover_native_launchagent_lifecycle_without_real_launchctl() {
    let root = unique_root();
    let home = root.join("home");
    let runtime = root.join("runtime");
    let paths = ServiceRuntimePathsV1::from_directories(
        home.join("Library/LaunchAgents"),
        home.join("Library/Application Support/Podway"),
        home.join("Library/Logs/Podway"),
        &runtime,
    )
    .expect("fixture service paths");

    let launchctl_log = root.join("launchctl.argv");
    let fake_launchctl = root.join("bin/launchctl");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = print ]; then printf 'pid = 4242\\n'; fi\nexit 0\n",
        launchctl_log.display()
    );
    write_executable(&fake_launchctl, script.as_bytes());

    let first_binary = root.join("bin/podwayd-v1");
    let second_binary = root.join("bin/podwayd-v2");
    write_executable(&first_binary, b"#!/bin/sh\nexit 0\n");
    write_executable(&second_binary, b"#!/bin/sh\nexit 0\n");

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
    let plist = fs::read_to_string(paths.launch_agent_path().as_path()).expect("installed plist");
    assert!(plist.contains(first_binary.to_str().expect("UTF-8 fixture")));
    assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
    assert!(plist.contains("<key>SuccessfulExit</key>\n    <false/>"));
    assert!(plist.contains("<string>--service</string>"));
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

    let socket = paths.socket_path().as_path();
    fs::write(socket, b"stale").expect("stale socket fixture");
    let updated = manager
        .update(spec(&second_binary, &paths))
        .expect("binary update");
    assert_eq!(updated.kind(), ServiceOutcomeKindV1::ChangedV1);
    assert!(!socket.exists(), "update must remove a stale socket");
    let updated_plist =
        fs::read_to_string(paths.launch_agent_path().as_path()).expect("updated plist");
    assert!(updated_plist.contains(second_binary.to_str().expect("UTF-8 fixture")));
    assert!(!updated_plist.contains(first_binary.to_str().expect("UTF-8 fixture")));

    assert_eq!(
        manager.stop().expect("explicit stop").kind(),
        ServiceOutcomeKindV1::StoppedV1
    );
    assert_eq!(
        manager.start().expect("explicit start").kind(),
        ServiceOutcomeKindV1::ChangedV1
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

    manager
        .install(spec(&second_binary, &paths))
        .expect("reinstall before purge");
    fs::write(log_path, b"purge me\n").expect("purge log fixture");
    manager
        .uninstall_with_options(UninstallOptionsV1::new(true))
        .expect("purging uninstall");
    assert!(!log_path.exists(), "explicit purge must remove logs");
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
    fs::remove_dir_all(root).expect("remove native service fixture");
}
