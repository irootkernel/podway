//! Phase 0B service-manager contract conformance; the recording double never launches a command.
//!
//! Requirements: ARC-008, SEC-002, OPS-001, OPS-002.

use std::{
    collections::HashMap,
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use podway_core::UnixMillis;
use podway_service::{
    FixedServiceClockV1, InstallSpecV1, LaunchctlOutputV1, LaunchctlRunnerV1, LocalPlatformPathV1,
    LogLocationV1, LogQueryV1, MacosServiceCommandRunnerV1, PodwayHomeV1,
    RecordingServiceCommandRunnerV1, RecordingServiceManagerV1, SERVICE_METADATA_MAX_BYTES_V1,
    ServiceClockV1, ServiceCommandResultV1, ServiceCommandRunnerV1, ServiceCommandV1,
    ServiceErrorV1, ServiceFilesystemErrorV1, ServiceFilesystemV1, ServiceLogStreamV1,
    ServiceManagerContractV1, ServiceOperationV1, ServiceOutcomeV1, ServicePathErrorV1,
    ServiceRunningV1, ServiceRuntimePathsV1, ServiceStatusV1, ServiceStoppedV1, UninstallOptionsV1,
    installed_socket_path_from_metadata_v1,
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
fn aut_home_001_podway_home_is_derived_from_the_os_account_home() {
    let home = PodwayHomeV1::from_account_home("/Users/podway", 501)
        .expect("fixture account home must be valid");

    assert_eq!(home.account_home(), Path::new("/Users/podway"));
    assert_eq!(home.as_path(), Path::new("/Users/podway/.podway"));
    assert_eq!(home.user_id(), 501);
}

#[test]
fn aut_home_001_effective_user_resolution_matches_the_os_account_database() {
    let effective_user_id = nix::unistd::geteuid();
    if effective_user_id.is_root() {
        assert!(matches!(
            PodwayHomeV1::for_effective_user(),
            Err(ServicePathErrorV1::RootUser)
        ));
        return;
    }

    let account = nix::unistd::User::from_uid(effective_user_id)
        .expect("effective user lookup must succeed")
        .expect("effective user must exist");
    let home = PodwayHomeV1::for_effective_user().expect("Podway home must resolve");
    assert_eq!(home.account_home(), account.dir);
    assert_eq!(home.as_path(), account.dir.join(".podway"));
    assert_eq!(home.user_id(), effective_user_id.as_raw());
}

#[test]
fn aut_home_001_podway_home_rejects_root_and_invalid_account_homes() {
    assert!(matches!(
        PodwayHomeV1::from_account_home("/var/root", 0),
        Err(ServicePathErrorV1::RootUser)
    ));
    assert!(matches!(
        PodwayHomeV1::from_account_home("Users/podway", 501),
        Err(ServicePathErrorV1::Relative {
            field: "account_home",
            ..
        })
    ));
    assert!(matches!(
        PodwayHomeV1::from_account_home("/Users/../podway", 501),
        Err(ServicePathErrorV1::Unnormalized {
            field: "account_home",
            ..
        })
    ));
}

#[test]
fn arc_008_service_v1_exposes_exact_global_runtime_paths() {
    let paths = match ServiceRuntimePathsV1::for_account_home("/Users/podway", 501) {
        Ok(value) => value,
        Err(_) => panic!("fixture service paths must be valid"),
    };

    assert_eq!(
        paths.launch_agent_path().as_path(),
        std::path::Path::new("/Users/podway/Library/LaunchAgents/dev.podway.podwayd.plist")
    );
    assert_eq!(
        paths.metadata_index_path().as_path(),
        std::path::Path::new("/Users/podway/.podway/state/service.json")
    );
    assert_eq!(
        paths.workspace_registry_path().as_path(),
        std::path::Path::new("/Users/podway/.podway/state/workspaces.json")
    );
    assert_eq!(
        paths.log_path().as_path(),
        std::path::Path::new("/Users/podway/.podway/logs/podwayd.log")
    );
    assert_eq!(
        paths.runtime_directory().as_path(),
        std::path::Path::new("/Users/podway/.podway/run")
    );
    assert_eq!(
        paths.socket_path().as_path(),
        std::path::Path::new("/Users/podway/.podway/run/podwayd.sock")
    );
    assert_eq!(
        paths.global_lock_path().as_path(),
        std::path::Path::new("/Users/podway/.podway/run/podwayd.lock")
    );
    assert_ne!(paths.metadata_index_path(), paths.workspace_registry_path());
}
#[test]
fn install_and_update_reject_runtime_paths_outside_manager_configuration() {
    let configured_paths = service_paths();
    let mismatched_paths = ServiceRuntimePathsV1::from_directories(
        "/Users/podway/Library/LaunchAgents",
        "/Users/podway/Library/Application Support/Podway",
        "/Users/podway/Library/Logs/Podway",
        "/var/folders/podway/other-runtime",
    )
    .expect("mismatched fixture paths");
    let manager = RecordingServiceManagerV1::new(
        RecordingServiceCommandRunnerV1::new([]),
        FixedServiceClockV1::new(UnixMillis::new(1)),
        configured_paths,
    );
    let spec = InstallSpecV1::new(
        LocalPlatformPathV1::new("/Applications/Podway/podwayd").expect("binary path"),
        podway_service::ServiceLabelV1::podwayd(),
        mismatched_paths,
    );

    for result in [manager.install(spec.clone()), manager.update(spec)] {
        assert!(matches!(
            result,
            Err(ServiceErrorV1::OperationFailureV1 {
                operation,
                source,
            }) if (operation == ServiceOperationV1::Install || operation == ServiceOperationV1::Update)
                && matches!(
                    source.as_ref(),
                    ServiceErrorV1::IoV1 {
                        operation: Some(source_operation),
                        message,
                    } if *source_operation == operation
                        && message == "install specification runtime paths differ from manager configuration"
                )
        ));
    }
    assert!(
        manager
            .command_runner()
            .recorded_commands()
            .expect("recorded commands")
            .is_empty()
    );
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
        ServiceRuntimePathsV1::for_account_home("/Users/podway", 0),
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
    let invalid_utf8 = PathBuf::from(OsString::from_vec(
        b"/Applications/Podway/\xff/podwayd".to_vec(),
    ));
    assert!(matches!(
        LocalPlatformPathV1::new(&invalid_utf8),
        Err(ServicePathErrorV1::Unnormalized {
            field: "local_platform_path",
            ..
        })
    ));
    assert!(
        LocalPlatformPathV1::new("/Applications/Podway/\u{fffd}/podwayd").is_ok(),
        "a real replacement character remains a distinct valid UTF-8 path"
    );
}
#[test]
fn service_paths_reject_c0_and_delete_characters() {
    for character in ['\0', '\u{001f}', '\u{007f}'] {
        let path = format!("/Applications/Podway/daemon{character}");
        assert!(matches!(
            LocalPlatformPathV1::new(path),
            Err(ServicePathErrorV1::Unnormalized {
                field: "local_platform_path",
                ..
            })
        ));
    }
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
        ServiceErrorV1::InvalidExecutableV1 {
            message: "invalid executable".to_owned(),
        },
        ServiceErrorV1::ContractMismatchV1 {
            expected_product: "podway".to_owned(),
            actual_product: Some("other".to_owned()),
            expected_manifest_digest: "sha256:expected".to_owned(),
            actual_manifest_digest: Some("sha256:actual".to_owned()),
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
        ServiceErrorV1::OutputLimitExceededV1 {
            limit_bytes: 65_536,
        },
        ServiceErrorV1::LaunchctlTimeoutV1 { timeout_ms: 30_000 },
        ServiceErrorV1::OperationFailureV1 {
            operation: ServiceOperationV1::Status,
            source: Box::new(ServiceErrorV1::InvalidMetadataV1 {
                message: "typed metadata failure".to_owned(),
            }),
        },
    ] {
        match error {
            ServiceErrorV1::InvalidMetadataV1 { message } => {
                assert_eq!(message, "invalid metadata");
            }
            ServiceErrorV1::InvalidExecutableV1 { message } => {
                assert_eq!(message, "invalid executable");
            }
            ServiceErrorV1::ContractMismatchV1 {
                expected_product,
                actual_product,
                expected_manifest_digest,
                actual_manifest_digest,
            } => {
                assert_eq!(expected_product, "podway");
                assert_eq!(actual_product.as_deref(), Some("other"));
                assert_eq!(expected_manifest_digest, "sha256:expected");
                assert_eq!(actual_manifest_digest.as_deref(), Some("sha256:actual"));
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
            ServiceErrorV1::OutputLimitExceededV1 { limit_bytes } => {
                assert_eq!(limit_bytes, 65_536);
            }
            ServiceErrorV1::LaunchctlTimeoutV1 { timeout_ms } => {
                assert_eq!(timeout_ms, 30_000);
            }
            ServiceErrorV1::OperationFailureV1 { operation, source } => {
                assert_eq!(operation, ServiceOperationV1::Status);
                assert!(matches!(
                    *source,
                    ServiceErrorV1::InvalidMetadataV1 { ref message }
                        if message == "typed metadata failure"
                ));
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
        Err(ServiceErrorV1::OperationFailureV1 {
            operation: ServiceOperationV1::Logs,
            source,
        }) if matches!(source.as_ref(), ServiceErrorV1::LogUnavailableV1 { .. })
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
        Err(ServiceErrorV1::OperationFailureV1 {
            operation: ServiceOperationV1::Start,
            source,
        }) if matches!(
            source.as_ref(),
            ServiceErrorV1::IoV1 {
                operation: Some(ServiceOperationV1::Start),
                message,
            } if message == "runner returned unexpected stopped outcome"
        )
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
        Err(ServiceErrorV1::OperationFailureV1 {
            operation: ServiceOperationV1::Logs,
            source,
        }) if matches!(
            source.as_ref(),
            ServiceErrorV1::IoV1 {
                operation: Some(ServiceOperationV1::Logs),
                message,
            } if message == "runner returned status result; expected log_location"
        )
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
        Err(ServiceErrorV1::OperationFailureV1 {
            operation: ServiceOperationV1::Start,
            source,
        }) if matches!(
            source.as_ref(),
            ServiceErrorV1::IoV1 {
                operation: Some(ServiceOperationV1::Start),
                message,
            } if message == "recording service command runner has no recorded result"
        )
    ));

    let commands = match manager.command_runner().recorded_commands() {
        Ok(value) => value,
        Err(_) => panic!("recording runner state must be readable"),
    };
    assert_eq!(commands.len(), 1);
}
fn phase6_native_daemon_bytes() -> Vec<u8> {
    let mut bytes = vec![0_u8; 40];
    bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
    bytes[4..8].copy_from_slice(&0x0100_000c_u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&1_u32.to_le_bytes());
    bytes[20..24].copy_from_slice(&8_u32.to_le_bytes());
    bytes[32..36].copy_from_slice(&0x32_u32.to_le_bytes());
    bytes[36..40].copy_from_slice(&8_u32.to_le_bytes());
    bytes
}

#[derive(Clone, Default)]
struct Phase6Filesystem {
    files: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
    events: Arc<Mutex<Vec<String>>>,
    fail_writes: Arc<Mutex<bool>>,
}

impl Phase6Filesystem {
    fn record(&self, event: impl Into<String>) {
        self.events.lock().expect("test lock").push(event.into());
    }
}

impl ServiceFilesystemV1 for Phase6Filesystem {
    fn exists(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
        let files = self.files.lock().expect("test lock");
        Ok(files.contains_key(path) || files.keys().any(|entry| entry.parent() == Some(path)))
    }

    fn is_executable(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
        Ok(path.starts_with("/Applications/Podway/"))
    }

    fn create_directory(&self, path: &Path, _: u32) -> Result<(), ServiceFilesystemErrorV1> {
        self.record(format!("mkdir:{}", path.display()));
        Ok(())
    }

    fn read_file_bounded(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, ServiceFilesystemErrorV1> {
        let bytes = if path.starts_with("/Applications/Podway/") {
            self.files
                .lock()
                .expect("test lock")
                .get(path)
                .cloned()
                .unwrap_or_else(phase6_native_daemon_bytes)
        } else {
            self.files
                .lock()
                .expect("test lock")
                .get(path)
                .cloned()
                .ok_or_else(|| ServiceFilesystemErrorV1::other("not found"))?
        };
        if bytes.len() > maximum_bytes {
            return Err(ServiceFilesystemErrorV1::limit_exceeded(
                "file exceeds read bound",
            ));
        }
        Ok(bytes)
    }

    fn write_atomically(
        &self,
        path: &Path,
        contents: &[u8],
        _: u32,
    ) -> Result<(), ServiceFilesystemErrorV1> {
        let publication_state = serde_json::from_slice::<serde_json::Value>(contents)
            .ok()
            .and_then(|metadata| metadata["publication_state"].as_str().map(str::to_owned))
            .map(|state| format!(":{state}"))
            .unwrap_or_default();
        if *self.fail_writes.lock().expect("test lock") {
            return Err(ServiceFilesystemErrorV1::other(
                "injected atomic write failure",
            ));
        }
        self.record(format!("write:{}{publication_state}", path.display()));
        self.files
            .lock()
            .expect("test lock")
            .insert(path.to_path_buf(), contents.to_vec());
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        if self.files.lock().expect("test lock").remove(path).is_some() {
            self.record(format!("remove:{}", path.display()));
        }
        Ok(())
    }
    fn list_directory_bounded(
        &self,
        path: &Path,
        maximum_entries: usize,
    ) -> Result<Vec<PathBuf>, ServiceFilesystemErrorV1> {
        let mut entries = self
            .files
            .lock()
            .expect("test lock")
            .keys()
            .filter(|entry| entry.parent() == Some(path))
            .cloned()
            .collect::<Vec<_>>();
        entries.sort();
        if entries.len() > maximum_entries {
            return Err(ServiceFilesystemErrorV1::limit_exceeded(
                "test directory exceeds entry limit",
            ));
        }
        Ok(entries)
    }

    fn remove_directory(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        if self
            .files
            .lock()
            .expect("test lock")
            .keys()
            .any(|entry| entry.parent() == Some(path))
        {
            return Err(ServiceFilesystemErrorV1::other(
                "test directory is not empty",
            ));
        }
        self.record(format!("rmdir:{}", path.display()));
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
        let is_bootstrap = arguments
            .first()
            .is_some_and(|argument| argument == "bootstrap");
        let is_print = arguments
            .first()
            .is_some_and(|argument| argument == "print");
        let exit_status = if is_bootstrap {
            self.bootstrap_status
        } else if is_print {
            self.print_status
        } else {
            0
        };
        let (stdout, stderr) = if is_print && exit_status == 0 {
            (
                format!(
                    "{} = {{\n\tpid = 4242\n}}\n",
                    arguments.get(1).expect("print target")
                ),
                String::new(),
            )
        } else if is_print && exit_status == 113 {
            (
                String::new(),
                "Bad request.\nCould not find service \"dev.podway.podwayd\" in domain for user gui: 501\n"
                    .to_owned(),
            )
        } else if exit_status == 0 {
            (String::new(), String::new())
        } else {
            (String::new(), "scripted launchctl failure".to_owned())
        };
        Ok(LaunchctlOutputV1 {
            exit_status,
            stdout,
            stderr,
        })
    }
}
#[derive(Clone)]
struct ExactPrintLaunchctl {
    output: LaunchctlOutputV1,
}

impl LaunchctlRunnerV1 for ExactPrintLaunchctl {
    fn run(&self, _: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        Ok(self.output.clone())
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
fn phase6_install_emits_complete_authenticated_plist_for_xml_sensitive_paths() {
    let filesystem = Phase6Filesystem::default();
    let binary = PathBuf::from(r#"/Applications/Podway/daemon&<>'".bin"#);
    let paths = ServiceRuntimePathsV1::from_directories(
        r#"/Users/podway/Library/LaunchAgents&<>'""#,
        r#"/Users/podway/Library/Application Support/Podway&<>'""#,
        r#"/Users/podway/Library/Logs/Podway&<>'""#,
        r#"/var/folders/podway/runtime&<>'""#,
    )
    .expect("XML-sensitive fixture paths remain valid");
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 113,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_000)),
        501,
    )
    .expect("non-root service runner");

    assert!(matches!(
        runner.run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_000),
            spec: InstallSpecV1::new(
                LocalPlatformPathV1::new(&binary).expect("XML-sensitive binary path"),
                podway_service::ServiceLabelV1::podwayd(),
                paths.clone(),
            ),
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(_)
        ))
    ));

    let plist = String::from_utf8(
        filesystem
            .files
            .lock()
            .expect("test lock")
            .get(paths.launch_agent_path().as_path())
            .cloned()
            .expect("installed plist"),
    )
    .expect("plist UTF-8");
    let xml_escape = |value: &Path| {
        value
            .display()
            .to_string()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    };
    let escaped_log = xml_escape(paths.log_path().as_path());
    assert!(plist.contains(&format!("<string>{escaped_log}</string>")));
    assert!(plist.contains("<key>Label</key>\n  <string>dev.podway.podwayd</string>"));
    assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
    assert!(
        plist.contains(
            "<key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>"
        )
    );
    assert!(plist.contains("<key>ThrottleInterval</key>\n  <integer>5</integer>"));
    assert!(plist.contains("<key>ProcessType</key>\n  <string>Background</string>"));
    assert_eq!(
        plist
            .match_indices(&format!("<string>{escaped_log}</string>"))
            .count(),
        2,
        "both standard output and error paths must use the escaped log path"
    );

    let receipt: serde_json::Value = serde_json::from_slice(
        filesystem
            .files
            .lock()
            .expect("test lock")
            .get(paths.metadata_index_path().as_path())
            .expect("receipt metadata"),
    )
    .expect("receipt metadata JSON");
    for (key, plist_key) in [
        ("generation", "PodwayGeneration"),
        ("daemon_identity", "PodwayDaemonSha256"),
    ] {
        let value = receipt[key].as_str().expect("authenticated receipt value");
        assert!(plist.contains(&format!(
            "<key>{plist_key}</key>\n  <string>{value}</string>"
        )));
    }
    let installed_binary = PathBuf::from(
        receipt["daemon_binary"]
            .as_str()
            .expect("receipt daemon path"),
    );
    let daemon_identity = receipt["daemon_identity"]
        .as_str()
        .expect("receipt daemon identity");
    assert_eq!(installed_binary, binary);
    assert_eq!(
        receipt["socket_path"],
        paths.socket_path().as_path().display().to_string()
    );
    let escaped_installed_binary = xml_escape(&installed_binary);
    let escaped_socket = xml_escape(paths.socket_path().as_path());
    assert!(plist.contains(&format!(
        "<key>ProgramArguments</key>\n  <array>\n    <string>{escaped_installed_binary}</string>\n    <string>--service</string>\n    <string>--socket</string>\n    <string>{escaped_socket}</string>\n  </array>"
    )));
    let expected_plist = include_str!("../../../docs/spec/launchagent.plist.template")
        .replace(
            "__PODWAY_GENERATION__",
            receipt["generation"].as_str().unwrap(),
        )
        .replace("__PODWAYD_SHA256__", daemon_identity)
        .replace("__PODWAYD_ABSOLUTE_PATH__", &escaped_installed_binary)
        .replace("__PODWAYD_SOCKET_PATH__", &escaped_socket)
        .replace("__PODWAYD_LOG_PATH__", &escaped_log);
    assert_eq!(
        plist, expected_plist,
        "the reference template keys and static values must exactly match installed output"
    );
    assert!(plist.contains(&xml_escape(&binary)));
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
    let relevant_events = events
        .iter()
        .filter(|event| {
            event.as_str() == format!("write:{plist}")
                || event.as_str() == format!("write:{metadata}:prepared")
                || event.as_str() == format!("write:{metadata}:receipt_durable")
                || event.starts_with("launchctl:bootstrap gui/501")
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        relevant_events,
        [
            format!("write:{metadata}:prepared"),
            format!("write:{plist}"),
            format!(
                "launchctl:bootstrap gui/501 {}",
                service_paths().launch_agent_path().as_path().display()
            ),
            format!("write:{metadata}:receipt_durable"),
        ]
    );
    let receipt = filesystem
        .files
        .lock()
        .expect("test lock")
        .get(service_paths().metadata_index_path().as_path())
        .cloned()
        .expect("receipt metadata");
    let receipt: serde_json::Value =
        serde_json::from_slice(&receipt).expect("receipt metadata must be parseable JSON");
    assert_eq!(receipt["publication_state"], "receipt_durable");
    assert_eq!(receipt["label"], "dev.podway.podwayd");
    assert_eq!(receipt["daemon_binary"], "/Applications/Podway/podwayd");
    assert_eq!(
        receipt["socket_path"],
        service_paths()
            .socket_path()
            .as_path()
            .display()
            .to_string()
    );
    assert_eq!(receipt["installed_at"], 3_000);
    assert_eq!(receipt["updated_at"], 3_000);
    assert!(
        receipt["generation"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
}

#[test]
fn phase6_install_launchctl_failure_preserves_prepared_metadata() {
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
    let metadata = filesystem
        .files
        .lock()
        .expect("test lock")
        .get(service_paths().metadata_index_path().as_path())
        .cloned()
        .expect("prepared metadata must remain for explicit repair");
    let metadata: serde_json::Value =
        serde_json::from_slice(&metadata).expect("prepared metadata must be parseable JSON");
    assert_eq!(metadata["version"], 1);
    assert_eq!(metadata["label"], "dev.podway.podwayd");
    assert_eq!(metadata["daemon_binary"], "/Applications/Podway/podwayd");
    assert_eq!(
        metadata["socket_path"],
        service_paths()
            .socket_path()
            .as_path()
            .display()
            .to_string()
    );
    assert_eq!(metadata["installed_at"], 3_001);
    assert_eq!(metadata["updated_at"], 3_001);
    assert_eq!(metadata["publication_state"], "prepared");
    assert!(
        metadata["daemon_identity"]
            .as_str()
            .is_some_and(|value| value.len() == 64)
    );
    assert!(
        metadata["generation"]
            .as_str()
            .is_some_and(|value| value.len() == 64)
    );
    let events = filesystem.events.lock().expect("test lock").clone();
    let relevant_events = events
        .iter()
        .filter(|event| {
            event.starts_with("write:") || event.starts_with("launchctl:bootstrap gui/501")
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        relevant_events,
        [
            format!(
                "write:{}:prepared",
                service_paths().metadata_index_path().as_path().display()
            ),
            format!(
                "write:{}",
                service_paths().launch_agent_path().as_path().display()
            ),
            format!(
                "launchctl:bootstrap gui/501 {}",
                service_paths().launch_agent_path().as_path().display()
            ),
        ]
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

    let receipt: serde_json::Value = serde_json::from_slice(
        &filesystem.files.lock().expect("test lock")
            [service_paths().metadata_index_path().as_path()],
    )
    .expect("receipt metadata");
    let installed_binary = receipt["daemon_binary"]
        .as_str()
        .expect("receipt daemon path");
    let plist = String::from_utf8(
        filesystem.files.lock().expect("test lock")[service_paths().launch_agent_path().as_path()]
            .clone(),
    )
    .expect("plist UTF-8");
    assert!(plist.contains(&format!("<string>{installed_binary}</string>")));
    assert_eq!(installed_binary, "/Applications/Podway/podwayd");
    assert!(plist.contains("<string>--service</string>"));
    assert!(plist.contains("<string>--socket</string>"));
    assert!(plist.contains(&format!(
        "<string>{}</string>",
        service_paths().socket_path().as_path().display()
    )));
    assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
    assert!(plist.contains("<key>SuccessfulExit</key>\n    <false/>"));
}
#[test]
fn phase6_restart_leaves_socket_recovery_to_daemon_endpoint_then_bootstraps() {
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
    let bootstrap = events
        .iter()
        .rposition(|event| event.starts_with("launchctl:bootstrap gui/501"))
        .expect("bootstrap");
    assert!(bootout < bootstrap);
    assert!(!events.iter().any(|event| event
        == &format!(
            "remove:{}",
            service_paths().socket_path().as_path().display()
        )));
}
#[test]
fn phase6_prepared_retry_delegates_stale_socket_recovery_and_publishes_receipt() {
    let filesystem = Phase6Filesystem::default();
    let failing_runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 1,
            print_status: 113,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_004)),
        501,
    )
    .expect("non-root service runner");
    assert!(matches!(
        failing_runner.run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_004),
            spec: phase6_spec(),
        }),
        Err(ServiceErrorV1::LaunchctlFailureV1 { .. })
    ));
    filesystem.files.lock().expect("test lock").insert(
        service_paths().socket_path().as_path().to_path_buf(),
        Vec::new(),
    );
    filesystem.events.lock().expect("test lock").clear();

    let retry_runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 113,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_004)),
        501,
    )
    .expect("non-root service runner");
    assert!(matches!(
        retry_runner.run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_004),
            spec: phase6_spec(),
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(_)
        ))
    ));

    let events = filesystem.events.lock().expect("test lock").clone();
    let bootstrap = events
        .iter()
        .position(|event| event.starts_with("launchctl:bootstrap gui/501"))
        .expect("bootstrap");
    let receipt = events
        .iter()
        .position(|event| {
            event
                == &format!(
                    "write:{}:receipt_durable",
                    service_paths().metadata_index_path().as_path().display()
                )
        })
        .expect("receipt publication");
    assert!(bootstrap < receipt);
    assert!(!events.iter().any(|event| event
        == &format!(
            "remove:{}",
            service_paths().socket_path().as_path().display()
        )));
}

