//! Phase 0B service-manager contract conformance; the recording double never launches a command.
//!
//! Requirements: ARC-008, SEC-002, OPS-001, OPS-002.

use podway_core::UnixMillis;
use podway_service::{
    FixedServiceClockV1, InstallSpecV1, LocalPlatformPathV1, LogLocationV1, LogQueryV1,
    RecordingServiceCommandRunnerV1, RecordingServiceManagerV1, ServiceClockV1,
    ServiceCommandResultV1, ServiceCommandV1, ServiceErrorV1, ServiceLogStreamV1,
    ServiceManagerContractV1, ServiceOperationV1, ServiceOutcomeV1, ServicePathErrorV1,
    ServiceRunningV1, ServiceRuntimePathsV1, ServiceStatusV1, ServiceStoppedV1,
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
