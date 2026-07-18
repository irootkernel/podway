//! Phase 0B service-manager contract conformance; the recording double never launches a command.
//!
//! Requirements: ARC-008, SEC-002, OPS-001, OPS-002.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use podway_core::UnixMillis;
use podway_service::{
    FixedServiceClockV1, InstallSpecV1, LaunchctlOutputV1, LaunchctlRunnerV1, LocalPlatformPathV1,
    LogLocationV1, LogQueryV1, MacosServiceCommandRunnerV1, RecordingServiceCommandRunnerV1,
    RecordingServiceManagerV1, ServiceClockV1, ServiceCommandResultV1, ServiceCommandRunnerV1,
    ServiceCommandV1, ServiceErrorV1, ServiceFilesystemErrorV1, ServiceFilesystemV1,
    ServiceLogStreamV1, ServiceManagerContractV1, ServiceOperationV1, ServiceOutcomeV1,
    ServicePathErrorV1, ServiceRunningV1, ServiceRuntimePathsV1, ServiceStatusV1, ServiceStoppedV1,
    UninstallOptionsV1, launch_agent_plist_v1,
};

fn service_paths() -> ServiceRuntimePathsV1 {
    match ServiceRuntimePathsV1::from_directories(
        "/Users/podway/Library/LaunchAgents",
        "/Users/podway/Library/Application Support/Podway",
        "/Users/podway/Library/Logs/Podway",
        "/var/folders/podway/runtime",
    ) {
        Ok(value) => value,
        Err(_) => panic!("fixture service paths must be valid"),
    }
}

#[test]
fn arc_008_service_v1_exposes_exact_global_runtime_paths() {
    let paths = match ServiceRuntimePathsV1::for_user("/Users/podway", "/var/folders/podway", 501) {
        Ok(value) => value,
        Err(_) => panic!("fixture service paths must be valid"),
    };

    assert_eq!(
        paths.launch_agent_path().as_path(),
        std::path::Path::new("/Users/podway/Library/LaunchAgents/dev.podway.podwayd.plist")
    );
    assert_eq!(
        paths.metadata_index_path().as_path(),
        std::path::Path::new("/Users/podway/Library/Application Support/Podway/service.json")
    );
    assert_eq!(
        paths.workspace_registry_path().as_path(),
        std::path::Path::new("/Users/podway/Library/Application Support/Podway/workspaces.json")
    );
    assert_eq!(
        paths.log_path().as_path(),
        std::path::Path::new("/Users/podway/Library/Logs/Podway/podwayd.log")
    );
    assert_eq!(
        paths.runtime_directory().as_path(),
        std::path::Path::new("/var/folders/podway/podway-501")
    );
    assert_eq!(
        paths.socket_path().as_path(),
        std::path::Path::new("/var/folders/podway/podway-501/podwayd.sock")
    );
    assert_eq!(
        paths.global_lock_path().as_path(),
        std::path::Path::new("/var/folders/podway/podway-501/podwayd.lock")
    );
    assert_ne!(paths.metadata_index_path(), paths.workspace_registry_path());
}

#[test]
fn arc_008_service_v1_runtime_paths_are_global_and_value_equal() {
    let paths = service_paths();
    let equivalent = match ServiceRuntimePathsV1::from_directories(
        "/Users/podway/Library/LaunchAgents",
        "/Users/podway/Library/Application Support/Podway",
        "/Users/podway/Library/Logs/Podway",
        "/var/folders/podway/runtime",
    ) {
        Ok(value) => value,
        Err(_) => panic!("equivalent service paths must be valid"),
    };
    let distinct = match ServiceRuntimePathsV1::from_directories(
        "/Users/podway/Library/LaunchAgents",
        "/Users/podway/Library/Application Support/Podway",
        "/Users/podway/Library/Logs/Podway",
        "/var/folders/podway/other-runtime",
    ) {
        Ok(value) => value,
        Err(_) => panic!("distinct service paths must be valid"),
    };

    assert_eq!(paths, equivalent);
    assert_ne!(paths, distinct);
    assert_ne!(paths.runtime_directory(), paths.workspace_registry_path());
}

