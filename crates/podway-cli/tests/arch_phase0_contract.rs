//! Phase 0 CLI boundary contracts.
//!
//! Requirement IDs: ARC-001, API-001, API-006, API-008.

use podway_cli::{ClientBoundaryError, CommandFamily, CommandRoute, ServiceOperation, route_for};
use podway_config::ConfigError;
use podway_protocol::ProtocolError;

#[test]
fn arc_001_exhaustively_routes_each_command_family_to_its_sole_boundary() {
    let cases = [
        (CommandFamily::Static, CommandRoute::Local),
        (CommandFamily::OfflineValidation, CommandRoute::Local),
        (CommandFamily::PresetMetadata, CommandRoute::Local),
        (CommandFamily::TaskRead, CommandRoute::Daemon),
        (CommandFamily::TaskMutation, CommandRoute::Daemon),
        (CommandFamily::ServiceLifecycle, CommandRoute::DirectService),
    ];

    for (family, expected_route) in cases {
        assert_eq!(
            route_for(family),
            expected_route,
            "unexpected route for {family:?}"
        );
    }
}

#[test]
fn arc_001_local_routes_do_not_select_daemon_service_store_or_git_boundaries() {
    for family in [
        CommandFamily::Static,
        CommandFamily::OfflineValidation,
        CommandFamily::PresetMetadata,
    ] {
        assert_eq!(
            route_for(family),
            CommandRoute::Local,
            "{family:?} must stay local"
        );
    }
}

#[test]
fn api_006_boundary_errors_preserve_local_daemon_and_direct_service_ownership() {
    let local = ClientBoundaryError::LocalValidation {
        message: "invalid argument".to_owned(),
    };
    assert_eq!(
        local.to_string(),
        "local validation failed: invalid argument"
    );
    assert!(std::error::Error::source(&local).is_none());

    let configuration_source = ConfigError::InvalidValue {
        field: "default_preset",
        reason: "fixture",
    };
    let configuration = ClientBoundaryError::Configuration {
        source: configuration_source.clone(),
    };
    assert_eq!(
        configuration.to_string(),
        "configuration failed: invalid default_preset: fixture"
    );
    assert_eq!(
        std::error::Error::source(&configuration)
            .expect("configuration errors retain their config source")
            .to_string(),
        configuration_source.to_string()
    );

    let daemon_connection = ClientBoundaryError::DaemonConnection {
        message: "socket unavailable".to_owned(),
    };
    assert_eq!(
        daemon_connection.to_string(),
        "daemon connection failed: socket unavailable"
    );
    assert!(std::error::Error::source(&daemon_connection).is_none());

    let protocol_source = ProtocolError::EmptyValue {
        field: "request_id",
    };
    let daemon_protocol = ClientBoundaryError::DaemonProtocol {
        source: protocol_source.clone(),
    };
    assert_eq!(
        daemon_protocol.to_string(),
        "daemon protocol failed: request_id must not be empty"
    );
    assert_eq!(
        std::error::Error::source(&daemon_protocol)
            .expect("daemon protocol errors retain their protocol source")
            .to_string(),
        protocol_source.to_string()
    );

    for (operation, expected_name) in [
        (ServiceOperation::Install, "install"),
        (ServiceOperation::Uninstall, "uninstall"),
        (ServiceOperation::Start, "start"),
        (ServiceOperation::Stop, "stop"),
        (ServiceOperation::Restart, "restart"),
        (ServiceOperation::Status, "status"),
        (ServiceOperation::Logs, "logs"),
    ] {
        let service = ClientBoundaryError::Service {
            operation,
            message: "unavailable".to_owned(),
        };
        assert_eq!(operation.to_string(), expected_name);
        assert_eq!(
            service.to_string(),
            format!("service operation {expected_name} failed: unavailable")
        );
        assert!(std::error::Error::source(&service).is_none());
    }
}
