#![forbid(unsafe_code)]

//! CLI boundary contracts and the production local daemon client.
//!
//! Command routing remains presentation-owned, while `client` performs one bounded framed exchange
//! against the service-owned Unix socket without opening workspace storage.
pub mod client;

use std::fmt;

use podway_config::ConfigError;
use podway_protocol::ProtocolError;

/// The boundary responsible for a command family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandRoute {
    /// A self-contained command that does not need a daemon or service manager.
    Local,
    /// A worktree task command sent through the daemon IPC protocol.
    Daemon,
    /// A daemon lifecycle command sent directly to the service manager.
    DirectService,
}

/// Stable command categories used to select a client boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandFamily {
    /// Help, version, and shell completion commands.
    Static,
    /// Offline procedure validation and display commands.
    OfflineValidation,
    /// Built-in preset metadata commands.
    PresetMetadata,
    /// Read-only task and workspace commands.
    TaskRead,
    /// Task and workspace mutations.
    TaskMutation,
    /// Daemon install, uninstall, lifecycle, status, and log commands.
    ServiceLifecycle,
}

/// Returns the sole owning boundary for a command family.
#[must_use]
pub const fn route_for(family: CommandFamily) -> CommandRoute {
    match family {
        CommandFamily::Static
        | CommandFamily::OfflineValidation
        | CommandFamily::PresetMetadata => CommandRoute::Local,
        CommandFamily::TaskRead | CommandFamily::TaskMutation => CommandRoute::Daemon,
        CommandFamily::ServiceLifecycle => CommandRoute::DirectService,
    }
}

/// The public response representation requested by a CLI caller.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OutputMode {
    /// Human-readable output; its wording is not a stable integration API.
    #[default]
    Text,
    /// A single versioned JSON response object defined by `podway-protocol`.
    Json,
}

/// A direct-service operation, retained on service-bound errors without coupling to a daemon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceOperation {
    Install,
    Uninstall,
    Start,
    Stop,
    Restart,
    Status,
    Logs,
}

/// Failures at the CLI's local, daemon-protocol, or direct-service boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientBoundaryError {
    /// Local argument or offline-validation failure.
    LocalValidation { message: String },
    /// A local configuration contract was invalid.
    Configuration { source: ConfigError },
    /// The daemon client could not establish or retain its local connection.
    DaemonConnection { message: String },
    /// The daemon request or response violated the protocol contract.
    DaemonProtocol { source: ProtocolError },
    /// A direct service-manager operation failed before a daemon request was made.
    Service {
        operation: ServiceOperation,
        message: String,
    },
}

impl fmt::Display for ClientBoundaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalValidation { message } => {
                write!(formatter, "local validation failed: {message}")
            }
            Self::Configuration { source } => write!(formatter, "configuration failed: {source}"),
            Self::DaemonConnection { message } => {
                write!(formatter, "daemon connection failed: {message}")
            }
            Self::DaemonProtocol { source } => {
                write!(formatter, "daemon protocol failed: {source}")
            }
            Self::Service { operation, message } => {
                write!(formatter, "service operation {operation} failed: {message}")
            }
        }
    }
}

impl std::error::Error for ClientBoundaryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Configuration { source } => Some(source),
            Self::DaemonProtocol { source } => Some(source),
            Self::LocalValidation { .. } | Self::DaemonConnection { .. } | Self::Service { .. } => {
                None
            }
        }
    }
}

impl fmt::Display for ServiceOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Install => "install",
            Self::Uninstall => "uninstall",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Status => "status",
            Self::Logs => "logs",
        };
        formatter.write_str(name)
    }
}