#[test]
fn arc_008_sec_002_service_v1_rejects_root_and_path_bound_inputs() {
    assert!(matches!(
        ServiceRuntimePathsV1::for_user("/Users/podway", "/var/folders/podway", 0),
        Err(ServicePathErrorV1::RootUser)
    ));
    assert!(matches!(
        ServiceRuntimePathsV1::from_directories(
            "",
            "/Users/podway/Library/Application Support/Podway",
            "/Users/podway/Library/Logs/Podway",
            "/var/folders/podway/runtime",
        ),
        Err(ServicePathErrorV1::Empty {
            field: "launch_agents_directory"
        })
    ));
    assert!(matches!(
        ServiceRuntimePathsV1::from_directories(
            "/Users/podway/Library/LaunchAgents",
            "Library/Application Support/Podway",
            "/Users/podway/Library/Logs/Podway",
            "/var/folders/podway/runtime",
        ),
        Err(ServicePathErrorV1::Relative {
            field: "application_support_directory",
            ..
        })
    ));
    assert!(matches!(
        ServiceRuntimePathsV1::from_directories(
            "/Users/podway/Library/LaunchAgents",
            "/Users/podway/Library/Application Support/Podway",
            "/Users/podway/Library/Logs/Podway/../Podway",
            "/var/folders/podway/runtime",
        ),
        Err(ServicePathErrorV1::Unnormalized {
            field: "log_directory",
            ..
        })
    ));
}

fn assert_service_manager_contract_signatures<T: ServiceManagerContractV1>() {
    let _: fn(&T, InstallSpecV1) -> Result<ServiceOutcomeV1, ServiceErrorV1> =
        <T as ServiceManagerContractV1>::install;
    let _: fn(&T, LogQueryV1) -> Result<LogLocationV1, ServiceErrorV1> =
        <T as ServiceManagerContractV1>::logs;
    let _: fn(&T) -> Result<ServiceOutcomeV1, ServiceErrorV1> =
        <T as ServiceManagerContractV1>::restart;
    let _: fn(&T) -> Result<ServiceOutcomeV1, ServiceErrorV1> =
        <T as ServiceManagerContractV1>::start;
    let _: fn(&T) -> Result<ServiceStatusV1, ServiceErrorV1> =
        <T as ServiceManagerContractV1>::status;
    let _: fn(&T) -> Result<ServiceOutcomeV1, ServiceErrorV1> =
        <T as ServiceManagerContractV1>::stop;
    let _: fn(&T) -> Result<ServiceOutcomeV1, ServiceErrorV1> =
        <T as ServiceManagerContractV1>::uninstall;
    let _: fn(&T, InstallSpecV1) -> Result<ServiceOutcomeV1, ServiceErrorV1> =
        <T as ServiceManagerContractV1>::update;
}