#[test]
fn phase6_start_does_not_unlink_socket_before_daemon_endpoint_validation() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 113,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_005)),
        501,
    )
    .expect("non-root service runner");
    runner
        .run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_005),
            spec: phase6_spec(),
        })
        .expect("initial install");
    filesystem.files.lock().expect("test lock").insert(
        service_paths().socket_path().as_path().to_path_buf(),
        Vec::new(),
    );
    filesystem.events.lock().expect("test lock").clear();

    assert!(matches!(
        runner.run(ServiceCommandV1::Start {
            requested_at: UnixMillis::new(3_005),
            paths: service_paths(),
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(_)
        ))
    ));
    let events = filesystem.events.lock().expect("test lock").clone();
    let bootstrap = events
        .iter()
        .position(|event| event.starts_with("launchctl:bootstrap gui/501"))
        .expect("bootstrap");
    assert!(
        !events.iter().any(|event| event
            == &format!(
                "remove:{}",
                service_paths().socket_path().as_path().display()
            )),
        "only the daemon endpoint guard may validate and remove a stale socket"
    );
    assert!(bootstrap < events.len());
    let receipt: serde_json::Value = serde_json::from_slice(
        filesystem
            .files
            .lock()
            .expect("test lock")
            .get(service_paths().metadata_index_path().as_path())
            .expect("receipt metadata"),
    )
    .expect("receipt JSON");
    assert_eq!(receipt["publication_state"], "receipt_durable");
}