#[test]
fn arc_008_service_v1_exposes_only_manifest_lifecycle_signatures() {
    assert_service_manager_contract_signatures::<RecordingServiceManagerV1>();
}
#[test]
fn arc_008_service_v1_exposes_exactly_the_manifest_error_surface() {
    for error in [
        ServiceErrorV1::InvalidMetadataV1 {
            message: "invalid metadata".to_owned(),
        },
        ServiceErrorV1::IoV1 {
            operation: Some(ServiceOperationV1::Status),
            message: "I/O failure".to_owned(),
        },
        ServiceErrorV1::LaunchctlFailureV1 {
            operation: ServiceOperationV1::Install,
            exit_status: Some(1),
            message: "launchctl failed".to_owned(),
        },
        ServiceErrorV1::LogUnavailableV1 {
            message: "log unavailable".to_owned(),
        },
        ServiceErrorV1::PathSafetyV1(ServicePathErrorV1::RootUser),
        ServiceErrorV1::PermissionDeniedV1 {
            operation: ServiceOperationV1::Uninstall,
            path: std::path::PathBuf::from(
                "/Users/podway/Library/LaunchAgents/dev.podway.podwayd.plist",
            ),
            message: "permission denied".to_owned(),
        },
        ServiceErrorV1::StaleOrUnexpectedProcessV1 {
            path: std::path::PathBuf::from("/var/folders/podway/runtime/podwayd.sock"),
            message: "stale process".to_owned(),
        },
        ServiceErrorV1::TimeoutV1 {
            operation: ServiceOperationV1::Restart,
            timeout_ms: 5_000,
        },
    ] {
        match error {
            ServiceErrorV1::InvalidMetadataV1 { message } => {
                assert_eq!(message, "invalid metadata");
            }
            ServiceErrorV1::IoV1 { operation, message } => {
                assert_eq!(operation, Some(ServiceOperationV1::Status));
                assert_eq!(message, "I/O failure");
            }
            ServiceErrorV1::LaunchctlFailureV1 {
                operation,
                exit_status,
                message,
            } => {
                assert_eq!(operation, ServiceOperationV1::Install);
                assert_eq!(exit_status, Some(1));
                assert_eq!(message, "launchctl failed");
            }
            ServiceErrorV1::LogUnavailableV1 { message } => {
                assert_eq!(message, "log unavailable");
            }
            ServiceErrorV1::PathSafetyV1(problem) => {
                assert!(matches!(problem, ServicePathErrorV1::RootUser));
            }
            ServiceErrorV1::PermissionDeniedV1 {
                operation,
                path,
                message,
            } => {
                assert_eq!(operation, ServiceOperationV1::Uninstall);
                assert_eq!(
                    path,
                    std::path::PathBuf::from(
                        "/Users/podway/Library/LaunchAgents/dev.podway.podwayd.plist",
                    )
                );
                assert_eq!(message, "permission denied");
            }
            ServiceErrorV1::StaleOrUnexpectedProcessV1 { path, message } => {
                assert_eq!(
                    path,
                    std::path::PathBuf::from("/var/folders/podway/runtime/podwayd.sock")
                );
                assert_eq!(message, "stale process");
            }
            ServiceErrorV1::TimeoutV1 {
                operation,
                timeout_ms,
            } => {
                assert_eq!(operation, ServiceOperationV1::Restart);
                assert_eq!(timeout_ms, 5_000);
            }
        }
    }
}

#[test]
fn arc_008_sec_002_service_v1_rejects_workspace_local_paths() {
    for (expected_field, directories) in [
        (
            "launch_agents_directory",
            [
                "/workspace/.podway/LaunchAgents",
                "/Users/podway/Library/Application Support/Podway",
                "/Users/podway/Library/Logs/Podway",
                "/var/folders/podway/runtime",
            ],
        ),
        (
            "application_support_directory",
            [
                "/Users/podway/Library/LaunchAgents",
                "/workspace/.podway/service",
                "/Users/podway/Library/Logs/Podway",
                "/var/folders/podway/runtime",
            ],
        ),
        (
            "log_directory",
            [
                "/Users/podway/Library/LaunchAgents",
                "/Users/podway/Library/Application Support/Podway",
                "/workspace/.podway/logs",
                "/var/folders/podway/runtime",
            ],
        ),
        (
            "runtime_directory",
            [
                "/Users/podway/Library/LaunchAgents",
                "/Users/podway/Library/Application Support/Podway",
                "/Users/podway/Library/Logs/Podway",
                "/workspace/.podway/runtime",
            ],
        ),
    ] {
        assert!(matches!(
            ServiceRuntimePathsV1::from_directories(
                directories[0],
                directories[1],
                directories[2],
                directories[3],
            ),
            Err(ServicePathErrorV1::WorkspaceLocal { field, .. }) if field == expected_field
        ));
    }
    assert!(matches!(
        LocalPlatformPathV1::new("/workspace/.podway/bin/podwayd"),
        Err(ServicePathErrorV1::WorkspaceLocal {
            field: "local_platform_path",
            ..
        })
    ));
}

#[test]
fn arc_008_service_v1_returns_only_a_bounded_log_location() {
    let paths = service_paths();
    let expected = LogLocationV1::new(paths.log_path().clone());
    let manager = RecordingServiceManagerV1::new(
        RecordingServiceCommandRunnerV1::new([Ok(ServiceCommandResultV1::LogLocation(
            expected.clone(),
        ))]),
        FixedServiceClockV1::new(UnixMillis::new(1_000)),
        paths.clone(),
    );

    let location: LogLocationV1 = match ServiceManagerContractV1::logs(
        &manager,
        LogQueryV1::new(ServiceLogStreamV1::DaemonV1),
    ) {
        Ok(value) => value,
        Err(_) => panic!("recorded log location must be accepted"),
    };
    assert_eq!(location, expected);
    assert_eq!(location.path(), paths.log_path());

    let commands = match manager.command_runner().recorded_commands() {
        Ok(value) => value,
        Err(_) => panic!("recording runner state must be readable"),
    };
    assert_eq!(commands.len(), 1);
    match commands[0].command() {
        ServiceCommandV1::Logs {
            requested_at,
            paths: observed_paths,
            query,
        } => {
            assert_eq!(*requested_at, UnixMillis::new(1_000));
            assert_eq!(observed_paths, &paths);
            assert_eq!(query.stream(), ServiceLogStreamV1::DaemonV1);
        }
        _ => panic!("logs must record a logs command"),
    }
    let outside_location = match LocalPlatformPathV1::new("/Users/podway/other.log") {
        Ok(path) => LogLocationV1::new(path),
        Err(_) => panic!("fixture log path must be valid"),
    };
    let outside_manager = RecordingServiceManagerV1::new(
        RecordingServiceCommandRunnerV1::new([Ok(ServiceCommandResultV1::LogLocation(
            outside_location,
        ))]),
        FixedServiceClockV1::new(UnixMillis::new(1_001)),
        paths,
    );
    assert!(matches!(
        ServiceManagerContractV1::logs(
            &outside_manager,
            LogQueryV1::new(ServiceLogStreamV1::DaemonV1),
        ),
        Err(ServiceErrorV1::LogUnavailableV1 { .. })
    ));
}

#[test]
fn ops_002_service_v1_returns_the_bounded_status_domain() {
    let paths = service_paths();
    let manager = RecordingServiceManagerV1::new(
        RecordingServiceCommandRunnerV1::new([Ok(ServiceCommandResultV1::Status(
            ServiceStatusV1::RunningV1(ServiceRunningV1::new(
                UnixMillis::new(2_000),
                Some(42),
                None,
            )),
        ))]),
        FixedServiceClockV1::new(UnixMillis::new(1_999)),
        paths.clone(),
    );

    let status: ServiceStatusV1 = match ServiceManagerContractV1::status(&manager) {
        Ok(value) => value,
        Err(_) => panic!("recorded status must be accepted"),
    };
    assert!(matches!(status, ServiceStatusV1::RunningV1(_)));
    assert_eq!(manager.clock().now(), UnixMillis::new(1_999));

    let commands = match manager.command_runner().recorded_commands() {
        Ok(value) => value,
        Err(_) => panic!("recording runner state must be readable"),
    };
    match commands.as_slice() {
        [recorded] => match recorded.command() {
            ServiceCommandV1::Status {
                requested_at,
                paths: observed_paths,
            } => {
                assert_eq!(*requested_at, UnixMillis::new(1_999));
                assert_eq!(observed_paths, &paths);
            }
            _ => panic!("status must record a status command"),
        },
        _ => panic!("status must record exactly one command"),
    }
}

#[test]
fn arc_008_ops_002_service_v1_maps_runner_contract_violations_to_io() {
    let outcome_manager = RecordingServiceManagerV1::new(
        RecordingServiceCommandRunnerV1::new([Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::StoppedV1(ServiceStoppedV1::new(UnixMillis::new(2_000), None)),
        ))]),
        FixedServiceClockV1::new(UnixMillis::new(1_999)),
        service_paths(),
    );
    assert!(matches!(
        ServiceManagerContractV1::start(&outcome_manager),
        Err(ServiceErrorV1::IoV1 {
            operation: Some(ServiceOperationV1::Start),
            message,
        }) if message == "runner returned unexpected stopped outcome"
    ));

    let result_manager = RecordingServiceManagerV1::new(
        RecordingServiceCommandRunnerV1::new([Ok(ServiceCommandResultV1::Status(
            ServiceStatusV1::StoppedV1(ServiceStoppedV1::new(UnixMillis::new(2_000), None)),
        ))]),
        FixedServiceClockV1::new(UnixMillis::new(2_001)),
        service_paths(),
    );
    assert!(matches!(
        ServiceManagerContractV1::logs(
            &result_manager,
            LogQueryV1::new(ServiceLogStreamV1::DaemonV1),
        ),
        Err(ServiceErrorV1::IoV1 {
            operation: Some(ServiceOperationV1::Logs),
            message,
        }) if message == "runner returned status result; expected log_location"
    ));
}