#[test]
fn phase6_update_restarts_without_bypassing_daemon_socket_validation() {
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
    let old_identity = serde_json::from_slice::<serde_json::Value>(
        &filesystem.files.lock().expect("test lock")
            [service_paths().metadata_index_path().as_path()],
    )
    .expect("initial receipt")["daemon_identity"]
        .as_str()
        .expect("initial daemon identity")
        .to_owned();
    filesystem.files.lock().expect("test lock").insert(
        service_paths().socket_path().as_path().to_path_buf(),
        Vec::new(),
    );
    filesystem.events.lock().expect("test lock").clear();
    let mut changed_daemon = phase6_native_daemon_bytes();
    changed_daemon.push(1);
    filesystem.files.lock().expect("test lock").insert(
        PathBuf::from("/Applications/Podway/podwayd"),
        changed_daemon,
    );
    let changed = phase6_spec();
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
    let bootout = events
        .iter()
        .position(|event| event.starts_with("launchctl:bootout gui/501/"))
        .expect("bootout");
    let bootstrap = events
        .iter()
        .position(|event| event.starts_with("launchctl:bootstrap gui/501"))
        .expect("bootstrap");
    let receipt = events
        .iter()
        .position(|event| {
            event
                == &format!(
                    "write:{}:receipt_durable",
                    service_paths().metadata_index_path().as_path().display()
                )
        })
        .expect("receipt publication");
    assert!(bootout < bootstrap && bootstrap < receipt);
    assert!(!events.iter().any(|event| event
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
            .contains("<string>/Applications/Podway/podwayd</string>")
    );
    let receipt: serde_json::Value = serde_json::from_slice(
        &filesystem.files.lock().expect("test lock")
            [service_paths().metadata_index_path().as_path()],
    )
    .expect("current receipt");
    assert_eq!(receipt["daemon_binary"], "/Applications/Podway/podwayd");
    assert_ne!(receipt["daemon_identity"], old_identity);
    assert!(matches!(
        runner.run(ServiceCommandV1::Update {
            requested_at: UnixMillis::new(3_004),
            spec: phase6_spec(),
        }),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::AlreadyInDesiredStateV1(_)
        ))
    ));
}