#[test]
fn arc_008_service_v1_recording_runner_exhaustion_fails_explicitly() {
    let manager = RecordingServiceManagerV1::new(
        RecordingServiceCommandRunnerV1::new([]),
        FixedServiceClockV1::new(UnixMillis::new(2_002)),
        service_paths(),
    );

    assert!(matches!(
        ServiceManagerContractV1::start(&manager),
        Err(ServiceErrorV1::IoV1 {
            operation: Some(ServiceOperationV1::Start),
            message,
        }) if message == "recording service command runner has no recorded result"
    ));

    let commands = match manager.command_runner().recorded_commands() {
        Ok(value) => value,
        Err(_) => panic!("recording runner state must be readable"),
    };
    assert_eq!(commands.len(), 1);
}
#[derive(Clone, Default)]
struct Phase6Filesystem {
    files: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
    events: Arc<Mutex<Vec<String>>>,
}

impl Phase6Filesystem {
    fn record(&self, event: impl Into<String>) {
        self.events.lock().expect("test lock").push(event.into());
    }
}

impl ServiceFilesystemV1 for Phase6Filesystem {
    fn exists(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
        Ok(self.files.lock().expect("test lock").contains_key(path))
    }

    fn is_executable(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
        Ok(path.starts_with("/Applications/Podway/"))
    }

    fn create_directory(&self, path: &Path, _: u32) -> Result<(), ServiceFilesystemErrorV1> {
        self.record(format!("mkdir:{}", path.display()));
        Ok(())
    }

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, ServiceFilesystemErrorV1> {
        self.files
            .lock()
            .expect("test lock")
            .get(path)
            .cloned()
            .ok_or_else(|| ServiceFilesystemErrorV1::other("not found"))
    }

    fn write_atomically(
        &self,
        path: &Path,
        contents: &[u8],
        _: u32,
    ) -> Result<(), ServiceFilesystemErrorV1> {
        self.record(format!("write:{}", path.display()));
        self.files
            .lock()
            .expect("test lock")
            .insert(path.to_path_buf(), contents.to_vec());
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        self.record(format!("remove:{}", path.display()));
        self.files.lock().expect("test lock").remove(path);
        Ok(())
    }

    fn remove_directory_contents(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        self.record(format!("purge:{}", path.display()));
        Ok(())
    }

    fn rotate_file(&self, path: &Path, _: u64, _: u8) -> Result<(), ServiceFilesystemErrorV1> {
        self.record(format!("rotate:{}", path.display()));
        Ok(())
    }
}

#[derive(Clone)]
struct Phase6Launchctl {
    events: Arc<Mutex<Vec<String>>>,
    bootstrap_status: i32,
    print_status: i32,
}

impl LaunchctlRunnerV1 for Phase6Launchctl {
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        self.events
            .lock()
            .expect("test lock")
            .push(format!("launchctl:{}", arguments.join(" ")));
        Ok(LaunchctlOutputV1 {
            exit_status: if arguments
                .first()
                .is_some_and(|argument| argument == "bootstrap")
            {
                self.bootstrap_status
            } else if arguments
                .first()
                .is_some_and(|argument| argument == "print")
            {
                self.print_status
            } else {
                0
            },
            stdout: String::new(),
            stderr: "scripted launchctl failure".to_owned(),
        })
    }
}

fn phase6_spec() -> InstallSpecV1 {
    InstallSpecV1::new(
        LocalPlatformPathV1::new("/Applications/Podway/podwayd").expect("fixture binary path"),
        podway_service::ServiceLabelV1::podwayd(),
        service_paths(),
    )
}