#[test]
fn phase6_start_stop_status_and_logs_cover_installed_lifecycle_states() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 113,
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
    match runner
        .run(ServiceCommandV1::Status {
            requested_at: UnixMillis::new(3_005),
            paths: service_paths(),
        })
        .expect("documented not-loaded launchctl output must classify as stopped")
    {
        ServiceCommandResultV1::Status(ServiceStatusV1::StoppedV1(stopped)) => {
            assert_eq!(stopped.observed_at(), UnixMillis::new(3_005));
            let metadata = stopped
                .metadata()
                .expect("stopped state retains receipt metadata");
            assert_eq!(metadata.version(), 1);
            assert_eq!(metadata.label(), "dev.podway.podwayd");
            assert_eq!(
                metadata.daemon_binary(),
                Path::new("/Applications/Podway/podwayd")
            );
            assert_eq!(
                metadata.socket_path(),
                service_paths().socket_path().as_path()
            );
            assert_eq!(metadata.installed_at(), UnixMillis::new(3_005));
            assert_eq!(metadata.updated_at(), UnixMillis::new(3_005));
        }
        other => panic!("exact not-loaded fixture must produce stopped status, got {other:?}"),
    }
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
fn phase6_not_loaded_accepts_only_documented_current_and_legacy_bytes() {
    let filesystem = Phase6Filesystem::default();
    let setup = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 0,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_006)),
        501,
    )
    .expect("setup runner");
    setup
        .run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_006),
            spec: phase6_spec(),
        })
        .expect("install status fixture");

    let current =
        "Bad request.\nCould not find service \"dev.podway.podwayd\" in domain for user gui: 501";
    let legacy = "Could not find service \"dev.podway.podwayd\" in domain for gui/501";
    for (exit_status, documented) in [(113, current), (3, legacy)] {
        for newline in ["", "\n"] {
            let runner = MacosServiceCommandRunnerV1::new(
                filesystem.clone(),
                ExactPrintLaunchctl {
                    output: LaunchctlOutputV1 {
                        exit_status,
                        stdout: String::new(),
                        stderr: format!("{documented}{newline}"),
                    },
                },
                FixedServiceClockV1::new(UnixMillis::new(3_006)),
                501,
            )
            .expect("exact-print runner");
            assert!(matches!(
                runner.run(ServiceCommandV1::Status {
                    requested_at: UnixMillis::new(3_006),
                    paths: service_paths(),
                }),
                Ok(ServiceCommandResultV1::Status(ServiceStatusV1::StoppedV1(
                    _
                )))
            ));
        }
    }

    for output in [
        LaunchctlOutputV1 {
            exit_status: 113,
            stdout: String::new(),
            stderr: format!("{current}\n\n"),
        },
        LaunchctlOutputV1 {
            exit_status: 113,
            stdout: "unexpected".to_owned(),
            stderr: current.to_owned(),
        },
        LaunchctlOutputV1 {
            exit_status: 3,
            stdout: String::new(),
            stderr: format!(" {legacy}"),
        },
        LaunchctlOutputV1 {
            exit_status: 3,
            stdout: String::new(),
            stderr: format!("{legacy}\n "),
        },
    ] {
        let runner = MacosServiceCommandRunnerV1::new(
            filesystem.clone(),
            ExactPrintLaunchctl { output },
            FixedServiceClockV1::new(UnixMillis::new(3_006)),
            501,
        )
        .expect("near-match runner");
        assert!(matches!(
            runner.run(ServiceCommandV1::Status {
                requested_at: UnixMillis::new(3_006),
                paths: service_paths(),
            }),
            Err(ServiceErrorV1::LaunchctlFailureV1 {
                operation: ServiceOperationV1::Status,
                ..
            })
        ));
    }
}