#[test]
fn phase6_install_orders_atomic_plist_bootstrap_and_metadata() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 0,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_000)),
        501,
    )
    .expect("non-root service runner");

    assert!(matches!(
        runner.run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_000),
            spec: phase6_spec(),
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(_)
        ))
    ));
    let events = filesystem.events.lock().expect("test lock").clone();
    let plist = service_paths()
        .launch_agent_path()
        .as_path()
        .display()
        .to_string();
    let metadata = service_paths()
        .metadata_index_path()
        .as_path()
        .display()
        .to_string();
    let plist_write = events
        .iter()
        .position(|event| event == &format!("write:{plist}"))
        .expect("plist write");
    let bootstrap = events
        .iter()
        .position(|event| event.starts_with("launchctl:bootstrap gui/501"))
        .expect("bootstrap");
    let metadata_write = events
        .iter()
        .position(|event| event == &format!("write:{metadata}"))
        .expect("metadata write");
    assert!(plist_write < bootstrap && bootstrap < metadata_write);
}

#[test]
fn phase6_install_launchctl_failure_does_not_record_metadata() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 1,
            print_status: 0,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_001)),
        501,
    )
    .expect("non-root service runner");

    assert!(matches!(
        runner.run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_001),
            spec: phase6_spec(),
        }),
        Err(ServiceErrorV1::LaunchctlFailureV1 {
            operation: ServiceOperationV1::Install,
            ..
        })
    ));
    assert!(
        !filesystem
            .files
            .lock()
            .expect("test lock")
            .contains_key(service_paths().metadata_index_path().as_path())
    );
}

#[test]
fn phase6_install_is_idempotent_and_plist_is_canonical() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 0,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_002)),
        501,
    )
    .expect("non-root service runner");
    let command = ServiceCommandV1::Install {
        requested_at: UnixMillis::new(3_002),
        spec: phase6_spec(),
    };

    runner.run(command.clone()).expect("first install succeeds");
    let event_count = filesystem.events.lock().expect("test lock").len();
    let later_runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 0,
        },
        FixedServiceClockV1::new(UnixMillis::new(9_999)),
        501,
    )
    .expect("non-root service runner");
    assert!(matches!(
        later_runner.run(command),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::AlreadyInDesiredStateV1(_)
        ))
    ));
    assert_eq!(
        filesystem.events.lock().expect("test lock").len(),
        event_count
    );

    let plist = String::from_utf8(launch_agent_plist_v1(
        Path::new("/Applications/Podway/podwayd"),
        service_paths().log_path().as_path(),
    ))
    .expect("plist UTF-8");
    assert!(plist.contains("<string>/Applications/Podway/podwayd</string>"));
    assert!(plist.contains("<string>--service</string>"));
    assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
    assert!(plist.contains("<key>SuccessfulExit</key>\n    <false/>"));
}
#[test]
fn phase6_restart_boots_out_removes_stale_socket_then_bootstraps() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 0,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_003)),
        501,
    )
    .expect("non-root service runner");
    runner
        .run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_003),
            spec: phase6_spec(),
        })
        .expect("initial install");
    filesystem.files.lock().expect("test lock").insert(
        service_paths().socket_path().as_path().to_path_buf(),
        Vec::new(),
    );

    assert!(matches!(
        runner.run(ServiceCommandV1::Restart {
            requested_at: UnixMillis::new(3_003),
            paths: service_paths(),
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(_)
        ))
    ));
    let events = filesystem.events.lock().expect("test lock").clone();
    let bootout = events
        .iter()
        .position(|event| event.starts_with("launchctl:bootout gui/501/"))
        .expect("bootout");
    let remove = events
        .iter()
        .position(|event| {
            event
                == &format!(
                    "remove:{}",
                    service_paths().socket_path().as_path().display()
                )
        })
        .expect("socket cleanup");
    let bootstrap = events
        .iter()
        .rposition(|event| event.starts_with("launchctl:bootstrap gui/501"))
        .expect("bootstrap");
    assert!(bootout < remove && remove < bootstrap);
}

#[test]
fn phase6_update_after_binary_change_replaces_plist_and_restarts_with_socket_cleanup() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 0,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_004)),
        501,
    )
    .expect("non-root service runner");
    runner
        .run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_004),
            spec: phase6_spec(),
        })
        .expect("initial install");
    filesystem.files.lock().expect("test lock").insert(
        service_paths().socket_path().as_path().to_path_buf(),
        Vec::new(),
    );
    let changed = InstallSpecV1::new(
        LocalPlatformPathV1::new("/Applications/Podway/podwayd-next").expect("fixture binary"),
        podway_service::ServiceLabelV1::podwayd(),
        service_paths(),
    );
    assert!(matches!(
        runner.run(ServiceCommandV1::Update {
            requested_at: UnixMillis::new(3_004),
            spec: changed,
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(_)
        ))
    ));
    let events = filesystem.events.lock().expect("test lock").clone();
    assert!(
        events
            .iter()
            .any(|event| event.starts_with("launchctl:bootout gui/501/"))
    );
    assert!(events.iter().any(|event| event
        == &format!(
            "remove:{}",
            service_paths().socket_path().as_path().display()
        )));
    let plist = filesystem
        .files
        .lock()
        .expect("test lock")
        .get(service_paths().launch_agent_path().as_path())
        .cloned()
        .expect("updated plist");
    assert!(
        String::from_utf8(plist)
            .expect("plist UTF-8")
            .contains("/Applications/Podway/podwayd-next")
    );
}

#[test]
fn phase6_start_stop_status_and_logs_cover_installed_lifecycle_states() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 1,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_005)),
        501,
    )
    .expect("non-root service runner");
    assert!(matches!(
        runner.run(ServiceCommandV1::Start {
            requested_at: UnixMillis::new(3_005),
            paths: service_paths(),
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::NotInstalledV1(_)
        ))
    ));
    runner
        .run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_005),
            spec: phase6_spec(),
        })
        .expect("initial install");
    assert!(matches!(
        runner.run(ServiceCommandV1::Start {
            requested_at: UnixMillis::new(3_005),
            paths: service_paths(),
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(_)
        ))
    ));
    assert!(matches!(
        runner.run(ServiceCommandV1::Stop {
            requested_at: UnixMillis::new(3_005),
            paths: service_paths(),
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::StoppedV1(_)
        ))
    ));
    assert!(matches!(
        runner.run(ServiceCommandV1::Status {
            requested_at: UnixMillis::new(3_005),
            paths: service_paths(),
        }),
        Ok(ServiceCommandResultV1::Status(ServiceStatusV1::StoppedV1(
            _
        )))
    ));
    filesystem.files.lock().expect("test lock").insert(
        service_paths().log_path().as_path().to_path_buf(),
        b"structured log".to_vec(),
    );
    assert!(matches!(
        runner.run(ServiceCommandV1::Logs {
            requested_at: UnixMillis::new(3_005),
            paths: service_paths(),
            query: LogQueryV1::new(ServiceLogStreamV1::DaemonV1)
                .with_lines(Some(50))
                .with_follow(true),
        }),
        Ok(ServiceCommandResultV1::LogLocation(_))
    ));
}

#[test]
fn phase6_uninstall_preserves_registry_and_logs_unless_purge_is_explicit() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 0,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_006)),
        501,
    )
    .expect("non-root service runner");
    for path in [
        service_paths().launch_agent_path().as_path(),
        service_paths().metadata_index_path().as_path(),
        service_paths().socket_path().as_path(),
        service_paths().global_lock_path().as_path(),
        service_paths().workspace_registry_path().as_path(),
        service_paths().log_path().as_path(),
    ] {
        filesystem
            .files
            .lock()
            .expect("test lock")
            .insert(path.to_path_buf(), Vec::new());
    }
    assert!(matches!(
        runner.run(ServiceCommandV1::Uninstall {
            requested_at: UnixMillis::new(3_006),
            paths: service_paths(),
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(_)
        ))
    ));
    let files = filesystem.files.lock().expect("test lock");
    assert!(files.contains_key(service_paths().workspace_registry_path().as_path()));
    assert!(files.contains_key(service_paths().log_path().as_path()));
    drop(files);
    assert!(matches!(
        runner.run(ServiceCommandV1::UninstallWithOptions {
            requested_at: UnixMillis::new(3_006),
            paths: service_paths(),
            options: UninstallOptionsV1::new(true),
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(_)
        ))
    ));
    assert!(
        filesystem
            .events
            .lock()
            .expect("test lock")
            .iter()
            .any(|event| event.starts_with("purge:"))
    );
}