#[test]
fn phase6_uninstall_preserves_registry_and_logs_unless_purge_is_explicit() {
    let filesystem = Phase6Filesystem::default();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 113,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_006)),
        501,
    )
    .expect("non-root service runner");
    runner
        .run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_006),
            spec: phase6_spec(),
        })
        .expect("fixture service must install before uninstall");
    for path in [
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
    for index in 1..=5 {
        filesystem.files.lock().expect("test lock").insert(
            PathBuf::from(format!(
                "{}.{index}",
                service_paths().log_path().as_path().display()
            )),
            Vec::new(),
        );
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
    let events = filesystem.events.lock().expect("test lock");
    assert!(events.iter().any(|event| {
        event == &format!("remove:{}", service_paths().log_path().as_path().display())
    }));
    for index in 1..=5 {
        assert!(events.iter().any(|event| {
            event
                == &format!(
                    "remove:{}.{}",
                    service_paths().log_path().as_path().display(),
                    index
                )
        }));
    }
    assert!(!events.iter().any(|event| event.starts_with("purge:")));
}

#[test]
fn aut_home_002_runtime_socket_path_rejects_overlong_account_home() {
    let account_home = format!("/{}", "a".repeat(120));
    assert!(matches!(
        ServiceRuntimePathsV1::for_account_home(account_home, 501),
        Err(ServicePathErrorV1::SocketPathTooLong { .. })
    ));
}

#[test]
fn aut_sock_001_explicit_socket_replaces_only_the_endpoint() {
    let paths = ServiceRuntimePathsV1::for_account_home("/Users/podway", 501)
        .expect("canonical service paths");
    let explicit = paths
        .clone()
        .with_socket_path("/var/run/podway-explicit.sock")
        .expect("absolute normalized explicit endpoint");

    assert_eq!(
        explicit.socket_path().as_path(),
        Path::new("/var/run/podway-explicit.sock")
    );
    assert_eq!(explicit.global_lock_path(), paths.global_lock_path());
    assert_eq!(explicit.metadata_index_path(), paths.metadata_index_path());
    assert_eq!(
        explicit.workspace_registry_path(),
        paths.workspace_registry_path()
    );
}

#[test]
fn aut_sock_002_explicit_socket_rejects_relative_and_unnormalized_paths() {
    let paths = ServiceRuntimePathsV1::for_account_home("/Users/podway", 501)
        .expect("canonical service paths");
    for invalid in ["relative.sock", "~/podwayd.sock", "/tmp/../podwayd.sock"] {
        assert!(
            paths.clone().with_socket_path(invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn explicit_socket_enforces_the_macos_sun_path_capacity_boundary() {
    let paths = ServiceRuntimePathsV1::for_account_home("/Users/podway", 501)
        .expect("canonical service paths");
    let longest_bindable = format!("/{}", "s".repeat(102));
    let first_unbindable = format!("/{}", "s".repeat(103));

    assert!(paths.clone().with_socket_path(longest_bindable).is_ok());
    assert!(matches!(
        paths.with_socket_path(first_unbindable),
        Err(ServicePathErrorV1::SocketPathTooLong { .. })
    ));
}

#[test]
fn aut_daemon_001_metadata_requires_and_returns_the_installed_socket() {
    let metadata = serde_json::json!({
        "version": 1,
        "label": "dev.podway.podwayd",
        "daemon_binary": "/Applications/Podway/podwayd",
        "daemon_identity": "0".repeat(64),
        "socket_path": "/Users/podway/.podway/run/custom.sock",
        "artifact_role": "production_daemon",
        "installed_at": 1,
        "updated_at": 1,
        "publication_state": "receipt_durable",
        "generation": "1".repeat(64),
    });
    let bytes = serde_json::to_vec(&metadata).expect("metadata fixture");
    assert_eq!(
        installed_socket_path_from_metadata_v1(&bytes).expect("installed endpoint"),
        Path::new("/Users/podway/.podway/run/custom.sock")
    );

    let mut legacy = metadata;
    legacy
        .as_object_mut()
        .expect("metadata object")
        .remove("socket_path");
    assert!(
        installed_socket_path_from_metadata_v1(
            &serde_json::to_vec(&legacy).expect("legacy metadata fixture")
        )
        .is_err(),
        "legacy metadata without an endpoint must not fall back"
    );
}
#[test]
fn phase6_direct_runtime_socket_path_rejects_unbindable_path() {
    let runtime_directory = format!("/{}", "a".repeat(120));
    assert!(matches!(
        ServiceRuntimePathsV1::from_directories(
            "/Users/podway/Library/LaunchAgents",
            "/Users/podway/Library/Application Support/Podway",
            "/Users/podway/Library/Logs/Podway",
            runtime_directory,
        ),
        Err(ServicePathErrorV1::SocketPathTooLong { .. })
    ));
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
    assert!(matches!(
        runner.run(ServiceCommandV1::Status {
            requested_at: UnixMillis::new(3_007),
            paths: service_paths(),
        }),
        Err(ServiceErrorV1::OperationFailureV1 { source, .. })
            if matches!(source.as_ref(), ServiceErrorV1::InvalidMetadataV1 { message }
                if message == "service is loaded without a coherent publication")
    ));
    filesystem.files.lock().expect("test lock").insert(
        service_paths().launch_agent_path().as_path().to_path_buf(),
        b"legacy plist without a generation marker".to_vec(),
    );
    filesystem.files.lock().expect("test lock").insert(
        service_paths().metadata_index_path().as_path().to_path_buf(),
        br#"{"version":1,"version":1,"label":"dev.podway.podwayd","daemon_binary":"/Applications/Podway/podwayd","installed_at":1,"updated_at":1,"publication_state":"receipt_durable","generation":"0000000000000000000000000000000000000000000000000000000000000000","daemon_identity":"0000000000000000000000000000000000000000000000000000000000000000"}"#.to_vec(),
    );
    assert!(matches!(
        runner.run(ServiceCommandV1::Status {
            requested_at: UnixMillis::new(3_007),
            paths: service_paths(),
        }),
        Err(ServiceErrorV1::OperationFailureV1 { source, .. })
            if matches!(source.as_ref(), ServiceErrorV1::InvalidMetadataV1 { .. })
    ));
    filesystem
        .files
        .lock()
        .expect("test lock")
        .remove(service_paths().launch_agent_path().as_path());
    filesystem
        .files
        .lock()
        .expect("test lock")
        .remove(service_paths().metadata_index_path().as_path());
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
        Err(ServiceErrorV1::OperationFailureV1 { source, .. })
            if matches!(source.as_ref(), ServiceErrorV1::InvalidMetadataV1 { .. })
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
        Err(ServiceErrorV1::OperationFailureV1 { source, .. })
            if matches!(source.as_ref(), ServiceErrorV1::InvalidMetadataV1 { .. })
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
            Err(ServiceErrorV1::OperationFailureV1 { source, .. })
                if matches!(source.as_ref(), ServiceErrorV1::InvalidMetadataV1 { .. })
        ));
    }
    match runner
        .run(install.clone())
        .expect("authenticated install must repair the tampered publication")
    {
        ServiceCommandResultV1::Outcome(ServiceOutcomeV1::ChangedV1(changed)) => {
            assert_eq!(changed.completed_at(), UnixMillis::new(3_008));
            let metadata = changed
                .metadata()
                .expect("repair must publish receipt metadata");
            assert_eq!(metadata.version(), 1);
            assert_eq!(metadata.label(), "dev.podway.podwayd");
            assert_eq!(
                metadata.daemon_binary(),
                Path::new("/Applications/Podway/podwayd")
            );
            assert_eq!(
                metadata.socket_path(),
                service_paths().socket_path().as_path()
            );
            assert_eq!(metadata.installed_at(), UnixMillis::new(3_008));
            assert_eq!(metadata.updated_at(), UnixMillis::new(3_008));
        }
        other => panic!("tampered publication must be repaired, got {other:?}"),
    }
    match runner
        .run(ServiceCommandV1::Status {
            requested_at: UnixMillis::new(3_008),
            paths: service_paths(),
        })
        .expect("structured loaded launchctl output must classify repaired service as running")
    {
        ServiceCommandResultV1::Status(ServiceStatusV1::RunningV1(running)) => {
            assert_eq!(running.observed_at(), UnixMillis::new(3_008));
            assert_eq!(running.process_id(), Some(4242));
            let metadata = running
                .metadata()
                .expect("running state retains receipt metadata");
            assert_eq!(metadata.version(), 1);
            assert_eq!(metadata.label(), "dev.podway.podwayd");
            assert_eq!(
                metadata.daemon_binary(),
                Path::new("/Applications/Podway/podwayd")
            );
            assert_eq!(
                metadata.socket_path(),
                service_paths().socket_path().as_path()
            );
            assert_eq!(metadata.installed_at(), UnixMillis::new(3_008));
            assert_eq!(metadata.updated_at(), UnixMillis::new(3_008));
        }
        other => panic!("structured loaded fixture must produce running status, got {other:?}"),
    }
    match runner
        .run(install)
        .expect("repaired publication must converge")
    {
        ServiceCommandResultV1::Outcome(ServiceOutcomeV1::AlreadyInDesiredStateV1(already)) => {
            assert_eq!(already.observed_at(), UnixMillis::new(3_008));
            assert_eq!(already.metadata().version(), 1);
            assert_eq!(already.metadata().label(), "dev.podway.podwayd");
            assert_eq!(
                already.metadata().daemon_binary(),
                Path::new("/Applications/Podway/podwayd")
            );
        }
        other => panic!("repaired publication must converge, got {other:?}"),
    }
    let receipt = filesystem
        .files
        .lock()
        .expect("test lock")
        .get(service_paths().metadata_index_path().as_path())
        .cloned()
        .expect("repaired receipt metadata");
    let receipt: serde_json::Value =
        serde_json::from_slice(&receipt).expect("repaired receipt metadata must be parseable JSON");
    assert_eq!(receipt["publication_state"], "receipt_durable");
    assert!(
        receipt["generation"]
            .as_str()
            .is_some_and(|value| value.len() == 64)
    );
}
#[test]
fn phase6_install_repairs_oversized_metadata_but_preserves_unsafe_corruption() {
    let filesystem = Phase6Filesystem::default();
    let metadata_path = service_paths()
        .metadata_index_path()
        .as_path()
        .to_path_buf();
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 0,
        },
        FixedServiceClockV1::new(UnixMillis::new(3_010)),
        501,
    )
    .expect("non-root service runner");
    let install = ServiceCommandV1::Install {
        requested_at: UnixMillis::new(3_010),
        spec: phase6_spec(),
    };
    runner
        .run(install.clone())
        .expect("initial install records the selected daemon");

    let unsafe_corrupt = br#"{"version":1,"label":"dev.podway.podwayd""#.to_vec();
    filesystem
        .files
        .lock()
        .expect("test lock")
        .insert(metadata_path.clone(), unsafe_corrupt.clone());
    for command in [
        ServiceCommandV1::Status {
            requested_at: UnixMillis::new(3_010),
            paths: service_paths(),
        },
        ServiceCommandV1::Update {
            requested_at: UnixMillis::new(3_010),
            spec: phase6_spec(),
        },
        install.clone(),
    ] {
        assert!(matches!(
            runner.run(command),
            Err(ServiceErrorV1::OperationFailureV1 { source, .. })
                if matches!(source.as_ref(), ServiceErrorV1::InvalidMetadataV1 { .. })
        ));
    }
    assert_eq!(
        filesystem.files.lock().expect("test lock")[&metadata_path],
        unsafe_corrupt,
        "non-limit metadata corruption must remain fail-closed"
    );

    let oversized = vec![b'x'; SERVICE_METADATA_MAX_BYTES_V1 + 1];
    filesystem
        .files
        .lock()
        .expect("test lock")
        .insert(metadata_path.clone(), oversized.clone());
    *filesystem.fail_writes.lock().expect("test lock") = true;
    assert!(matches!(
        runner.run(install.clone()),
        Err(ServiceErrorV1::IoV1 {
            operation: Some(ServiceOperationV1::Install),
            ..
        })
    ));
    assert_eq!(
        filesystem.files.lock().expect("test lock")[&metadata_path],
        oversized,
        "a failed bounded-size repair must not replace oversized metadata"
    );

    *filesystem.fail_writes.lock().expect("test lock") = false;
    assert!(matches!(
        runner.run(install),
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(_)
        ))
    ));
    let repaired = filesystem.files.lock().expect("test lock")[&metadata_path].clone();
    assert!(serde_json::from_slice::<serde_json::Value>(&repaired).is_ok());
}
#[test]
fn phase6_authenticated_generation_rejects_state_and_timestamp_tampering() {
    let filesystem = Phase6Filesystem::default();
    let clock = FixedServiceClockV1::new(UnixMillis::new(3_009));
    let runner = MacosServiceCommandRunnerV1::new(
        filesystem.clone(),
        Phase6Launchctl {
            events: filesystem.events.clone(),
            bootstrap_status: 0,
            print_status: 0,
        },
        clock,
        501,
    )
    .expect("non-root service runner");
    runner
        .run(ServiceCommandV1::Install {
            requested_at: UnixMillis::new(3_009),
            spec: phase6_spec(),
        })
        .expect("initial install");
    let metadata_path = service_paths()
        .metadata_index_path()
        .as_path()
        .to_path_buf();
    let original = filesystem.files.lock().expect("test lock")[&metadata_path].clone();
    for (field, value) in [
        (
            "publication_state",
            serde_json::Value::String("prepared".to_owned()),
        ),
        ("updated_at", serde_json::Value::from(3_010_u64)),
    ] {
        let mut tampered: serde_json::Value =
            serde_json::from_slice(&original).expect("receipt metadata JSON");
        tampered[field] = value;
        filesystem.files.lock().expect("test lock").insert(
            metadata_path.clone(),
            serde_json::to_vec(&tampered).expect("tampered metadata JSON"),
        );
        assert!(matches!(
            runner.run(ServiceCommandV1::Status {
                requested_at: UnixMillis::new(3_009),
                paths: service_paths(),
            }),
            Err(ServiceErrorV1::OperationFailureV1 { source, .. })
                if matches!(source.as_ref(), ServiceErrorV1::InvalidMetadataV1 { .. })
        ));
    }
}