#[test]
fn phase6_runtime_socket_path_falls_back_when_tmpdir_exceeds_unix_socket_limit() {
    let temporary_directory = format!("/{}", "a".repeat(120));
    let paths = ServiceRuntimePathsV1::for_user("/Users/podway", temporary_directory, 501)
        .expect("fallback runtime path");
    assert_eq!(
        paths.socket_path().as_path(),
        Path::new("/tmp/podway-501/podwayd.sock")
    );
}

#[test]
fn phase6_status_rejects_incompatible_metadata_and_reports_running_when_loaded() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 0,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_007)),
        501,
    )
    .expect("non-root service runner");
    filesystem.files.lock().expect("test lock").insert(
        service_paths().launch_agent_path().as_path().to_path_buf(),
        b"legacy plist without a generation marker".to_vec(),
    );
    filesystem.files.lock().expect("test lock").insert(
        service_paths().metadata_index_path().as_path().to_path_buf(),
        br#"{"version":2,"label":"dev.podway.podwayd","daemon_binary":"/Applications/Podway/podwayd","installed_at":1,"updated_at":1}"#.to_vec(),
    );
    assert!(matches!(
        runner.run(ServiceCommandV1::Status {
            requested_at: UnixMillis::new(3_007),
            paths: service_paths(),
        }),
        Err(ServiceErrorV1::InvalidMetadataV1 { .. })
    ));
    filesystem.files.lock().expect("test lock").insert(
        service_paths().metadata_index_path().as_path().to_path_buf(),
        br#"{"version":1,"label":"dev.podway.podwayd","daemon_binary":"/Applications/Podway/podwayd","installed_at":1,"updated_at":1}"#.to_vec(),
    );
    assert!(matches!(
        runner.run(ServiceCommandV1::Status {
            requested_at: UnixMillis::new(3_007),
            paths: service_paths(),
        }),
        Err(ServiceErrorV1::InvalidMetadataV1 { .. })
    ));
}
#[test]
fn phase6_authenticated_generation_rejects_plist_semantic_tampering_and_install_repairs_it() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 0,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_008)),
        501,
    )
    .expect("non-root service runner");
    let install = ServiceCommandV1::Install {
        requested_at: UnixMillis::new(3_008),
        spec: phase6_spec(),
    };
    runner.run(install.clone()).expect("initial install");
    let plist_path = service_paths().launch_agent_path().as_path().to_path_buf();
    let tampered =
        String::from_utf8(filesystem.files.lock().expect("test lock")[&plist_path].clone())
            .expect("plist UTF-8")
            .replace(
                "<key>RunAtLoad</key>\n  <true/>",
                "<key>RunAtLoad</key>\n  <false/>",
            )
            .into_bytes();
    filesystem
        .files
        .lock()
        .expect("test lock")
        .insert(plist_path, tampered);
    for command in [
        ServiceCommandV1::Status {
            requested_at: UnixMillis::new(3_008),
            paths: service_paths(),
        },
        ServiceCommandV1::Start {
            requested_at: UnixMillis::new(3_008),
            paths: service_paths(),
        },
        ServiceCommandV1::Restart {
            requested_at: UnixMillis::new(3_008),
            paths: service_paths(),
        },
    ] {
        assert!(matches!(
            runner.run(command),
            Err(ServiceErrorV1::InvalidMetadataV1 { .. })
        ));
    }
    assert!(matches!(
        runner.run(install),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(_)
        ))
    ));
    assert!(matches!(
        runner.run(ServiceCommandV1::Status {
            requested_at: UnixMillis::new(3_008),
            paths: service_paths(),
        }),
        Ok(ServiceCommandResultV1::Status(_))
    ));
}
