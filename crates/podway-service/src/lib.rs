#![forbid(unsafe_code)]

//! Platform-service composition contracts for Podway.
//!
//! This crate owns platform-service publication, including durable LaunchAgent metadata I/O, while
//! translating direct lifecycle requests into typed runner commands with injectable command execution,
//! clocks, runtime paths, and filesystem access at the composition boundary.

use nix::{
    dir::Dir,
    errno::Errno,
    fcntl::{AtFlags, Flock, FlockArg, OFlag, open, openat, renameat},
    sys::{
        signal::{Signal, kill},
        stat::{Mode, SFlag, fchmod, fstat, fstatat, mkdirat},
    },
    unistd::{Pid, UnlinkatFlags, User, fsync, geteuid, unlinkat},
};
use podway_core::UnixMillis;
use serde::{
    Deserialize, Deserializer as _, Serialize,
    de::{IgnoredAny, MapAccess, Visitor},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    error::Error,
    fmt, fs,
    io::{ErrorKind, Read, Write},
    os::{
        fd::OwnedFd,
        unix::{ffi::OsStrExt, fs::MetadataExt, net::UnixStream, process::CommandExt},
    },
    panic::{self, AssertUnwindSafe},
    path::{Component, Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

pub const SERVICE_LABEL_V1: &str = "dev.podway.podwayd";
pub const SERVICE_METADATA_VERSION_V1: u16 = 1;
pub const SERVICE_LOG_MAX_BYTES_V1: u64 = 10 * 1024 * 1024;
pub const SERVICE_LOG_RETAINED_FILES_V1: u8 = 5;
pub const SERVICE_METADATA_MAX_BYTES_V1: usize = 16 * 1024;
pub const SERVICE_PLIST_MAX_BYTES_V1: usize = 64 * 1024;
pub const SERVICE_DAEMON_BINARY_MAX_BYTES_V1: usize = 128 * 1024 * 1024;
const SERVICE_BINARY_IDENTITY_HEX_LENGTH_V1: usize = 64;
const SERVICE_TEMPORARY_STALE_AGE_V1: Duration = Duration::from_secs(300);
const SERVICE_LIFECYCLE_LOCK_TIMEOUT_V1: Duration = Duration::from_secs(10);
const SERVICE_LIFECYCLE_LOCK_RETRY_V1: Duration = Duration::from_millis(10);
const SERVICE_TEMPORARY_SCAN_LIMIT_V1: usize = 8_192;
const SERVICE_TEMPORARY_RETAIN_LIMIT_V1: usize = 64;
const SERVICE_TEMPORARY_RETAIN_TARGET_V1: usize = 32;
static SERVICE_TEMPORARY_SEQUENCE_V1: AtomicU64 = AtomicU64::new(0);
const SERVICE_STAGED_DAEMONS_DIRECTORY_V1: &str = ".podway-daemons-v1";
const SERVICE_STAGED_DAEMONS_MAX_ENTRIES_V1: usize = 4096;
/// A non-authoritative, content-free observation emitted by the service adapter.
///
/// Variants are stable categories only; paths, command arguments, process output, metadata, and
/// error messages must never be included in an observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceObservationV1 {
    ServiceOutcome(ServiceOperationV1),
    Error(ServiceOperationV1),
    StaleSocketRemoved,
    LogRotationCompleted,
    AtomicPlistPublished,
    AtomicMetadataPublished,
    LaunchctlSideEffectRequested,
    LaunchctlSideEffectCompleted,
    UninstallLogsPreserved,
    UninstallLogsPurged,
}

/// Receives best-effort service observations. Implementations must not influence service results.
pub trait ServiceObserverV1: Send + Sync {
    fn observe(&self, observation: ServiceObservationV1);
}

/// The default observer deliberately discards every observation.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopServiceObserverV1;

impl ServiceObserverV1 for NoopServiceObserverV1 {
    fn observe(&self, _: ServiceObservationV1) {}
}

/// The only public service-lifecycle boundary.
///
/// Both the offline CLI and the daemon invoke this contract directly. It deliberately has no
/// workspace, store, Git, or daemon dependency.
pub trait ServiceManagerContractV1: Send + Sync {
    fn install(&self, spec: InstallSpecV1) -> Result<ServiceOutcomeV1, ServiceErrorV1>;
    fn logs(&self, query: LogQueryV1) -> Result<LogLocationV1, ServiceErrorV1>;
    fn restart(&self) -> Result<ServiceOutcomeV1, ServiceErrorV1>;
    fn start(&self) -> Result<ServiceOutcomeV1, ServiceErrorV1>;
    fn status(&self) -> Result<ServiceStatusV1, ServiceErrorV1>;
    fn stop(&self) -> Result<ServiceOutcomeV1, ServiceErrorV1>;
    fn uninstall(&self) -> Result<ServiceOutcomeV1, ServiceErrorV1>;
    fn update(&self, spec: InstallSpecV1) -> Result<ServiceOutcomeV1, ServiceErrorV1>;

    /// Removes the service using explicit cleanup options. The v1 no-option method remains
    /// equivalent to preserving logs.
    fn uninstall_with_options(
        &self,
        options: UninstallOptionsV1,
    ) -> Result<ServiceOutcomeV1, ServiceErrorV1> {
        if options.purge_logs() {
            return Err(ServiceErrorV1::IoV1 {
                operation: Some(ServiceOperationV1::Uninstall),
                message: "this service manager does not support log-purge options".to_owned(),
            });
        }
        self.uninstall()
    }
}

/// A source of service command execution. Production adapters may invoke `launchctl`; tests can
/// record commands without spawning a process.
pub trait ServiceCommandRunnerV1: Send + Sync {
    fn run(&self, command: ServiceCommandV1) -> Result<ServiceCommandResultV1, ServiceErrorV1>;
}

/// Supplies time to service commands so callers can keep metadata-related behavior deterministic.
pub trait ServiceClockV1: Send + Sync {
    fn now(&self) -> UnixMillis;
}

/// An absolute normalized path that is owned by the local platform service, never a workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalPlatformPathV1(PathBuf);

impl LocalPlatformPathV1 {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, ServicePathErrorV1> {
        let path = path.as_ref().to_path_buf();
        validate_service_path(&path, "local_platform_path")?;
        Ok(Self(path))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    fn service_global(path: impl AsRef<Path>) -> Result<Self, ServicePathErrorV1> {
        let path = path.as_ref().to_path_buf();
        validate_absolute_normalized_path(&path, "service_global_path")?;
        Ok(Self(path))
    }
}

/// The canonical per-user root for Podway service-global state.
///
/// Production resolution uses the effective operating-system account rather than ambient
/// environment variables. The explicit constructor exists for deterministic composition tests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PodwayHomeV1 {
    account_home: PathBuf,
    root: PathBuf,
    user_id: u32,
}

impl PodwayHomeV1 {
    /// Resolves the effective user's account home through the operating-system account database.
    pub fn for_effective_user() -> Result<Self, ServicePathErrorV1> {
        let user_id = geteuid().as_raw();
        if user_id == 0 {
            return Err(ServicePathErrorV1::RootUser);
        }
        let user = User::from_uid(geteuid())
            .map_err(|_| ServicePathErrorV1::EffectiveUserLookup { user_id })?
            .ok_or(ServicePathErrorV1::EffectiveUserNotFound { user_id })?;
        Self::from_account_home(user.dir, user_id)
    }

    /// Constructs a canonical Podway home from an explicit operating-system account home.
    pub fn from_account_home(
        account_home: impl AsRef<Path>,
        user_id: u32,
    ) -> Result<Self, ServicePathErrorV1> {
        if user_id == 0 {
            return Err(ServicePathErrorV1::RootUser);
        }
        let account_home = account_home.as_ref();
        validate_service_path(account_home, "account_home")?;
        Ok(Self {
            account_home: account_home.to_path_buf(),
            root: account_home.join(".podway"),
            user_id,
        })
    }

    pub fn account_home(&self) -> &Path {
        &self.account_home
    }

    pub fn as_path(&self) -> &Path {
        &self.root
    }

    pub const fn user_id(&self) -> u32 {
        self.user_id
    }
}

/// The fixed v1 LaunchAgent label.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceLabelV1;

impl ServiceLabelV1 {
    pub const fn podwayd() -> Self {
        Self
    }

    pub const fn as_str(self) -> &'static str {
        SERVICE_LABEL_V1
    }
}

/// All bounded global paths owned by the per-user Podway service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceRuntimePathsV1 {
    runtime_directory: LocalPlatformPathV1,
    global_lock_path: LocalPlatformPathV1,
    launch_agent_path: LocalPlatformPathV1,
    log_path: LocalPlatformPathV1,
    metadata_index_path: LocalPlatformPathV1,
    workspace_registry_path: LocalPlatformPathV1,
    socket_path: LocalPlatformPathV1,
}

impl ServiceRuntimePathsV1 {
    pub fn from_directories(
        launch_agents_directory: impl AsRef<Path>,
        application_support_directory: impl AsRef<Path>,
        log_directory: impl AsRef<Path>,
        runtime_directory: impl AsRef<Path>,
    ) -> Result<Self, ServicePathErrorV1> {
        let launch_agents_directory = launch_agents_directory.as_ref();
        let application_support_directory = application_support_directory.as_ref();
        let log_directory = log_directory.as_ref();
        let runtime_directory = runtime_directory.as_ref();

        validate_service_path(launch_agents_directory, "launch_agents_directory")?;
        validate_service_path(
            application_support_directory,
            "application_support_directory",
        )?;
        validate_service_path(log_directory, "log_directory")?;
        validate_service_path(runtime_directory, "runtime_directory")?;

        let socket_path = runtime_directory.join("podwayd.sock");
        if socket_path.as_os_str().len() >= 104 {
            return Err(ServicePathErrorV1::SocketPathTooLong { path: socket_path });
        }

        Ok(Self {
            runtime_directory: LocalPlatformPathV1::new(runtime_directory)?,
            global_lock_path: LocalPlatformPathV1::new(runtime_directory.join("podwayd.lock"))?,
            launch_agent_path: LocalPlatformPathV1::new(
                launch_agents_directory.join(format!("{SERVICE_LABEL_V1}.plist")),
            )?,
            log_path: LocalPlatformPathV1::new(log_directory.join("podwayd.log"))?,
            metadata_index_path: LocalPlatformPathV1::new(
                application_support_directory.join("service.json"),
            )?,
            workspace_registry_path: LocalPlatformPathV1::new(
                application_support_directory.join("workspaces.json"),
            )?,
            socket_path: LocalPlatformPathV1::new(socket_path)?,
        })
    }

    pub fn for_user(
        home_directory: impl AsRef<Path>,
        _temporary_directory: impl AsRef<Path>,
        user_id: u32,
    ) -> Result<Self, ServicePathErrorV1> {
        Self::from_podway_home(&PodwayHomeV1::from_account_home(home_directory, user_id)?)
    }

    pub fn for_effective_user() -> Result<Self, ServicePathErrorV1> {
        Self::from_podway_home(&PodwayHomeV1::for_effective_user()?)
    }

    pub fn from_podway_home(home: &PodwayHomeV1) -> Result<Self, ServicePathErrorV1> {
        let runtime_directory = home.as_path().join("run");
        let state_directory = home.as_path().join("state");
        let logs_directory = home.as_path().join("logs");
        let socket_path = runtime_directory.join("podwayd.sock");
        if socket_path.as_os_str().len() >= 104 {
            return Err(ServicePathErrorV1::SocketPathTooLong { path: socket_path });
        }

        Ok(Self {
            runtime_directory: LocalPlatformPathV1::service_global(&runtime_directory)?,
            global_lock_path: LocalPlatformPathV1::service_global(
                runtime_directory.join("podwayd.lock"),
            )?,
            launch_agent_path: LocalPlatformPathV1::service_global(
                home.account_home()
                    .join("Library/LaunchAgents")
                    .join(format!("{SERVICE_LABEL_V1}.plist")),
            )?,
            log_path: LocalPlatformPathV1::service_global(logs_directory.join("podwayd.log"))?,
            metadata_index_path: LocalPlatformPathV1::service_global(
                state_directory.join("service.json"),
            )?,
            workspace_registry_path: LocalPlatformPathV1::service_global(
                state_directory.join("workspaces.json"),
            )?,
            socket_path: LocalPlatformPathV1::service_global(socket_path)?,
        })
    }

    pub fn global_lock_path(&self) -> &LocalPlatformPathV1 {
        &self.global_lock_path
    }

    pub fn launch_agent_path(&self) -> &LocalPlatformPathV1 {
        &self.launch_agent_path
    }

    pub fn log_path(&self) -> &LocalPlatformPathV1 {
        &self.log_path
    }

    pub fn metadata_index_path(&self) -> &LocalPlatformPathV1 {
        &self.metadata_index_path
    }

    pub fn runtime_directory(&self) -> &LocalPlatformPathV1 {
        &self.runtime_directory
    }

    pub fn workspace_registry_path(&self) -> &LocalPlatformPathV1 {
        &self.workspace_registry_path
    }
    pub fn socket_path(&self) -> &LocalPlatformPathV1 {
        &self.socket_path
    }

    /// Returns the same per-user service layout with an explicitly selected IPC endpoint.
    pub fn with_socket_path(
        mut self,
        socket_path: impl AsRef<Path>,
    ) -> Result<Self, ServicePathErrorV1> {
        let socket_path = socket_path.as_ref();
        if socket_path.as_os_str().len() >= 104 {
            return Err(ServicePathErrorV1::SocketPathTooLong {
                path: socket_path.to_path_buf(),
            });
        }
        self.socket_path = LocalPlatformPathV1::service_global(socket_path)?;
        Ok(self)
    }
}

/// Input used to install or update the fixed v1 LaunchAgent definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallSpecV1 {
    daemon_executable_path: LocalPlatformPathV1,
    label: ServiceLabelV1,
    runtime_paths: ServiceRuntimePathsV1,
    expected_daemon_version: Option<String>,
}

impl InstallSpecV1 {
    pub fn new(
        daemon_executable_path: LocalPlatformPathV1,
        label: ServiceLabelV1,
        runtime_paths: ServiceRuntimePathsV1,
    ) -> Self {
        Self {
            daemon_executable_path,
            label,
            runtime_paths,
            expected_daemon_version: None,
        }
    }

    pub fn with_expected_daemon_version(mut self, version: impl Into<String>) -> Self {
        self.expected_daemon_version = Some(version.into());
        self
    }

    pub fn daemon_executable_path(&self) -> &LocalPlatformPathV1 {
        &self.daemon_executable_path
    }

    pub const fn label(&self) -> ServiceLabelV1 {
        self.label
    }

    pub fn runtime_paths(&self) -> &ServiceRuntimePathsV1 {
        &self.runtime_paths
    }
}

/// The one local log stream exposed by the v1 service contract.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ServiceLogStreamV1 {
    #[default]
    DaemonV1,
}

/// Selects a bounded, named service log location and optional presentation parameters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LogQueryV1 {
    stream: ServiceLogStreamV1,
    follow: bool,
    lines: Option<u16>,
}

impl LogQueryV1 {
    pub const fn new(stream: ServiceLogStreamV1) -> Self {
        Self {
            stream,
            follow: false,
            lines: None,
        }
    }

    pub const fn with_follow(mut self, follow: bool) -> Self {
        self.follow = follow;
        self
    }

    pub const fn with_lines(mut self, lines: Option<u16>) -> Self {
        self.lines = lines;
        self
    }

    pub const fn stream(self) -> ServiceLogStreamV1 {
        self.stream
    }

    pub const fn follow(self) -> bool {
        self.follow
    }

    pub const fn lines(self) -> Option<u16> {
        self.lines
    }
}

/// Explicit uninstall cleanup behavior. Logs are preserved unless requested otherwise.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UninstallOptionsV1 {
    purge_logs: bool,
}

impl UninstallOptionsV1 {
    pub const fn new(purge_logs: bool) -> Self {
        Self { purge_logs }
    }

    pub const fn purge_logs(self) -> bool {
        self.purge_logs
    }
}

/// A bounded local platform path where the selected service log can be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogLocationV1 {
    path: LocalPlatformPathV1,
}

impl LogLocationV1 {
    pub const fn new(path: LocalPlatformPathV1) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &LocalPlatformPathV1 {
        &self.path
    }
}

/// Persisted install metadata owned by the eventual platform adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceInstallMetadataV1 {
    version: u16,
    label: String,
    daemon_binary: PathBuf,
    daemon_identity: String,
    artifact_role: ServiceArtifactRoleV1,
    installed_at: UnixMillis,
    updated_at: UnixMillis,
    publication_state: ServicePublicationStateV1,
    generation: Option<String>,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ServiceArtifactRoleV1 {
    ProductionDaemon,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServicePublicationStateV1 {
    Prepared,
    ReceiptDurable,
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum ServiceMetadataReadV1 {
    Missing,
    Present(Box<ServiceInstallMetadataV1>),
    Oversized,
}

impl ServiceInstallMetadataV1 {
    pub fn new(
        daemon_binary: impl AsRef<Path>,
        installed_at: UnixMillis,
        updated_at: UnixMillis,
    ) -> Result<Self, ServiceMetadataErrorV1> {
        if updated_at < installed_at {
            return Err(ServiceMetadataErrorV1::UpdatedBeforeInstalled {
                installed_at,
                updated_at,
            });
        }
        let daemon_binary = daemon_binary.as_ref().to_path_buf();
        validate_service_path(&daemon_binary, "daemon_binary")
            .map_err(ServiceMetadataErrorV1::InvalidDaemonBinary)?;
        Ok(Self {
            version: SERVICE_METADATA_VERSION_V1,
            label: SERVICE_LABEL_V1.to_owned(),
            daemon_binary,
            daemon_identity: String::new(),
            artifact_role: ServiceArtifactRoleV1::ProductionDaemon,
            installed_at,
            updated_at,
            publication_state: ServicePublicationStateV1::Prepared,
            generation: None,
        })
    }

    pub fn with_daemon_identity(
        mut self,
        daemon_identity: impl Into<String>,
    ) -> Result<Self, ServiceMetadataErrorV1> {
        let daemon_identity = daemon_identity.into();
        if !is_sha256_hex_v1(&daemon_identity) {
            return Err(ServiceMetadataErrorV1::InvalidDaemonBinary(
                ServicePathErrorV1::Unnormalized {
                    field: "daemon_identity",
                    path: self.daemon_binary.clone(),
                },
            ));
        }
        self.daemon_identity = daemon_identity;
        Ok(self)
    }
    pub const fn version(&self) -> u16 {
        self.version
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn daemon_binary(&self) -> &Path {
        &self.daemon_binary
    }

    pub fn daemon_identity(&self) -> &str {
        &self.daemon_identity
    }

    pub const fn installed_at(&self) -> UnixMillis {
        self.installed_at
    }

    pub const fn updated_at(&self) -> UnixMillis {
        self.updated_at
    }

    fn with_publication_state(mut self, publication_state: ServicePublicationStateV1) -> Self {
        self.publication_state = publication_state;
        self.generation = None;
        self
    }
    fn with_generation_for_plist(mut self, plist_without_generation: &[u8]) -> Self {
        self.generation = Some(publication_generation_v1(&self, plist_without_generation));
        self
    }
}

/// A direct service command constructed by [`ServiceManagerV1`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceCommandV1 {
    Install {
        requested_at: UnixMillis,
        spec: InstallSpecV1,
    },
    Update {
        requested_at: UnixMillis,
        spec: InstallSpecV1,
    },
    Start {
        requested_at: UnixMillis,
        paths: ServiceRuntimePathsV1,
    },
    Stop {
        requested_at: UnixMillis,
        paths: ServiceRuntimePathsV1,
    },
    Restart {
        requested_at: UnixMillis,
        paths: ServiceRuntimePathsV1,
    },
    Status {
        requested_at: UnixMillis,
        paths: ServiceRuntimePathsV1,
    },
    Logs {
        requested_at: UnixMillis,
        paths: ServiceRuntimePathsV1,
        query: LogQueryV1,
    },
    Uninstall {
        requested_at: UnixMillis,
        paths: ServiceRuntimePathsV1,
    },
    UninstallWithOptions {
        requested_at: UnixMillis,
        paths: ServiceRuntimePathsV1,
        options: UninstallOptionsV1,
    },
}

impl ServiceCommandV1 {
    pub const fn operation(&self) -> ServiceOperationV1 {
        match self {
            Self::Install { .. } => ServiceOperationV1::Install,
            Self::Update { .. } => ServiceOperationV1::Update,
            Self::Start { .. } => ServiceOperationV1::Start,
            Self::Stop { .. } => ServiceOperationV1::Stop,
            Self::Restart { .. } => ServiceOperationV1::Restart,
            Self::Status { .. } => ServiceOperationV1::Status,
            Self::Logs { .. } => ServiceOperationV1::Logs,
            Self::Uninstall { .. } | Self::UninstallWithOptions { .. } => {
                ServiceOperationV1::Uninstall
            }
        }
    }
}

/// The lifecycle operation associated with a command or error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceOperationV1 {
    Install,
    Update,
    Start,
    Stop,
    Restart,
    Status,
    Logs,
    Uninstall,
}

impl ServiceOperationV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Status => "status",
            Self::Logs => "logs",
            Self::Uninstall => "uninstall",
        }
    }
}

impl fmt::Display for ServiceOperationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A successful operation that changed service-owned state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceChangedV1 {
    completed_at: UnixMillis,
    metadata: Option<ServiceInstallMetadataV1>,
}

impl ServiceChangedV1 {
    pub const fn new(completed_at: UnixMillis, metadata: Option<ServiceInstallMetadataV1>) -> Self {
        Self {
            completed_at,
            metadata,
        }
    }

    pub const fn completed_at(&self) -> UnixMillis {
        self.completed_at
    }

    pub fn metadata(&self) -> Option<&ServiceInstallMetadataV1> {
        self.metadata.as_ref()
    }
}

/// A successful operation whose requested installed state already existed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceAlreadyV1 {
    observed_at: UnixMillis,
    metadata: ServiceInstallMetadataV1,
}

impl ServiceAlreadyV1 {
    pub const fn new(observed_at: UnixMillis, metadata: ServiceInstallMetadataV1) -> Self {
        Self {
            observed_at,
            metadata,
        }
    }

    pub const fn observed_at(&self) -> UnixMillis {
        self.observed_at
    }

    pub const fn metadata(&self) -> &ServiceInstallMetadataV1 {
        &self.metadata
    }
}

/// A successful operation that found no installed service definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceNotInstalledV1 {
    observed_at: UnixMillis,
}

impl ServiceNotInstalledV1 {
    pub const fn new(observed_at: UnixMillis) -> Self {
        Self { observed_at }
    }

    pub const fn observed_at(&self) -> UnixMillis {
        self.observed_at
    }
}

/// A successful operation that observed the service stopped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceStoppedV1 {
    observed_at: UnixMillis,
    metadata: Option<ServiceInstallMetadataV1>,
}

impl ServiceStoppedV1 {
    pub const fn new(observed_at: UnixMillis, metadata: Option<ServiceInstallMetadataV1>) -> Self {
        Self {
            observed_at,
            metadata,
        }
    }

    pub const fn observed_at(&self) -> UnixMillis {
        self.observed_at
    }

    pub fn metadata(&self) -> Option<&ServiceInstallMetadataV1> {
        self.metadata.as_ref()
    }
}

/// A successful operation that observed a running service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceRunningV1 {
    observed_at: UnixMillis,
    process_id: Option<u32>,
    metadata: Option<ServiceInstallMetadataV1>,
}

impl ServiceRunningV1 {
    pub const fn new(
        observed_at: UnixMillis,
        process_id: Option<u32>,
        metadata: Option<ServiceInstallMetadataV1>,
    ) -> Self {
        Self {
            observed_at,
            process_id,
            metadata,
        }
    }

    pub const fn observed_at(&self) -> UnixMillis {
        self.observed_at
    }

    pub const fn process_id(&self) -> Option<u32> {
        self.process_id
    }

    pub fn metadata(&self) -> Option<&ServiceInstallMetadataV1> {
        self.metadata.as_ref()
    }
}

/// Typed lifecycle outcomes accepted by the service contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceOutcomeV1 {
    AlreadyInDesiredStateV1(ServiceAlreadyV1),
    ChangedV1(ServiceChangedV1),
    NotInstalledV1(ServiceNotInstalledV1),
    RunningV1(ServiceRunningV1),
    StoppedV1(ServiceStoppedV1),
}

impl ServiceOutcomeV1 {
    pub const fn kind(&self) -> ServiceOutcomeKindV1 {
        match self {
            Self::AlreadyInDesiredStateV1(_) => ServiceOutcomeKindV1::AlreadyInDesiredStateV1,
            Self::ChangedV1(_) => ServiceOutcomeKindV1::ChangedV1,
            Self::NotInstalledV1(_) => ServiceOutcomeKindV1::NotInstalledV1,
            Self::RunningV1(_) => ServiceOutcomeKindV1::RunningV1,
            Self::StoppedV1(_) => ServiceOutcomeKindV1::StoppedV1,
        }
    }
}

/// Stable discriminator for defensive lifecycle-outcome validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceOutcomeKindV1 {
    AlreadyInDesiredStateV1,
    ChangedV1,
    NotInstalledV1,
    RunningV1,
    StoppedV1,
}

impl ServiceOutcomeKindV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyInDesiredStateV1 => "already_in_desired_state",
            Self::ChangedV1 => "changed",
            Self::NotInstalledV1 => "not_installed",
            Self::RunningV1 => "running",
            Self::StoppedV1 => "stopped",
        }
    }
}

/// The bounded status domain exposed by [`ServiceManagerContractV1::status`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceStatusV1 {
    NotInstalledV1(ServiceNotInstalledV1),
    RunningV1(ServiceRunningV1),
    StoppedV1(ServiceStoppedV1),
}

/// The result produced by a [`ServiceCommandRunnerV1`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceCommandResultV1 {
    LogLocation(LogLocationV1),
    Outcome(ServiceOutcomeV1),
    Status(ServiceStatusV1),
}

impl ServiceCommandResultV1 {
    pub const fn kind(&self) -> ServiceCommandResultKindV1 {
        match self {
            Self::LogLocation(_) => ServiceCommandResultKindV1::LogLocation,
            Self::Outcome(_) => ServiceCommandResultKindV1::Outcome,
            Self::Status(_) => ServiceCommandResultKindV1::Status,
        }
    }
}

/// Stable discriminator for runner result validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceCommandResultKindV1 {
    LogLocation,
    Outcome,
    Status,
}

impl ServiceCommandResultKindV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LogLocation => "log_location",
            Self::Outcome => "outcome",
            Self::Status => "status",
        }
    }
}

/// Concrete adapter for [`ServiceManagerContractV1`].
pub struct ServiceManagerV1<R, C> {
    command_runner: R,
    clock: C,
    paths: ServiceRuntimePathsV1,
}

impl<R, C> ServiceManagerV1<R, C>
where
    R: ServiceCommandRunnerV1,
    C: ServiceClockV1,
{
    pub fn new(command_runner: R, clock: C, paths: ServiceRuntimePathsV1) -> Self {
        Self {
            command_runner,
            clock,
            paths,
        }
    }

    pub fn paths(&self) -> &ServiceRuntimePathsV1 {
        &self.paths
    }

    pub fn command_runner(&self) -> &R {
        &self.command_runner
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    fn run_outcome(
        &self,
        command: ServiceCommandV1,
        accepts: fn(&ServiceOutcomeV1) -> bool,
    ) -> Result<ServiceOutcomeV1, ServiceErrorV1> {
        let operation = command.operation();
        let result = match self
            .command_runner
            .run(command)
            .map_err(|error| error.wrap_with_operation(operation))
        {
            Ok(ServiceCommandResultV1::Outcome(outcome)) if accepts(&outcome) => Ok(outcome),
            Ok(ServiceCommandResultV1::Outcome(outcome)) => Err(ServiceErrorV1::IoV1 {
                operation: Some(operation),
                message: format!(
                    "runner returned unexpected {} outcome",
                    outcome.kind().as_str()
                ),
            }),
            Ok(result) => Err(ServiceErrorV1::IoV1 {
                operation: Some(operation),
                message: format!(
                    "runner returned {} result; expected {}",
                    result.kind().as_str(),
                    ServiceCommandResultKindV1::Outcome.as_str()
                ),
            }),
            Err(error) => Err(error),
        };
        result.map_err(|error| error.wrap_with_operation(operation))
    }
    fn validate_spec_paths(
        &self,
        operation: ServiceOperationV1,
        spec: &InstallSpecV1,
    ) -> Result<(), ServiceErrorV1> {
        if spec.runtime_paths() == &self.paths {
            Ok(())
        } else {
            Err(ServiceErrorV1::IoV1 {
                operation: Some(operation),
                message: "install specification runtime paths differ from manager configuration"
                    .to_owned(),
            })
        }
    }
}

impl<R, C> ServiceManagerContractV1 for ServiceManagerV1<R, C>
where
    R: ServiceCommandRunnerV1,
    C: ServiceClockV1,
{
    fn install(&self, spec: InstallSpecV1) -> Result<ServiceOutcomeV1, ServiceErrorV1> {
        self.validate_spec_paths(ServiceOperationV1::Install, &spec)
            .map_err(|error| error.wrap_with_operation(ServiceOperationV1::Install))?;
        self.run_outcome(
            ServiceCommandV1::Install {
                requested_at: self.clock.now(),
                spec,
            },
            accepts_install,
        )
    }

    fn logs(&self, query: LogQueryV1) -> Result<LogLocationV1, ServiceErrorV1> {
        let command = ServiceCommandV1::Logs {
            requested_at: self.clock.now(),
            paths: self.paths.clone(),
            query,
        };
        let operation = command.operation();
        let result = match self
            .command_runner
            .run(command)
            .map_err(|error| error.wrap_with_operation(operation))
        {
            Ok(ServiceCommandResultV1::LogLocation(location))
                if location.path() == self.paths.log_path() =>
            {
                Ok(location)
            }
            Ok(ServiceCommandResultV1::LogLocation(_)) => Err(ServiceErrorV1::LogUnavailableV1 {
                message: "runner returned a log location outside the configured service runtime"
                    .to_owned(),
            }),
            Ok(result) => Err(ServiceErrorV1::IoV1 {
                operation: Some(operation),
                message: format!(
                    "runner returned {} result; expected {}",
                    result.kind().as_str(),
                    ServiceCommandResultKindV1::LogLocation.as_str()
                ),
            }),
            Err(error) => Err(error),
        };
        result.map_err(|error| error.wrap_with_operation(operation))
    }

    fn restart(&self) -> Result<ServiceOutcomeV1, ServiceErrorV1> {
        self.run_outcome(
            ServiceCommandV1::Restart {
                requested_at: self.clock.now(),
                paths: self.paths.clone(),
            },
            accepts_restart,
        )
    }

    fn start(&self) -> Result<ServiceOutcomeV1, ServiceErrorV1> {
        self.run_outcome(
            ServiceCommandV1::Start {
                requested_at: self.clock.now(),
                paths: self.paths.clone(),
            },
            accepts_start,
        )
    }

    fn status(&self) -> Result<ServiceStatusV1, ServiceErrorV1> {
        let command = ServiceCommandV1::Status {
            requested_at: self.clock.now(),
            paths: self.paths.clone(),
        };
        let operation = command.operation();
        let result = match self
            .command_runner
            .run(command)
            .map_err(|error| error.wrap_with_operation(operation))
        {
            Ok(ServiceCommandResultV1::Status(status)) => Ok(status),
            Ok(result) => Err(ServiceErrorV1::IoV1 {
                operation: Some(operation),
                message: format!(
                    "runner returned {} result; expected {}",
                    result.kind().as_str(),
                    ServiceCommandResultKindV1::Status.as_str()
                ),
            }),
            Err(error) => Err(error),
        };
        result.map_err(|error| error.wrap_with_operation(operation))
    }

    fn stop(&self) -> Result<ServiceOutcomeV1, ServiceErrorV1> {
        self.run_outcome(
            ServiceCommandV1::Stop {
                requested_at: self.clock.now(),
                paths: self.paths.clone(),
            },
            accepts_stop,
        )
    }

    fn uninstall(&self) -> Result<ServiceOutcomeV1, ServiceErrorV1> {
        self.run_outcome(
            ServiceCommandV1::Uninstall {
                requested_at: self.clock.now(),
                paths: self.paths.clone(),
            },
            accepts_uninstall,
        )
    }
    fn uninstall_with_options(
        &self,
        options: UninstallOptionsV1,
    ) -> Result<ServiceOutcomeV1, ServiceErrorV1> {
        self.run_outcome(
            ServiceCommandV1::UninstallWithOptions {
                requested_at: self.clock.now(),
                paths: self.paths.clone(),
                options,
            },
            accepts_uninstall,
        )
    }

    fn update(&self, spec: InstallSpecV1) -> Result<ServiceOutcomeV1, ServiceErrorV1> {
        self.validate_spec_paths(ServiceOperationV1::Update, &spec)
            .map_err(|error| error.wrap_with_operation(ServiceOperationV1::Update))?;
        self.run_outcome(
            ServiceCommandV1::Update {
                requested_at: self.clock.now(),
                spec,
            },
            accepts_update,
        )
    }
}

fn accepts_install(outcome: &ServiceOutcomeV1) -> bool {
    matches!(
        outcome,
        ServiceOutcomeV1::ChangedV1(_) | ServiceOutcomeV1::AlreadyInDesiredStateV1(_)
    )
}

fn accepts_update(outcome: &ServiceOutcomeV1) -> bool {
    matches!(
        outcome,
        ServiceOutcomeV1::ChangedV1(_)
            | ServiceOutcomeV1::AlreadyInDesiredStateV1(_)
            | ServiceOutcomeV1::NotInstalledV1(_)
    )
}

fn accepts_start(outcome: &ServiceOutcomeV1) -> bool {
    matches!(
        outcome,
        ServiceOutcomeV1::ChangedV1(_)
            | ServiceOutcomeV1::NotInstalledV1(_)
            | ServiceOutcomeV1::RunningV1(_)
    )
}

fn accepts_stop(outcome: &ServiceOutcomeV1) -> bool {
    matches!(
        outcome,
        ServiceOutcomeV1::NotInstalledV1(_) | ServiceOutcomeV1::StoppedV1(_)
    )
}

fn accepts_restart(outcome: &ServiceOutcomeV1) -> bool {
    matches!(
        outcome,
        ServiceOutcomeV1::ChangedV1(_)
            | ServiceOutcomeV1::NotInstalledV1(_)
            | ServiceOutcomeV1::RunningV1(_)
    )
}

fn accepts_uninstall(outcome: &ServiceOutcomeV1) -> bool {
    matches!(
        outcome,
        ServiceOutcomeV1::ChangedV1(_) | ServiceOutcomeV1::NotInstalledV1(_)
    )
}

/// A deterministic clock for tests and composition fixtures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedServiceClockV1 {
    now: UnixMillis,
}

impl FixedServiceClockV1 {
    pub const fn new(now: UnixMillis) -> Self {
        Self { now }
    }
}

impl ServiceClockV1 for FixedServiceClockV1 {
    fn now(&self) -> UnixMillis {
        self.now
    }
}

/// One command captured by [`RecordingServiceCommandRunnerV1`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedServiceCommandV1 {
    command: ServiceCommandV1,
}

impl RecordedServiceCommandV1 {
    pub const fn new(command: ServiceCommandV1) -> Self {
        Self { command }
    }

    pub const fn command(&self) -> &ServiceCommandV1 {
        &self.command
    }
}

/// A recording command runner that never executes a process.
///
/// Each invocation records its command and consumes one preloaded result. An invocation without a
/// preloaded result fails rather than reporting a fabricated successful operation.
#[derive(Debug, Default)]
pub struct RecordingServiceCommandRunnerV1 {
    commands: Mutex<Vec<RecordedServiceCommandV1>>,
    results: Mutex<VecDeque<Result<ServiceCommandResultV1, ServiceErrorV1>>>,
}

impl RecordingServiceCommandRunnerV1 {
    pub fn new(
        results: impl IntoIterator<Item = Result<ServiceCommandResultV1, ServiceErrorV1>>,
    ) -> Self {
        Self {
            commands: Mutex::new(Vec::new()),
            results: Mutex::new(results.into_iter().collect()),
        }
    }

    pub fn push_result(
        &self,
        result: Result<ServiceCommandResultV1, ServiceErrorV1>,
    ) -> Result<(), ServiceErrorV1> {
        self.results
            .lock()
            .map_err(|_| ServiceErrorV1::IoV1 {
                operation: None,
                message: "recording service results lock was poisoned".to_owned(),
            })?
            .push_back(result);
        Ok(())
    }

    pub fn recorded_commands(&self) -> Result<Vec<RecordedServiceCommandV1>, ServiceErrorV1> {
        Ok(self
            .commands
            .lock()
            .map_err(|_| ServiceErrorV1::IoV1 {
                operation: None,
                message: "recording service commands lock was poisoned".to_owned(),
            })?
            .clone())
    }

    pub fn pending_result_count(&self) -> Result<usize, ServiceErrorV1> {
        Ok(self
            .results
            .lock()
            .map_err(|_| ServiceErrorV1::IoV1 {
                operation: None,
                message: "recording service results lock was poisoned".to_owned(),
            })?
            .len())
    }
}

impl ServiceCommandRunnerV1 for RecordingServiceCommandRunnerV1 {
    fn run(&self, command: ServiceCommandV1) -> Result<ServiceCommandResultV1, ServiceErrorV1> {
        self.commands
            .lock()
            .map_err(|_| ServiceErrorV1::IoV1 {
                operation: Some(command.operation()),
                message: "recording service commands lock was poisoned".to_owned(),
            })?
            .push(RecordedServiceCommandV1::new(command.clone()));

        self.results
            .lock()
            .map_err(|_| ServiceErrorV1::IoV1 {
                operation: Some(command.operation()),
                message: "recording service results lock was poisoned".to_owned(),
            })?
            .pop_front()
            .unwrap_or(Err(ServiceErrorV1::IoV1 {
                operation: Some(command.operation()),
                message: "recording service command runner has no recorded result".to_owned(),
            }))
    }
}
/// The manifest-defined recording service-manager double.
pub type RecordingServiceManagerV1 =
    ServiceManagerV1<RecordingServiceCommandRunnerV1, FixedServiceClockV1>;

/// Invalid runtime or binary paths rejected before they reach an adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServicePathErrorV1 {
    Empty { field: &'static str },
    Relative { field: &'static str, path: PathBuf },
    Unnormalized { field: &'static str, path: PathBuf },
    WorkspaceLocal { field: &'static str, path: PathBuf },
    EffectiveUserLookup { user_id: u32 },
    EffectiveUserNotFound { user_id: u32 },
    RootUser,
    SocketPathTooLong { path: PathBuf },
}

impl fmt::Display for ServicePathErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { field } => write!(formatter, "{field} must not be empty"),
            Self::Relative { field, path } => {
                write!(formatter, "{field} must be absolute: {}", path.display())
            }
            Self::Unnormalized { field, path } => {
                write!(
                    formatter,
                    "{field} must not contain dot path components: {}",
                    path.display()
                )
            }
            Self::WorkspaceLocal { field, path } => write!(
                formatter,
                "{field} must not point into workspace-local Podway state: {}",
                path.display()
            ),
            Self::EffectiveUserLookup { user_id } => {
                write!(
                    formatter,
                    "could not query effective operating-system user {user_id}"
                )
            }
            Self::EffectiveUserNotFound { user_id } => {
                write!(
                    formatter,
                    "effective operating-system user {user_id} was not found"
                )
            }
            Self::RootUser => formatter.write_str("a per-user service cannot use the root user"),
            Self::SocketPathTooLong { path } => write!(
                formatter,
                "service socket path exceeds the platform limit: {}",
                path.display()
            ),
        }
    }
}

impl Error for ServicePathErrorV1 {}

/// Invalid service metadata rejected before persistence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceMetadataErrorV1 {
    InvalidDaemonBinary(ServicePathErrorV1),
    UpdatedBeforeInstalled {
        installed_at: UnixMillis,
        updated_at: UnixMillis,
    },
}

impl fmt::Display for ServiceMetadataErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDaemonBinary(error) => write!(formatter, "invalid daemon binary: {error}"),
            Self::UpdatedBeforeInstalled {
                installed_at,
                updated_at,
            } => write!(
                formatter,
                "metadata update time {updated_at} precedes install time {installed_at}"
            ),
        }
    }
}

impl Error for ServiceMetadataErrorV1 {}

/// Typed failures raised by service adapters and manager result validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceErrorV1 {
    InvalidMetadataV1 {
        message: String,
    },
    InvalidExecutableV1 {
        message: String,
    },
    IoV1 {
        operation: Option<ServiceOperationV1>,
        message: String,
    },
    LaunchctlFailureV1 {
        operation: ServiceOperationV1,
        exit_status: Option<i32>,
        message: String,
    },
    LogUnavailableV1 {
        message: String,
    },
    PathSafetyV1(ServicePathErrorV1),
    PermissionDeniedV1 {
        operation: ServiceOperationV1,
        path: PathBuf,
        message: String,
    },
    StaleOrUnexpectedProcessV1 {
        path: PathBuf,
        message: String,
    },
    TimeoutV1 {
        operation: ServiceOperationV1,
        timeout_ms: u64,
    },
    OutputLimitExceededV1 {
        limit_bytes: usize,
    },
    LaunchctlTimeoutV1 {
        timeout_ms: u64,
    },
    OperationFailureV1 {
        operation: ServiceOperationV1,
        source: Box<ServiceErrorV1>,
    },
}

impl fmt::Display for ServiceErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMetadataV1 { message } => {
                write!(formatter, "service metadata failure: {message}")
            }
            Self::InvalidExecutableV1 { message } => {
                write!(formatter, "service executable failure: {message}")
            }
            Self::IoV1 { operation, message } => match operation {
                Some(operation) => write!(formatter, "{operation} I/O failure: {message}"),
                None => write!(formatter, "service I/O failure: {message}"),
            },
            Self::LaunchctlFailureV1 {
                operation,
                exit_status,
                message,
            } => match exit_status {
                Some(exit_status) => write!(
                    formatter,
                    "launchctl {operation} failed with exit status {exit_status}: {message}"
                ),
                None => write!(formatter, "launchctl {operation} failed: {message}"),
            },
            Self::LogUnavailableV1 { message } => {
                write!(formatter, "service log failure: {message}")
            }
            Self::PathSafetyV1(error) => error.fmt(formatter),
            Self::PermissionDeniedV1 {
                operation,
                path,
                message,
            } => write!(
                formatter,
                "{operation} permission failure at {}: {message}",
                path.display()
            ),
            Self::StaleOrUnexpectedProcessV1 { path, message } => {
                write!(
                    formatter,
                    "stale runtime path {}: {message}",
                    path.display()
                )
            }
            Self::TimeoutV1 {
                operation,
                timeout_ms,
            } => write!(formatter, "{operation} timed out after {timeout_ms} ms"),
            Self::OutputLimitExceededV1 { limit_bytes } => {
                write!(formatter, "launchctl output exceeded {limit_bytes} bytes")
            }
            Self::LaunchctlTimeoutV1 { timeout_ms } => {
                write!(formatter, "launchctl timed out after {timeout_ms} ms")
            }
            Self::OperationFailureV1 { operation, source } => {
                write!(formatter, "{operation} failed: {source}")
            }
        }
    }
}

impl Error for ServiceErrorV1 {}
impl ServiceErrorV1 {
    fn with_operation(self, operation: ServiceOperationV1) -> Self {
        let already_typed = match &self {
            Self::IoV1 {
                operation: Some(error_operation),
                ..
            }
            | Self::LaunchctlFailureV1 {
                operation: error_operation,
                ..
            }
            | Self::PermissionDeniedV1 {
                operation: error_operation,
                ..
            }
            | Self::TimeoutV1 {
                operation: error_operation,
                ..
            }
            | Self::OperationFailureV1 {
                operation: error_operation,
                ..
            } => *error_operation == operation,
            _ => false,
        };
        if already_typed {
            self
        } else {
            Self::OperationFailureV1 {
                operation,
                source: Box::new(self),
            }
        }
    }

    fn wrap_with_operation(self, operation: ServiceOperationV1) -> Self {
        if matches!(
            &self,
            Self::OperationFailureV1 {
                operation: error_operation,
                ..
            } if *error_operation == operation
        ) {
            self
        } else {
            Self::OperationFailureV1 {
                operation,
                source: Box::new(self),
            }
        }
    }
}

fn validate_absolute_normalized_path(
    path: &Path,
    field: &'static str,
) -> Result<(), ServicePathErrorV1> {
    if path.as_os_str().is_empty() {
        return Err(ServicePathErrorV1::Empty { field });
    }
    if path.to_str().is_none() {
        return Err(ServicePathErrorV1::Unnormalized {
            field,
            path: path.to_path_buf(),
        });
    }
    if !path.is_absolute() {
        return Err(ServicePathErrorV1::Relative {
            field,
            path: path.to_path_buf(),
        });
    }

    for component in path.components() {
        match component {
            Component::CurDir | Component::ParentDir => {
                return Err(ServicePathErrorV1::Unnormalized {
                    field,
                    path: path.to_path_buf(),
                });
            }
            Component::Normal(component)
                if component
                    .as_encoded_bytes()
                    .iter()
                    .any(|byte| *byte < 0x20 || *byte == 0x7f) =>
            {
                return Err(ServicePathErrorV1::Unnormalized {
                    field,
                    path: path.to_path_buf(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_service_path(path: &Path, field: &'static str) -> Result<(), ServicePathErrorV1> {
    validate_absolute_normalized_path(path, field)?;
    if path
        .components()
        .any(|component| matches!(component, Component::Normal(value) if value == ".podway"))
    {
        return Err(ServicePathErrorV1::WorkspaceLocal {
            field,
            path: path.to_path_buf(),
        });
    }
    Ok(())
}
/// Filesystem boundary used by the macOS LaunchAgent adapter. Implementations must perform
/// `write_atomically` as a same-directory replace so a partially written plist is never loaded.
pub trait ServiceFilesystemV1: Send + Sync {
    fn exists(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1>;
    fn is_executable(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1>;
    fn create_directory(&self, path: &Path, mode: u32) -> Result<(), ServiceFilesystemErrorV1>;
    fn read_file_bounded(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, ServiceFilesystemErrorV1>;
    fn write_atomically(
        &self,
        path: &Path,
        contents: &[u8],
        mode: u32,
    ) -> Result<(), ServiceFilesystemErrorV1>;
    /// Removes the path entry itself without following a final symlink; a missing path succeeds.
    fn remove_file(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1>;
    fn list_directory_bounded(
        &self,
        path: &Path,
        maximum_entries: usize,
    ) -> Result<Vec<PathBuf>, ServiceFilesystemErrorV1>;
    fn remove_directory(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1>;
    fn rotate_file(
        &self,
        path: &Path,
        maximum_bytes: u64,
        retained_files: u8,
    ) -> Result<(), ServiceFilesystemErrorV1>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceFilesystemErrorKindV1 {
    PermissionDenied,
    LimitExceeded,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceFilesystemErrorV1 {
    pub permission_denied: bool,
    pub kind: ServiceFilesystemErrorKindV1,
    pub message: String,
}

impl ServiceFilesystemErrorV1 {
    pub fn permission(message: impl Into<String>) -> Self {
        Self {
            permission_denied: true,
            kind: ServiceFilesystemErrorKindV1::PermissionDenied,
            message: message.into(),
        }
    }
    pub fn limit_exceeded(message: impl Into<String>) -> Self {
        Self {
            permission_denied: false,
            kind: ServiceFilesystemErrorKindV1::LimitExceeded,
            message: message.into(),
        }
    }
    pub fn other(message: impl Into<String>) -> Self {
        Self {
            permission_denied: false,
            kind: ServiceFilesystemErrorKindV1::Other,
            message: message.into(),
        }
    }
}
/// The real filesystem adapter used by the macOS service composition.
///
/// It never follows a caller-selected temporary path: atomic writes are staged next to their
/// destination, synced, permissioned, and renamed as one filesystem operation.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdServiceFilesystemV1;

#[cfg(any(test, debug_assertions))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurabilityFailpointV1 {
    AfterTemporaryWrite,
    AfterFileSyncAndMode,
    BeforeRename,
    AfterRename,
    AfterParentDirectorySync,
}

#[cfg(any(test, debug_assertions))]
#[derive(Debug)]
struct DurabilityFailpointSelectionV1 {
    destination: PathBuf,
    write_invocation: u64,
    observed_writes: u64,
    failpoint: DurabilityFailpointV1,
}

#[cfg(any(test, debug_assertions))]
static DURABILITY_FAILPOINT_V1: Mutex<Option<DurabilityFailpointSelectionV1>> = Mutex::new(None);

impl StdServiceFilesystemV1 {
    #[cfg(any(test, debug_assertions))]
    pub fn inject_durability_failpoint_for_testing(
        destination: impl AsRef<Path>,
        write_invocation: u64,
        failpoint: DurabilityFailpointV1,
    ) {
        assert!(write_invocation > 0, "write invocation must be non-zero");
        *DURABILITY_FAILPOINT_V1
            .lock()
            .expect("durability failpoint lock") = Some(DurabilityFailpointSelectionV1 {
            destination: destination.as_ref().to_path_buf(),
            write_invocation,
            observed_writes: 0,
            failpoint,
        });
    }

    #[cfg(any(test, debug_assertions))]
    fn fail_at_durability_boundary(path: &Path, failpoint: DurabilityFailpointV1) {
        let selected = DURABILITY_FAILPOINT_V1
            .lock()
            .expect("durability failpoint lock");
        if selected.as_ref().is_some_and(|selection| {
            selection.destination == path
                && selection.observed_writes == selection.write_invocation
                && selection.failpoint == failpoint
        }) {
            std::process::exit(86);
        }
    }

    #[cfg(any(test, debug_assertions))]
    fn observe_atomic_write(path: &Path) {
        let mut selected = DURABILITY_FAILPOINT_V1
            .lock()
            .expect("durability failpoint lock");
        if let Some(selection) = selected
            .as_mut()
            .filter(|selection| selection.destination == path)
        {
            selection.observed_writes += 1;
        }
    }

    fn error(error: std::io::Error) -> ServiceFilesystemErrorV1 {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            ServiceFilesystemErrorV1::permission(error.to_string())
        } else {
            ServiceFilesystemErrorV1::other(error.to_string())
        }
    }
    fn nix_error(error: Errno) -> ServiceFilesystemErrorV1 {
        if error == Errno::EACCES || error == Errno::EPERM {
            ServiceFilesystemErrorV1::permission(error.to_string())
        } else {
            ServiceFilesystemErrorV1::other(error.to_string())
        }
    }
    fn limit_error(maximum_bytes: usize) -> ServiceFilesystemErrorV1 {
        ServiceFilesystemErrorV1::limit_exceeded(format!(
            "service file exceeds {maximum_bytes} bytes"
        ))
    }
    fn requires_owner_private_provenance(path: &Path) -> bool {
        path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name == "service.json"
                || name == format!("{SERVICE_LABEL_V1}.plist")
                || path.ancestors().skip(1).any(|parent| {
                    parent.file_name().is_some_and(|name| {
                        name.to_string_lossy() == SERVICE_STAGED_DAEMONS_DIRECTORY_V1
                    })
                })
        })
    }

    fn service_path_components(
        path: &Path,
    ) -> Result<Vec<&std::ffi::OsStr>, ServiceFilesystemErrorV1> {
        if !path.is_absolute() {
            return Err(ServiceFilesystemErrorV1::other(
                "service path must be absolute",
            ));
        }
        let mut components = Vec::new();
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(component) => components.push(component),
                _ => {
                    return Err(ServiceFilesystemErrorV1::other(
                        "service path is not normalized",
                    ));
                }
            }
        }
        if components.is_empty() {
            return Err(ServiceFilesystemErrorV1::other(
                "service path must name a directory below the root",
            ));
        }
        Ok(components)
    }
    fn open_verified_directory_optional(
        path: &Path,
        create_final_mode: Option<u32>,
    ) -> Result<Option<OwnedFd>, ServiceFilesystemErrorV1> {
        let normalized;
        let path = if let Ok(suffix) = path.strip_prefix("/var") {
            normalized = Path::new("/private/var").join(suffix);
            normalized.as_path()
        } else if let Ok(suffix) = path.strip_prefix("/tmp") {
            normalized = Path::new("/private/tmp").join(suffix);
            normalized.as_path()
        } else {
            path
        };
        let components = Self::service_path_components(path)?;
        let mut directory = open(
            "/",
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map_err(Self::nix_error)?;
        for (index, component) in components.iter().enumerate() {
            if let Some(mode) = create_final_mode {
                match mkdirat(
                    &directory,
                    *component,
                    Mode::from_bits_truncate(if index + 1 == components.len() {
                        mode
                    } else {
                        0o700
                    } as _),
                ) {
                    Ok(()) | Err(Errno::EEXIST) => {}
                    Err(error) => return Err(Self::nix_error(error)),
                }
            }
            let next = match openat(
                &directory,
                *component,
                OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
                Mode::empty(),
            ) {
                Ok(next) => next,
                Err(Errno::ENOENT) if create_final_mode.is_none() => return Ok(None),
                Err(error) => return Err(Self::nix_error(error)),
            };
            let stat = fstat(&next).map_err(Self::nix_error)?;
            if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFDIR {
                return Err(ServiceFilesystemErrorV1::other(
                    "service path component is not a directory",
                ));
            }
            directory = next;
        }
        if let Some(mode) = create_final_mode {
            fchmod(&directory, Mode::from_bits_truncate(mode as _)).map_err(Self::nix_error)?;
        }
        Ok(Some(directory))
    }

    fn open_verified_directory(
        path: &Path,
        create_final_mode: Option<u32>,
    ) -> Result<OwnedFd, ServiceFilesystemErrorV1> {
        Self::open_verified_directory_optional(path, create_final_mode)?
            .ok_or_else(|| ServiceFilesystemErrorV1::other("service directory does not exist"))
    }
    fn temporary_name_v1(
        file_name: &std::ffi::OsStr,
        timestamp_nanos: u128,
        sequence: u64,
    ) -> String {
        format!(
            ".{}.{}.{}.{}.{}.tmp",
            file_name.to_string_lossy(),
            std::process::id(),
            timestamp_nanos,
            sequence ^ timestamp_nanos as u64,
            sequence
        )
    }
    fn is_owned_temporary_name_v1(file_name: &std::ffi::OsStr, name: &str) -> bool {
        let prefix = format!(".{}.", file_name.to_string_lossy());
        let Some(remainder) = name
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix(".tmp"))
        else {
            return false;
        };
        let mut fields = remainder.split('.');
        let Some(process_id) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            return false;
        };
        let Some(timestamp_nanos) = fields.next().and_then(|value| value.parse::<u128>().ok())
        else {
            return false;
        };
        let Some(nonce) = fields.next().and_then(|value| value.parse::<u64>().ok()) else {
            return false;
        };
        let Some(sequence) = fields.next().and_then(|value| value.parse::<u64>().ok()) else {
            return false;
        };
        let _ = process_id;
        nonce == sequence ^ timestamp_nanos as u64 && fields.next().is_none()
    }
    fn is_owned_staged_temporary_name_v1(name: &str) -> bool {
        let Some(remainder) = name
            .strip_prefix('.')
            .and_then(|value| value.strip_suffix(".tmp"))
        else {
            return false;
        };
        let mut fields = remainder.split('.');
        let Some(identity) = fields.next() else {
            return false;
        };
        if identity.len() != 64
            || !identity
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return false;
        }
        let Some(process_id) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            return false;
        };
        let Some(timestamp_nanos) = fields.next().and_then(|value| value.parse::<u128>().ok())
        else {
            return false;
        };
        let Some(nonce) = fields.next().and_then(|value| value.parse::<u64>().ok()) else {
            return false;
        };
        let Some(sequence) = fields.next().and_then(|value| value.parse::<u64>().ok()) else {
            return false;
        };
        let _ = process_id;
        nonce == sequence ^ timestamp_nanos as u64 && fields.next().is_none()
    }
    fn reclaim_stale_temporaries(
        parent: &OwnedFd,
        file_name: &std::ffi::OsStr,
    ) -> Result<(), ServiceFilesystemErrorV1> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let mut owned = Vec::new();
        let mut directory = Dir::openat(
            parent,
            ".",
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map_err(Self::nix_error)?;
        for (index, entry) in directory.iter().enumerate() {
            if index >= SERVICE_TEMPORARY_SCAN_LIMIT_V1 {
                return Err(ServiceFilesystemErrorV1::other(
                    "service temporary directory exceeds the bounded scan limit",
                ));
            }
            let entry = entry.map_err(Self::nix_error)?;
            let name = std::ffi::OsStr::from_bytes(entry.file_name().to_bytes());
            let display_name = name.to_string_lossy();
            if !Self::is_owned_temporary_name_v1(file_name, &display_name) {
                continue;
            }
            let stat =
                fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(Self::nix_error)?;
            if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
                continue;
            }
            owned.push((stat.st_mtime, name.to_os_string()));
        }
        owned.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

        let mut retained = Vec::new();
        for (modified, name) in owned {
            if now.saturating_sub(modified) >= SERVICE_TEMPORARY_STALE_AGE_V1.as_secs() as i64 {
                unlinkat(parent, name.as_os_str(), UnlinkatFlags::NoRemoveDir)
                    .map_err(Self::nix_error)?;
            } else {
                retained.push((modified, name));
            }
        }
        if retained.len() >= SERVICE_TEMPORARY_RETAIN_LIMIT_V1 {
            let remove_count = retained.len() - SERVICE_TEMPORARY_RETAIN_TARGET_V1;
            for (_, name) in retained.into_iter().take(remove_count) {
                unlinkat(parent, name.as_os_str(), UnlinkatFlags::NoRemoveDir)
                    .map_err(Self::nix_error)?;
            }
        }
        Ok(())
    }
}

impl ServiceFilesystemV1 for StdServiceFilesystemV1 {
    fn exists(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
        let parent = path.parent().ok_or_else(|| {
            ServiceFilesystemErrorV1::other("service path has no parent directory")
        })?;
        let name = path
            .file_name()
            .ok_or_else(|| ServiceFilesystemErrorV1::other("service path has no file name"))?;
        let Some(parent) = Self::open_verified_directory_optional(parent, None)? else {
            return Ok(false);
        };
        match fstatat(&parent, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(stat) => {
                Ok(SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFLNK)
            }
            Err(Errno::ENOENT) => Ok(false),
            Err(error) => Err(Self::nix_error(error)),
        }
    }

    fn is_executable(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
        let parent = path.parent().ok_or_else(|| {
            ServiceFilesystemErrorV1::other("service executable has no parent directory")
        })?;
        let name = path.file_name().ok_or_else(|| {
            ServiceFilesystemErrorV1::other("service executable has no file name")
        })?;
        let Some(parent) = Self::open_verified_directory_optional(parent, None)? else {
            return Ok(false);
        };
        let descriptor = match openat(
            &parent,
            name,
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(descriptor) => descriptor,
            Err(Errno::ENOENT) => return Ok(false),
            Err(error) => return Err(Self::nix_error(error)),
        };
        let stat = fstat(&descriptor).map_err(Self::nix_error)?;
        Ok(
            SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG
                && stat.st_mode & 0o111 != 0,
        )
    }

    fn create_directory(&self, path: &Path, mode: u32) -> Result<(), ServiceFilesystemErrorV1> {
        Self::open_verified_directory(path, Some(mode)).map(drop)
    }

    fn read_file_bounded(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, ServiceFilesystemErrorV1> {
        let parent = path.parent().ok_or_else(|| {
            ServiceFilesystemErrorV1::other("service file has no parent directory")
        })?;
        let file_name = path
            .file_name()
            .ok_or_else(|| ServiceFilesystemErrorV1::other("service file has no file name"))?;
        let parent = Self::open_verified_directory(parent, None)?;
        let descriptor = openat(
            &parent,
            file_name,
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(Self::nix_error)?;
        let stat = fstat(&descriptor).map_err(Self::nix_error)?;
        if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
            return Err(ServiceFilesystemErrorV1::other(
                "service file is not a regular file",
            ));
        }
        if Self::requires_owner_private_provenance(path)
            && (stat.st_uid != geteuid().as_raw() || stat.st_mode & 0o077 != 0)
        {
            return Err(ServiceFilesystemErrorV1::permission(
                "service file is not owner-private",
            ));
        }
        if stat.st_size > maximum_bytes as i64 {
            return Err(Self::limit_error(maximum_bytes));
        }
        let mut file = fs::File::from(descriptor);
        let mut contents = Vec::with_capacity(stat.st_size as usize);
        Read::by_ref(&mut file)
            .take(maximum_bytes as u64 + 1)
            .read_to_end(&mut contents)
            .map_err(Self::error)?;
        if contents.len() > maximum_bytes {
            return Err(Self::limit_error(maximum_bytes));
        }
        Ok(contents)
    }

    fn write_atomically(
        &self,
        path: &Path,
        contents: &[u8],
        mode: u32,
    ) -> Result<(), ServiceFilesystemErrorV1> {
        #[cfg(any(test, debug_assertions))]
        Self::observe_atomic_write(path);
        let parent = path.parent().ok_or_else(|| {
            ServiceFilesystemErrorV1::other("service file has no parent directory")
        })?;
        let file_name = path
            .file_name()
            .ok_or_else(|| ServiceFilesystemErrorV1::other("service file has no file name"))?;
        let parent = Self::open_verified_directory(parent, None)?;
        Self::reclaim_stale_temporaries(&parent, file_name)?;
        loop {
            let sequence = SERVICE_TEMPORARY_SEQUENCE_V1.fetch_add(1, Ordering::Relaxed);
            let timestamp_nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let temporary = Self::temporary_name_v1(file_name, timestamp_nanos, sequence);
            match openat(
                &parent,
                temporary.as_str(),
                OFlag::O_CLOEXEC
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_WRONLY,
                Mode::from_bits_truncate(mode as _),
            ) {
                Ok(descriptor) => {
                    let mut file = fs::File::from(descriptor);
                    let result = (|| {
                        file.write_all(contents).map_err(Self::error)?;
                        #[cfg(any(test, debug_assertions))]
                        Self::fail_at_durability_boundary(
                            path,
                            DurabilityFailpointV1::AfterTemporaryWrite,
                        );
                        file.sync_all().map_err(Self::error)?;
                        fchmod(&file, Mode::from_bits_truncate(mode as _))
                            .map_err(Self::nix_error)?;
                        file.sync_all().map_err(Self::error)?;
                        #[cfg(any(test, debug_assertions))]
                        Self::fail_at_durability_boundary(
                            path,
                            DurabilityFailpointV1::AfterFileSyncAndMode,
                        );
                        #[cfg(any(test, debug_assertions))]
                        Self::fail_at_durability_boundary(
                            path,
                            DurabilityFailpointV1::BeforeRename,
                        );
                        renameat(&parent, temporary.as_str(), &parent, file_name)
                            .map_err(Self::nix_error)?;
                        #[cfg(any(test, debug_assertions))]
                        Self::fail_at_durability_boundary(path, DurabilityFailpointV1::AfterRename);
                        fsync(&parent).map_err(Self::nix_error)?;
                        #[cfg(any(test, debug_assertions))]
                        Self::fail_at_durability_boundary(
                            path,
                            DurabilityFailpointV1::AfterParentDirectorySync,
                        );
                        Ok(())
                    })();
                    if result.is_err() {
                        let _ = unlinkat(&parent, temporary.as_str(), UnlinkatFlags::NoRemoveDir);
                    }
                    return result;
                }
                Err(Errno::EEXIST) => continue,
                Err(error) => return Err(Self::nix_error(error)),
            }
        }
    }

    fn remove_file(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        let parent = path.parent().ok_or_else(|| {
            ServiceFilesystemErrorV1::other("service file has no parent directory")
        })?;
        let name = path
            .file_name()
            .ok_or_else(|| ServiceFilesystemErrorV1::other("service file has no file name"))?;
        let parent = Self::open_verified_directory(parent, None)?;
        match unlinkat(&parent, name, UnlinkatFlags::NoRemoveDir) {
            Ok(()) | Err(Errno::ENOENT) => Ok(()),
            Err(error) => Err(Self::nix_error(error)),
        }
    }
    fn list_directory_bounded(
        &self,
        path: &Path,
        maximum_entries: usize,
    ) -> Result<Vec<PathBuf>, ServiceFilesystemErrorV1> {
        let directory_fd = Self::open_verified_directory(path, None)?;
        let mut directory = Dir::openat(
            &directory_fd,
            ".",
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map_err(Self::nix_error)?;
        let mut entries = Vec::new();
        for entry in directory.iter() {
            let entry = entry.map_err(Self::nix_error)?;
            let name = std::ffi::OsStr::from_bytes(entry.file_name().to_bytes());
            if name.as_bytes() == b"." || name.as_bytes() == b".." {
                continue;
            }
            if entries.len() >= maximum_entries {
                return Err(ServiceFilesystemErrorV1::limit_exceeded(
                    "service directory exceeds its bounded entry limit",
                ));
            }
            let metadata = fstatat(&directory_fd, name, AtFlags::AT_SYMLINK_NOFOLLOW)
                .map_err(Self::nix_error)?;
            if SFlag::from_bits_truncate(metadata.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
                return Err(ServiceFilesystemErrorV1::other(
                    "service directory contains a non-regular entry",
                ));
            }
            entries.push(path.join(name));
        }
        entries.sort();
        Ok(entries)
    }

    fn remove_directory(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        let parent = path.parent().ok_or_else(|| {
            ServiceFilesystemErrorV1::other("service directory has no parent directory")
        })?;
        let name = path
            .file_name()
            .ok_or_else(|| ServiceFilesystemErrorV1::other("service directory has no file name"))?;
        let parent = Self::open_verified_directory(parent, None)?;
        match unlinkat(&parent, name, UnlinkatFlags::RemoveDir) {
            Ok(()) | Err(Errno::ENOENT) => Ok(()),
            Err(error) => Err(Self::nix_error(error)),
        }
    }
    fn rotate_file(
        &self,
        path: &Path,
        maximum_bytes: u64,
        retained_files: u8,
    ) -> Result<(), ServiceFilesystemErrorV1> {
        let parent_path = path.parent().ok_or_else(|| {
            ServiceFilesystemErrorV1::other("service file has no parent directory")
        })?;
        let name = path
            .file_name()
            .ok_or_else(|| ServiceFilesystemErrorV1::other("service file has no file name"))?;
        let parent = Self::open_verified_directory(parent_path, None)?;
        let metadata = match fstatat(&parent, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(metadata)
                if SFlag::from_bits_truncate(metadata.st_mode) & SFlag::S_IFMT
                    == SFlag::S_IFREG =>
            {
                metadata
            }
            Ok(_) => {
                return Err(ServiceFilesystemErrorV1::other(
                    "service log is not a regular file",
                ));
            }
            Err(Errno::ENOENT) => return Ok(()),
            Err(error) => return Err(Self::nix_error(error)),
        };
        if metadata.st_size <= maximum_bytes as i64 {
            return Ok(());
        }
        if retained_files == 0 {
            unlinkat(&parent, name, UnlinkatFlags::NoRemoveDir).map_err(Self::nix_error)?;
            return Ok(());
        }
        let rotated_name = |index: u8| {
            let mut value = name.to_os_string();
            value.push(format!(".{index}"));
            value
        };
        let oldest = rotated_name(retained_files);
        match unlinkat(&parent, oldest.as_os_str(), UnlinkatFlags::NoRemoveDir) {
            Ok(()) | Err(Errno::ENOENT) => {}
            Err(error) => return Err(Self::nix_error(error)),
        }
        for index in (1..retained_files).rev() {
            let source = rotated_name(index);
            let destination = rotated_name(index + 1);
            match renameat(
                &parent,
                source.as_os_str(),
                &parent,
                destination.as_os_str(),
            ) {
                Ok(()) | Err(Errno::ENOENT) => {}
                Err(error) => return Err(Self::nix_error(error)),
            }
        }
        let first = rotated_name(1);
        renameat(&parent, name, &parent, first.as_os_str()).map_err(Self::nix_error)?;
        let file = openat(
            &parent,
            name,
            OFlag::O_CLOEXEC | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_WRONLY,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(Self::nix_error)?;
        fchmod(&file, Mode::from_bits_truncate(0o600)).map_err(Self::nix_error)?;
        fsync(&file).map_err(Self::nix_error)?;
        fsync(&parent).map_err(Self::nix_error)
    }
}

/// Injectable launchctl boundary. Arguments exclude the `launchctl` executable itself.
pub trait LaunchctlRunnerV1: Send + Sync {
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchctlOutputV1 {
    pub exit_status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl LaunchctlOutputV1 {
    pub fn success() -> Self {
        Self {
            exit_status: 0,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
    /// The documented `launchctl bootstrap` duplicate-load response.
    ///
    /// A duplicate bootstrap is not a successful side effect. Reconciliation accepts only this
    /// exact typed process result, because it proves launchd already owns the requested label.
    pub fn already_loaded_bootstrap(&self) -> bool {
        self.exit_status == 5
            && self.stdout.is_empty()
            && self.stderr == "Bootstrap failed: 5: Input/output error"
    }
}
const LAUNCHCTL_TIMEOUT_V1: Duration = Duration::from_secs(10);
const LAUNCHCTL_OUTPUT_LIMIT_V1: usize = 1024 * 1024;
const LAUNCHCTL_POST_KILL_DRAIN_V1: Duration = Duration::from_millis(250);

/// The process-backed `launchctl` adapter used by CLI composition on macOS.
///
/// Each invocation owns a dedicated process group and drains both output streams from one bounded
/// polling loop. Timeout or overflow handling terminates that group and performs a bounded pipe
/// drain; deliberately escaped process groups or sessions are outside this containment contract.
#[derive(Clone, Debug)]
pub struct SystemLaunchctlRunnerV1 {
    executable: PathBuf,
    timeout: Duration,
    output_limit: usize,
    post_kill_drain: Duration,
}

impl Default for SystemLaunchctlRunnerV1 {
    fn default() -> Self {
        Self::new("/bin/launchctl")
    }
}

impl SystemLaunchctlRunnerV1 {
    pub fn new(executable: impl AsRef<Path>) -> Self {
        Self {
            executable: executable.as_ref().to_path_buf(),
            timeout: LAUNCHCTL_TIMEOUT_V1,
            output_limit: LAUNCHCTL_OUTPUT_LIMIT_V1,
            post_kill_drain: LAUNCHCTL_POST_KILL_DRAIN_V1,
        }
    }

    /// Overrides bounded execution limits for deterministic adapter tests.
    pub fn with_bounds(
        mut self,
        timeout: Duration,
        output_limit: usize,
        post_kill_drain: Duration,
    ) -> Self {
        self.timeout = timeout;
        self.output_limit = output_limit;
        self.post_kill_drain = post_kill_drain;
        self
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }
}

fn launchctl_io_error_v1(message: impl Into<String>) -> ServiceErrorV1 {
    ServiceErrorV1::IoV1 {
        operation: None,
        message: message.into(),
    }
}

fn wait_for_launchctl_child_v1(
    child: &mut Child,
    deadline: Instant,
) -> Result<Option<ExitStatus>, ServiceErrorV1> {
    loop {
        match child.try_wait().map_err(|error| {
            launchctl_io_error_v1(format!("could not observe launchctl: {error}"))
        })? {
            Some(status) => return Ok(Some(status)),
            None if Instant::now() >= deadline => return Ok(None),
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn launchctl_process_group_v1(process_id: u32) -> Result<Pid, ServiceErrorV1> {
    let process_id = i32::try_from(process_id).map_err(|_| {
        launchctl_io_error_v1("launchctl process identifier exceeds process-group bounds")
    })?;
    Ok(Pid::from_raw(-process_id))
}

fn signal_launchctl_group_v1(process_id: u32, signal: Signal) -> Result<bool, ServiceErrorV1> {
    let process_group = launchctl_process_group_v1(process_id)?;
    match kill(process_group, signal) {
        Ok(()) => Ok(true),
        Err(nix::errno::Errno::ESRCH) => Ok(false),
        Err(error) => Err(launchctl_io_error_v1(format!(
            "could not signal launchctl process group with {signal:?}: {error}"
        ))),
    }
}

fn wait_for_launchctl_group_absence_v1(
    process_id: u32,
    grace: Duration,
) -> Result<(), ServiceErrorV1> {
    let process_group = launchctl_process_group_v1(process_id)?;
    let deadline = Instant::now() + grace;
    loop {
        match kill(process_group, None) {
            Err(nix::errno::Errno::ESRCH) => return Ok(()),
            Ok(()) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(()) => {
                return Err(launchctl_io_error_v1(
                    "launchctl process group remained after bounded termination",
                ));
            }
            Err(error) => {
                return Err(launchctl_io_error_v1(format!(
                    "could not confirm launchctl process-group absence: {error}"
                )));
            }
        }
    }
}

fn terminate_launchctl_group_v1(
    child: &mut Child,
    grace: Duration,
) -> Result<ExitStatus, ServiceErrorV1> {
    let process_id = child.id();
    let mut cleanup_errors = Vec::new();

    if let Err(error) = signal_launchctl_group_v1(process_id, Signal::SIGTERM) {
        cleanup_errors.push(error.to_string());
    }

    match wait_for_launchctl_child_v1(child, Instant::now() + grace) {
        Ok(Some(_)) | Ok(None) => {}
        Err(error) => cleanup_errors.push(error.to_string()),
    }

    if let Err(error) = signal_launchctl_group_v1(process_id, Signal::SIGKILL) {
        cleanup_errors.push(error.to_string());
    }

    let status = match wait_for_launchctl_child_v1(child, Instant::now() + grace) {
        Ok(Some(status)) => status,
        Ok(None) => {
            match child.kill() {
                Err(error) if error.kind() != ErrorKind::InvalidInput => {
                    cleanup_errors
                        .push(format!("could not directly kill launchctl child: {error}"));
                }
                _ => {}
            }
            match wait_for_launchctl_child_v1(child, Instant::now() + grace) {
                Ok(Some(status)) => status,
                Ok(None) => {
                    cleanup_errors
                        .push("launchctl child remained after bounded cleanup".to_owned());
                    return Err(launchctl_io_error_v1(format!(
                        "launchctl cleanup failed without confirmed child reaping: {}",
                        cleanup_errors.join("; ")
                    )));
                }
                Err(error) => {
                    cleanup_errors.push(error.to_string());
                    return Err(launchctl_io_error_v1(format!(
                        "launchctl cleanup failed without confirmed child reaping: {}",
                        cleanup_errors.join("; ")
                    )));
                }
            }
        }
        Err(error) => {
            cleanup_errors.push(error.to_string());
            return Err(launchctl_io_error_v1(format!(
                "launchctl cleanup failed without confirmed child reaping: {}",
                cleanup_errors.join("; ")
            )));
        }
    };
    if let Err(error) = wait_for_launchctl_group_absence_v1(process_id, grace) {
        cleanup_errors.push(error.to_string());
    }

    if cleanup_errors.is_empty() {
        Ok(status)
    } else {
        Err(launchctl_io_error_v1(format!(
            "launchctl cleanup failed after reaping child: {}",
            cleanup_errors.join("; ")
        )))
    }
}

fn drain_launchctl_stream_v1(
    stream: &mut UnixStream,
    captured: &mut Vec<u8>,
    aggregate_bytes: &mut usize,
    output_limit: usize,
) -> Result<(bool, bool), ServiceErrorV1> {
    let mut buffer = [0_u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return Ok((true, false)),
            Ok(read) => {
                let per_stream_remaining = output_limit.saturating_sub(captured.len());
                let aggregate_remaining = output_limit.saturating_sub(*aggregate_bytes);
                if read > per_stream_remaining || read > aggregate_remaining {
                    return Ok((false, true));
                }
                captured.extend_from_slice(&buffer[..read]);
                *aggregate_bytes += read;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok((false, false)),
            Err(error) => {
                return Err(launchctl_io_error_v1(format!(
                    "could not read launchctl output: {error}"
                )));
            }
        }
    }
}
fn final_drain_launchctl_streams_v1(
    stdout: &mut UnixStream,
    stderr: &mut UnixStream,
    limit: Duration,
) -> Result<(), ServiceErrorV1> {
    let deadline = Instant::now() + limit;
    let mut buffer = [0_u8; 8192];
    let mut stdout_closed = false;
    let mut stderr_closed = false;
    while (!stdout_closed || !stderr_closed) && Instant::now() < deadline {
        for (stream, closed) in [
            (&mut *stdout, &mut stdout_closed),
            (&mut *stderr, &mut stderr_closed),
        ] {
            if *closed {
                continue;
            }
            match stream.read(&mut buffer) {
                Ok(0) => *closed = true,
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => {
                    return Err(launchctl_io_error_v1(format!(
                        "could not drain launchctl output after termination: {error}"
                    )));
                }
            }
        }
        if !stdout_closed || !stderr_closed {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    if stdout_closed && stderr_closed {
        Ok(())
    } else {
        Err(launchctl_io_error_v1(
            "launchctl descendants retained an output pipe after termination",
        ))
    }
}

fn terminate_and_drain_launchctl_v1(
    child: &mut Child,
    stdout: &mut UnixStream,
    stderr: &mut UnixStream,
    grace: Duration,
) -> Result<(), ServiceErrorV1> {
    let termination = terminate_launchctl_group_v1(child, grace).map(|_| ());
    let drain = final_drain_launchctl_streams_v1(stdout, stderr, grace);
    match (termination, drain) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(termination), Err(drain)) => Err(launchctl_io_error_v1(format!(
            "{termination}; launchctl drain also failed: {drain}"
        ))),
    }
}

fn launchctl_error_after_cleanup_v1(
    primary: ServiceErrorV1,
    cleanup: Result<(), ServiceErrorV1>,
) -> ServiceErrorV1 {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup_error) => launchctl_io_error_v1(format!(
            "{primary}; launchctl cleanup also failed: {cleanup_error}"
        )),
    }
}

fn terminate_exited_launchctl_group_and_drain_v1(
    child: &Child,
    stdout: &mut UnixStream,
    stderr: &mut UnixStream,
    grace: Duration,
) -> Result<(), ServiceErrorV1> {
    let termination = signal_launchctl_group_v1(child.id(), Signal::SIGKILL)
        .and_then(|_| wait_for_launchctl_group_absence_v1(child.id(), grace));
    let drain = final_drain_launchctl_streams_v1(stdout, stderr, grace);
    match (termination, drain) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(termination), Err(drain)) => Err(launchctl_io_error_v1(format!(
            "{termination}; launchctl drain also failed: {drain}"
        ))),
    }
}
impl LaunchctlRunnerV1 for SystemLaunchctlRunnerV1 {
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        let (mut stdout, child_stdout) = UnixStream::pair().map_err(|error| {
            launchctl_io_error_v1(format!("could not create launchctl stdout pipe: {error}"))
        })?;
        let (mut stderr, child_stderr) = UnixStream::pair().map_err(|error| {
            launchctl_io_error_v1(format!("could not create launchctl stderr pipe: {error}"))
        })?;
        stdout.set_nonblocking(true).map_err(|error| {
            launchctl_io_error_v1(format!(
                "could not configure launchctl stdout pipe: {error}"
            ))
        })?;
        stderr.set_nonblocking(true).map_err(|error| {
            launchctl_io_error_v1(format!(
                "could not configure launchctl stderr pipe: {error}"
            ))
        })?;

        let mut command = Command::new(&self.executable);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::from(OwnedFd::from(child_stdout)))
            .stderr(Stdio::from(OwnedFd::from(child_stderr)));
        command.process_group(0);
        let mut child = command.spawn().map_err(|error| {
            launchctl_io_error_v1(format!(
                "could not execute {}: {error}",
                self.executable.display()
            ))
        })?;
        drop(command);

        let deadline = Instant::now() + self.timeout;
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let mut aggregate_bytes = 0_usize;
        let mut stdout_closed = false;
        let mut stderr_closed = false;
        let status = loop {
            if !stdout_closed {
                let (closed, overflowed) = match drain_launchctl_stream_v1(
                    &mut stdout,
                    &mut stdout_bytes,
                    &mut aggregate_bytes,
                    self.output_limit,
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        return Err(launchctl_error_after_cleanup_v1(
                            error,
                            terminate_and_drain_launchctl_v1(
                                &mut child,
                                &mut stdout,
                                &mut stderr,
                                self.post_kill_drain,
                            ),
                        ));
                    }
                };
                stdout_closed = closed;
                if overflowed {
                    return Err(launchctl_error_after_cleanup_v1(
                        ServiceErrorV1::OutputLimitExceededV1 {
                            limit_bytes: self.output_limit,
                        },
                        terminate_and_drain_launchctl_v1(
                            &mut child,
                            &mut stdout,
                            &mut stderr,
                            self.post_kill_drain,
                        ),
                    ));
                }
            }
            if !stderr_closed {
                let (closed, overflowed) = match drain_launchctl_stream_v1(
                    &mut stderr,
                    &mut stderr_bytes,
                    &mut aggregate_bytes,
                    self.output_limit,
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        return Err(launchctl_error_after_cleanup_v1(
                            error,
                            terminate_and_drain_launchctl_v1(
                                &mut child,
                                &mut stdout,
                                &mut stderr,
                                self.post_kill_drain,
                            ),
                        ));
                    }
                };
                stderr_closed = closed;
                if overflowed {
                    return Err(launchctl_error_after_cleanup_v1(
                        ServiceErrorV1::OutputLimitExceededV1 {
                            limit_bytes: self.output_limit,
                        },
                        terminate_and_drain_launchctl_v1(
                            &mut child,
                            &mut stdout,
                            &mut stderr,
                            self.post_kill_drain,
                        ),
                    ));
                }
            }

            match child.try_wait() {
                Err(error) => {
                    let primary = launchctl_io_error_v1(format!(
                        "could not observe {}: {error}",
                        self.executable.display()
                    ));
                    return Err(launchctl_error_after_cleanup_v1(
                        primary,
                        terminate_and_drain_launchctl_v1(
                            &mut child,
                            &mut stdout,
                            &mut stderr,
                            self.post_kill_drain,
                        ),
                    ));
                }
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    return Err(launchctl_error_after_cleanup_v1(
                        ServiceErrorV1::LaunchctlTimeoutV1 {
                            timeout_ms: self.timeout.as_millis().try_into().unwrap_or(u64::MAX),
                        },
                        terminate_and_drain_launchctl_v1(
                            &mut child,
                            &mut stdout,
                            &mut stderr,
                            self.post_kill_drain,
                        ),
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            }
        };

        let drain_deadline = Instant::now() + self.post_kill_drain;
        while !stdout_closed || !stderr_closed {
            if !stdout_closed {
                let (closed, overflowed) = match drain_launchctl_stream_v1(
                    &mut stdout,
                    &mut stdout_bytes,
                    &mut aggregate_bytes,
                    self.output_limit,
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        return Err(launchctl_error_after_cleanup_v1(
                            error,
                            terminate_exited_launchctl_group_and_drain_v1(
                                &child,
                                &mut stdout,
                                &mut stderr,
                                self.post_kill_drain,
                            ),
                        ));
                    }
                };
                stdout_closed = closed;
                if overflowed {
                    return Err(launchctl_error_after_cleanup_v1(
                        ServiceErrorV1::OutputLimitExceededV1 {
                            limit_bytes: self.output_limit,
                        },
                        terminate_exited_launchctl_group_and_drain_v1(
                            &child,
                            &mut stdout,
                            &mut stderr,
                            self.post_kill_drain,
                        ),
                    ));
                }
            }
            if !stderr_closed {
                let (closed, overflowed) = match drain_launchctl_stream_v1(
                    &mut stderr,
                    &mut stderr_bytes,
                    &mut aggregate_bytes,
                    self.output_limit,
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        return Err(launchctl_error_after_cleanup_v1(
                            error,
                            terminate_exited_launchctl_group_and_drain_v1(
                                &child,
                                &mut stdout,
                                &mut stderr,
                                self.post_kill_drain,
                            ),
                        ));
                    }
                };
                stderr_closed = closed;
                if overflowed {
                    return Err(launchctl_error_after_cleanup_v1(
                        ServiceErrorV1::OutputLimitExceededV1 {
                            limit_bytes: self.output_limit,
                        },
                        terminate_exited_launchctl_group_and_drain_v1(
                            &child,
                            &mut stdout,
                            &mut stderr,
                            self.post_kill_drain,
                        ),
                    ));
                }
            }
            if (!stdout_closed || !stderr_closed) && Instant::now() >= drain_deadline {
                return Err(launchctl_error_after_cleanup_v1(
                    launchctl_io_error_v1(
                        "launchctl descendants retained an output pipe after exit",
                    ),
                    terminate_exited_launchctl_group_and_drain_v1(
                        &child,
                        &mut stdout,
                        &mut stderr,
                        self.post_kill_drain,
                    ),
                ));
            }
            if !stdout_closed || !stderr_closed {
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        let surviving_group = signal_launchctl_group_v1(child.id(), Signal::SIGKILL)?;
        wait_for_launchctl_group_absence_v1(child.id(), self.post_kill_drain)?;
        if surviving_group {
            return Err(launchctl_io_error_v1(
                "launchctl descendants outlived a completed child process",
            ));
        }
        Ok(LaunchctlOutputV1 {
            exit_status: status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        })
    }
}

/// Production LaunchAgent command runner. Lifecycle serialization locks the verified root-owned
/// sticky `/private/var/tmp` directory itself, avoiding both environment-selected lock domains and
/// predictable user-creatable lock entries. Every path component is opened descriptor-relatively
/// without following symlinks.
struct ServiceLifecycleTransactionLockV1 {
    _file: Flock<fs::File>,
}

impl ServiceLifecycleTransactionLockV1 {
    fn io_error(operation: ServiceOperationV1, message: impl Into<String>) -> ServiceErrorV1 {
        ServiceErrorV1::IoV1 {
            operation: Some(operation),
            message: message.into(),
        }
    }

    fn open_trusted_directory(operation: ServiceOperationV1) -> Result<fs::File, ServiceErrorV1> {
        let directory =
            StdServiceFilesystemV1::open_verified_directory(Path::new("/private/var/tmp"), None)
                .map(fs::File::from)
                .map_err(|error| {
                    Self::io_error(
                        operation,
                        format!(
                            "cannot open service lifecycle lock directory: {}",
                            error.message
                        ),
                    )
                })?;
        let metadata = directory.metadata().map_err(|error| {
            Self::io_error(
                operation,
                format!("cannot inspect service lifecycle lock directory: {error}"),
            )
        })?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o1000 == 0 {
            return Err(Self::io_error(
                operation,
                "service lifecycle lock directory is not a root-owned sticky directory",
            ));
        }
        Ok(directory)
    }

    fn acquire(operation: ServiceOperationV1) -> Result<Self, ServiceErrorV1> {
        let effective_user_id = geteuid().as_raw();
        if effective_user_id == 0 {
            return Err(Self::io_error(
                operation,
                "service lifecycle lock cannot be acquired as root",
            ));
        }
        let directory = Self::open_trusted_directory(operation)?;
        let file = directory;
        let deadline = Instant::now() + SERVICE_LIFECYCLE_LOCK_TIMEOUT_V1;
        let mut file = file;
        loop {
            match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Ok(lock) => return Ok(Self { _file: lock }),
                Err((returned_file, nix::errno::Errno::EWOULDBLOCK))
                    if Instant::now() < deadline =>
                {
                    file = returned_file;
                    std::thread::sleep(SERVICE_LIFECYCLE_LOCK_RETRY_V1);
                }
                Err((_, nix::errno::Errno::EWOULDBLOCK)) => {
                    return Err(ServiceErrorV1::TimeoutV1 {
                        operation,
                        timeout_ms: SERVICE_LIFECYCLE_LOCK_TIMEOUT_V1.as_millis() as u64,
                    });
                }
                Err((_, error)) => {
                    return Err(Self::io_error(
                        operation,
                        format!("cannot acquire service lifecycle lock: {error}"),
                    ));
                }
            }
        }
    }
}

/// It owns only the fixed per-user service files supplied by [`ServiceRuntimePathsV1`]; no workspace path is accepted or traversed.
pub struct MacosServiceCommandRunnerV1<F, L, C> {
    filesystem: F,
    launchctl: L,
    clock: C,
    observer: Arc<dyn ServiceObserverV1>,
    user_id: u32,
}

impl<F, L, C> MacosServiceCommandRunnerV1<F, L, C>
where
    F: ServiceFilesystemV1,
    L: LaunchctlRunnerV1,
    C: ServiceClockV1,
{
    pub fn new(
        filesystem: F,
        launchctl: L,
        clock: C,
        user_id: u32,
    ) -> Result<Self, ServicePathErrorV1> {
        Self::new_with_observer(
            filesystem,
            launchctl,
            clock,
            user_id,
            Arc::new(NoopServiceObserverV1),
        )
    }

    pub fn new_with_observer(
        filesystem: F,
        launchctl: L,
        clock: C,
        user_id: u32,
        observer: Arc<dyn ServiceObserverV1>,
    ) -> Result<Self, ServicePathErrorV1> {
        if user_id == 0 {
            return Err(ServicePathErrorV1::RootUser);
        }
        Ok(Self {
            filesystem,
            launchctl,
            clock,
            observer,
            user_id,
        })
    }

    fn domain(&self) -> String {
        format!("gui/{}", self.user_id)
    }
    fn loaded_target(&self) -> String {
        format!("{}/{}", self.domain(), SERVICE_LABEL_V1)
    }

    fn fs_error(
        &self,
        op: ServiceOperationV1,
        path: &Path,
        error: ServiceFilesystemErrorV1,
    ) -> ServiceErrorV1 {
        if error.permission_denied {
            ServiceErrorV1::PermissionDeniedV1 {
                operation: op,
                path: path.to_path_buf(),
                message: error.message,
            }
        } else {
            ServiceErrorV1::IoV1 {
                operation: Some(op),
                message: error.message,
            }
        }
    }
    fn observe(&self, observation: ServiceObservationV1) {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| self.observer.observe(observation)));
    }
    fn lifecycle_transaction(
        &self,
        operation: ServiceOperationV1,
    ) -> Result<ServiceLifecycleTransactionLockV1, ServiceErrorV1> {
        ServiceLifecycleTransactionLockV1::acquire(operation)
    }

    fn launch(
        &self,
        op: ServiceOperationV1,
        arguments: Vec<String>,
    ) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        self.observe(ServiceObservationV1::LaunchctlSideEffectRequested);
        let output = self.launchctl.run(&arguments)?;
        if output.exit_status == 0 {
            self.observe(ServiceObservationV1::LaunchctlSideEffectCompleted);
            Ok(output)
        } else {
            Err(ServiceErrorV1::LaunchctlFailureV1 {
                operation: op,
                exit_status: Some(output.exit_status),
                message: if output.stderr.is_empty() {
                    output.stdout
                } else {
                    output.stderr
                },
            })
        }
    }

    fn metadata(
        &self,
        op: ServiceOperationV1,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<Option<ServiceInstallMetadataV1>, ServiceErrorV1> {
        match self.metadata_read(op, paths)? {
            ServiceMetadataReadV1::Missing => Ok(None),
            ServiceMetadataReadV1::Present(metadata) => Ok(Some(*metadata)),
            ServiceMetadataReadV1::Oversized => Err(ServiceErrorV1::InvalidMetadataV1 {
                message: "service metadata exceeds its bounded size".to_owned(),
            }),
        }
    }
    fn metadata_read(
        &self,
        op: ServiceOperationV1,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<ServiceMetadataReadV1, ServiceErrorV1> {
        let path = paths.metadata_index_path().as_path();
        if !self
            .filesystem
            .exists(path)
            .map_err(|e| self.fs_error(op, path, e))?
        {
            return Ok(ServiceMetadataReadV1::Missing);
        }
        let bytes = match self
            .filesystem
            .read_file_bounded(path, SERVICE_METADATA_MAX_BYTES_V1)
        {
            Ok(bytes) => bytes,
            Err(error) if error.kind == ServiceFilesystemErrorKindV1::LimitExceeded => {
                return Ok(ServiceMetadataReadV1::Oversized);
            }
            Err(error) => return Err(self.fs_error(op, path, error)),
        };
        if bytes.len() > SERVICE_METADATA_MAX_BYTES_V1 {
            return Ok(ServiceMetadataReadV1::Oversized);
        }
        parse_metadata_v1(&bytes).map(|metadata| ServiceMetadataReadV1::Present(Box::new(metadata)))
    }

    fn coherent_metadata(
        &self,
        op: ServiceOperationV1,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<Option<ServiceInstallMetadataV1>, ServiceErrorV1> {
        let metadata = self.metadata(op, paths)?;
        let plist_path = paths.launch_agent_path().as_path();
        let plist_exists = self
            .filesystem
            .exists(plist_path)
            .map_err(|e| self.fs_error(op, plist_path, e))?;
        match (metadata, plist_exists) {
            (None, false) => Ok(None),
            (Some(_), false) | (None, true) => Err(ServiceErrorV1::InvalidMetadataV1 {
                message: "service publication is incomplete".to_owned(),
            }),
            (Some(metadata), true) => {
                let plist = self
                    .filesystem
                    .read_file_bounded(plist_path, SERVICE_PLIST_MAX_BYTES_V1)
                    .map_err(|e| self.fs_error(op, plist_path, e))?;
                let expected = authenticated_plist_v1(&metadata, paths.log_path().as_path())?;
                if metadata.generation.is_none() || plist != expected {
                    return Err(ServiceErrorV1::InvalidMetadataV1 {
                        message: "service publication authentication does not match".to_owned(),
                    });
                }
                if metadata.publication_state != ServicePublicationStateV1::ReceiptDurable {
                    return Err(ServiceErrorV1::InvalidMetadataV1 {
                        message: "service publication receipt is not durable".to_owned(),
                    });
                }
                let staged_directory = paths
                    .metadata_index_path()
                    .as_path()
                    .parent()
                    .expect("validated metadata path has a parent")
                    .join(SERVICE_STAGED_DAEMONS_DIRECTORY_V1);
                if metadata.daemon_binary() != staged_directory.join(metadata.daemon_identity()) {
                    return Err(ServiceErrorV1::InvalidMetadataV1 {
                        message: "persisted daemon binary is not a controlled staged path"
                            .to_owned(),
                    });
                }
                if !self
                    .filesystem
                    .is_executable(metadata.daemon_binary())
                    .map_err(|error| self.fs_error(op, metadata.daemon_binary(), error))?
                {
                    return Err(ServiceErrorV1::InvalidMetadataV1 {
                        message: "persisted daemon binary is not executable".to_owned(),
                    });
                }
                let daemon_bytes = self
                    .filesystem
                    .read_file_bounded(metadata.daemon_binary(), SERVICE_DAEMON_BINARY_MAX_BYTES_V1)
                    .map_err(|error| self.fs_error(op, metadata.daemon_binary(), error))?;
                if sha256_hex_v1(&daemon_bytes) != metadata.daemon_identity() {
                    return Err(ServiceErrorV1::InvalidMetadataV1 {
                        message: "persisted daemon binary identity does not match".to_owned(),
                    });
                }
                Ok(Some(metadata))
            }
        }
    }

    fn ensure_directories(
        &self,
        op: ServiceOperationV1,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<(), ServiceErrorV1> {
        for path in [
            paths.metadata_index_path().as_path().parent(),
            paths.log_path().as_path().parent(),
            paths.launch_agent_path().as_path().parent(),
            Some(paths.runtime_directory().as_path()),
        ] {
            let path = path.expect("all validated service paths have a parent");
            self.filesystem
                .create_directory(path, 0o700)
                .map_err(|e| self.fs_error(op, path, e))?;
        }
        Ok(())
    }
    fn stage_daemon(
        &self,
        op: ServiceOperationV1,
        paths: &ServiceRuntimePathsV1,
        bytes: &[u8],
        identity: &str,
    ) -> Result<PathBuf, ServiceErrorV1> {
        let parent = paths
            .metadata_index_path()
            .as_path()
            .parent()
            .expect("validated metadata path has a parent");
        let directory = parent.join(SERVICE_STAGED_DAEMONS_DIRECTORY_V1);
        let staged = directory.join(identity);
        if self
            .filesystem
            .exists(&staged)
            .map_err(|error| self.fs_error(op, &staged, error))?
        {
            let staged_bytes = self
                .filesystem
                .read_file_bounded(&staged, SERVICE_DAEMON_BINARY_MAX_BYTES_V1)
                .map_err(|error| self.fs_error(op, &staged, error))?;
            if !self
                .filesystem
                .is_executable(&staged)
                .map_err(|error| self.fs_error(op, &staged, error))?
                || staged_bytes.len() != bytes.len()
                || staged_bytes != bytes
                || sha256_hex_v1(&staged_bytes) != identity
            {
                return Err(ServiceErrorV1::InvalidMetadataV1 {
                    message: "staged daemon binary identity does not match".to_owned(),
                });
            }
            return Ok(staged);
        }

        self.filesystem
            .create_directory(&directory, 0o700)
            .map_err(|error| self.fs_error(op, &directory, error))?;
        self.filesystem
            .write_atomically(&staged, bytes, 0o700)
            .map_err(|error| self.fs_error(op, &staged, error))?;
        let staged_bytes = self
            .filesystem
            .read_file_bounded(&staged, SERVICE_DAEMON_BINARY_MAX_BYTES_V1)
            .map_err(|error| self.fs_error(op, &staged, error))?;
        if !self
            .filesystem
            .is_executable(&staged)
            .map_err(|error| self.fs_error(op, &staged, error))?
            || staged_bytes.len() != bytes.len()
            || staged_bytes != bytes
            || sha256_hex_v1(&staged_bytes) != identity
        {
            return Err(ServiceErrorV1::InvalidMetadataV1 {
                message: "staged daemon binary identity does not match".to_owned(),
            });
        }
        Ok(staged)
    }
    fn reconcile_staged_daemons(
        &self,
        op: ServiceOperationV1,
        paths: &ServiceRuntimePathsV1,
        keep: Option<&Path>,
    ) -> Result<(), ServiceErrorV1> {
        let directory = paths
            .metadata_index_path()
            .as_path()
            .parent()
            .expect("validated metadata path has a parent")
            .join(SERVICE_STAGED_DAEMONS_DIRECTORY_V1);
        if !self
            .filesystem
            .exists(&directory)
            .map_err(|error| self.fs_error(op, &directory, error))?
        {
            return Ok(());
        }
        let entries = self
            .filesystem
            .list_directory_bounded(&directory, SERVICE_STAGED_DAEMONS_MAX_ENTRIES_V1)
            .map_err(|error| self.fs_error(op, &directory, error))?;
        for entry in entries {
            let name = entry
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .ok_or_else(|| ServiceErrorV1::InvalidMetadataV1 {
                    message: "staged daemon entry name is not canonical UTF-8".to_owned(),
                })?;
            if entry.parent() != Some(directory.as_path()) {
                return Err(ServiceErrorV1::InvalidMetadataV1 {
                    message: "staged daemon directory contains an unowned entry".to_owned(),
                });
            }
            if StdServiceFilesystemV1::is_owned_staged_temporary_name_v1(name) {
                self.filesystem
                    .remove_file(&entry)
                    .map_err(|error| self.fs_error(op, &entry, error))?;
                continue;
            }
            if name.len() != 64
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(ServiceErrorV1::InvalidMetadataV1 {
                    message: "staged daemon directory contains an unowned entry".to_owned(),
                });
            }
            if keep == Some(entry.as_path()) {
                continue;
            }
            self.filesystem
                .remove_file(&entry)
                .map_err(|error| self.fs_error(op, &entry, error))?;
        }
        if keep.is_none() {
            self.filesystem
                .remove_directory(&directory)
                .map_err(|error| self.fs_error(op, &directory, error))?;
        }
        Ok(())
    }

    fn bootstrap(
        &self,
        op: ServiceOperationV1,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<(), ServiceErrorV1> {
        let arguments = vec![
            "bootstrap".to_owned(),
            self.domain(),
            paths.launch_agent_path().as_path().display().to_string(),
        ];
        self.observe(ServiceObservationV1::LaunchctlSideEffectRequested);
        let output = self.launchctl.run(&arguments)?;
        if output.exit_status == 0 {
            self.observe(ServiceObservationV1::LaunchctlSideEffectCompleted);
            return Ok(());
        }
        if output.already_loaded_bootstrap() {
            self.observe(ServiceObservationV1::LaunchctlSideEffectRequested);
            let loaded = self
                .launchctl
                .run(&["print".to_owned(), self.loaded_target()])?;
            if launchctl_loaded_state_v1(&loaded, &self.loaded_target()) {
                self.observe(ServiceObservationV1::LaunchctlSideEffectCompleted);
                return Ok(());
            }
        }
        Err(ServiceErrorV1::LaunchctlFailureV1 {
            operation: op,
            exit_status: Some(output.exit_status),
            message: if output.stderr.is_empty() {
                output.stdout
            } else {
                output.stderr
            },
        })
    }

    fn bootout(&self, op: ServiceOperationV1) -> Result<(), ServiceErrorV1> {
        self.launch(op, vec!["bootout".to_owned(), self.loaded_target()])?;
        Ok(())
    }
    fn loaded_or_not_loaded(&self, op: ServiceOperationV1) -> Result<bool, ServiceErrorV1> {
        self.observe(ServiceObservationV1::LaunchctlSideEffectRequested);
        let output = self
            .launchctl
            .run(&["print".to_owned(), self.loaded_target()])?;
        self.observe(ServiceObservationV1::LaunchctlSideEffectCompleted);
        if launchctl_loaded_state_v1(&output, &self.loaded_target()) {
            Ok(true)
        } else if launchctl_not_loaded_v1(&output, &self.domain()) {
            Ok(false)
        } else {
            Err(ServiceErrorV1::LaunchctlFailureV1 {
                operation: op,
                exit_status: Some(output.exit_status),
                message: if output.stderr.is_empty() {
                    output.stdout
                } else {
                    output.stderr
                },
            })
        }
    }
    fn publish_metadata(
        &self,
        op: ServiceOperationV1,
        path: &Path,
        metadata: &ServiceInstallMetadataV1,
    ) -> Result<(), ServiceErrorV1> {
        let json = metadata_json_v1(metadata)?;
        self.filesystem
            .write_atomically(path, json.as_bytes(), 0o600)
            .map_err(|error| self.fs_error(op, path, error))?;
        self.observe(ServiceObservationV1::AtomicMetadataPublished);
        Ok(())
    }
    fn remove_stale_socket(
        &self,
        op: ServiceOperationV1,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<(), ServiceErrorV1> {
        let socket = paths.socket_path().as_path();
        let existed = self
            .filesystem
            .exists(socket)
            .map_err(|error| self.fs_error(op, socket, error))?;
        self.filesystem
            .remove_file(socket)
            .map_err(|error| self.fs_error(op, socket, error))?;
        if existed {
            self.observe(ServiceObservationV1::StaleSocketRemoved);
        }
        Ok(())
    }

    fn rotate_log(
        &self,
        op: ServiceOperationV1,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<(), ServiceErrorV1> {
        let log = paths.log_path().as_path();
        self.filesystem
            .rotate_file(log, SERVICE_LOG_MAX_BYTES_V1, SERVICE_LOG_RETAINED_FILES_V1)
            .map_err(|error| self.fs_error(op, log, error))?;
        self.observe(ServiceObservationV1::LogRotationCompleted);
        Ok(())
    }

    fn verify_staged_daemon_version(
        &self,
        op: ServiceOperationV1,
        binary: &Path,
        expected_version: &str,
    ) -> Result<(), ServiceErrorV1> {
        let output = Command::new(binary)
            .arg("--version")
            .output()
            .map_err(|error| ServiceErrorV1::IoV1 {
                operation: Some(op),
                message: error.to_string(),
            })?;
        let observed = std::str::from_utf8(&output.stdout)
            .ok()
            .and_then(|stdout| stdout.strip_suffix('\n'))
            .filter(|version| !version.contains('\n') && !version.contains('\r'));
        if !output.status.success()
            || !output.stderr.is_empty()
            || observed != Some(expected_version)
        {
            return Err(ServiceErrorV1::InvalidExecutableV1 {
                message: "staged daemon version is incompatible with this CLI".to_owned(),
            });
        }
        Ok(())
    }

    fn install_or_update(
        &self,
        op: ServiceOperationV1,
        spec: InstallSpecV1,
        update_only: bool,
    ) -> Result<ServiceCommandResultV1, ServiceErrorV1> {
        let paths = spec.runtime_paths();
        let binary = spec.daemon_executable_path().as_path();
        if !self
            .filesystem
            .is_executable(binary)
            .map_err(|e| self.fs_error(op, binary, e))?
        {
            return Err(ServiceErrorV1::InvalidExecutableV1 {
                message: format!("daemon binary is not executable: {}", binary.display()),
            });
        }
        let daemon_bytes = self
            .filesystem
            .read_file_bounded(binary, SERVICE_DAEMON_BINARY_MAX_BYTES_V1)
            .map_err(|error| self.fs_error(op, binary, error))?;
        validate_native_arm64_macos_macho_v1(&daemon_bytes)?;
        let daemon_identity = sha256_hex_v1(&daemon_bytes);
        let plist_path = paths.launch_agent_path().as_path();
        let exists = self
            .filesystem
            .exists(plist_path)
            .map_err(|e| self.fs_error(op, plist_path, e))?;
        let existing = match self.metadata_read(op, paths)? {
            ServiceMetadataReadV1::Missing | ServiceMetadataReadV1::Oversized => None,
            ServiceMetadataReadV1::Present(metadata) => Some(*metadata),
        };
        if update_only && existing.is_none() && !exists {
            return Ok(ServiceCommandResultV1::Outcome(
                ServiceOutcomeV1::NotInstalledV1(ServiceNotInstalledV1::new(self.clock.now())),
            ));
        }
        self.reconcile_staged_daemons(
            op,
            paths,
            existing
                .as_ref()
                .map(ServiceInstallMetadataV1::daemon_binary),
        )?;
        let staged_binary = self.stage_daemon(op, paths, &daemon_bytes, &daemon_identity)?;
        if let Some(expected_version) = spec.expected_daemon_version.as_deref() {
            self.verify_staged_daemon_version(op, &staged_binary, expected_version)?;
        }
        let now = self.clock.now();
        let same_binary = existing.as_ref().is_some_and(|metadata| {
            metadata.daemon_binary() == staged_binary
                && metadata.daemon_identity() == daemon_identity
        });
        let metadata = ServiceInstallMetadataV1::new(
            &staged_binary,
            existing
                .as_ref()
                .map_or(now, ServiceInstallMetadataV1::installed_at),
            if same_binary {
                existing
                    .as_ref()
                    .expect("same binary has existing metadata")
                    .updated_at()
            } else {
                now
            },
        )
        .map_err(|error| ServiceErrorV1::InvalidMetadataV1 {
            message: error.to_string(),
        })?
        .with_daemon_identity(daemon_identity)
        .map_err(|error| ServiceErrorV1::InvalidMetadataV1 {
            message: error.to_string(),
        })?;
        let plist_without_generation = launch_agent_plist_with_generation_v1(
            &staged_binary,
            paths.log_path().as_path(),
            None,
            Some(metadata.daemon_identity()),
        );
        let prepared = metadata
            .clone()
            .with_publication_state(ServicePublicationStateV1::Prepared)
            .with_generation_for_plist(&plist_without_generation);
        let metadata = metadata
            .with_publication_state(ServicePublicationStateV1::ReceiptDurable)
            .with_generation_for_plist(&plist_without_generation);
        let plist = launch_agent_plist_with_generation_v1(
            &staged_binary,
            paths.log_path().as_path(),
            metadata.generation.as_deref(),
            Some(metadata.daemon_identity()),
        );
        let observed_plist = if exists {
            match self
                .filesystem
                .read_file_bounded(plist_path, SERVICE_PLIST_MAX_BYTES_V1)
            {
                Ok(plist) => Some(plist),
                Err(error) if error.kind == ServiceFilesystemErrorKindV1::LimitExceeded => None,
                Err(error) => return Err(self.fs_error(op, plist_path, error)),
            }
        } else {
            None
        };
        if existing.as_ref() == Some(&metadata) && observed_plist.as_deref() == Some(&plist) {
            self.reconcile_staged_daemons(op, paths, Some(&staged_binary))?;
            return Ok(ServiceCommandResultV1::Outcome(
                ServiceOutcomeV1::AlreadyInDesiredStateV1(ServiceAlreadyV1::new(
                    self.clock.now(),
                    metadata,
                )),
            ));
        }
        self.ensure_directories(op, paths)?;
        let metadata_path = paths.metadata_index_path().as_path();
        if existing.as_ref() == Some(&prepared) && observed_plist.as_deref() == Some(&plist) {
            if !self.loaded_or_not_loaded(op)? {
                self.remove_stale_socket(op, paths)?;
                self.bootstrap(op, paths)?;
            }
            self.publish_metadata(op, metadata_path, &metadata)?;
            self.reconcile_staged_daemons(op, paths, Some(&staged_binary))?;
            return Ok(ServiceCommandResultV1::Outcome(
                ServiceOutcomeV1::ChangedV1(ServiceChangedV1::new(now, Some(metadata))),
            ));
        }
        if self.loaded_or_not_loaded(op)? {
            self.bootout(op)?;
        }
        self.remove_stale_socket(op, paths)?;
        self.publish_metadata(op, metadata_path, &prepared)?;
        self.filesystem
            .write_atomically(plist_path, &plist, 0o600)
            .map_err(|error| self.fs_error(op, plist_path, error))?;
        self.observe(ServiceObservationV1::AtomicPlistPublished);
        self.rotate_log(op, paths)?;
        self.bootstrap(op, paths)?;
        self.publish_metadata(op, metadata_path, &metadata)?;
        self.reconcile_staged_daemons(op, paths, Some(&staged_binary))?;
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(ServiceChangedV1::new(now, Some(metadata))),
        ))
    }
    fn uninstall(
        &self,
        paths: &ServiceRuntimePathsV1,
        options: UninstallOptionsV1,
    ) -> Result<ServiceCommandResultV1, ServiceErrorV1> {
        let plist = paths.launch_agent_path().as_path();
        let metadata = paths.metadata_index_path().as_path();
        let installed = self
            .filesystem
            .exists(plist)
            .map_err(|e| self.fs_error(ServiceOperationV1::Uninstall, plist, e))?;
        let has_metadata = self
            .filesystem
            .exists(metadata)
            .map_err(|e| self.fs_error(ServiceOperationV1::Uninstall, metadata, e))?;
        let staged_directory = metadata
            .parent()
            .expect("validated metadata path has a parent")
            .join(SERVICE_STAGED_DAEMONS_DIRECTORY_V1);
        let has_staged_daemons = self.filesystem.exists(&staged_directory).map_err(|error| {
            self.fs_error(ServiceOperationV1::Uninstall, &staged_directory, error)
        })?;
        let runtime_files = [
            paths.socket_path().as_path(),
            paths.global_lock_path().as_path(),
        ];
        let mut has_runtime_file = false;
        for path in runtime_files {
            if self
                .filesystem
                .exists(path)
                .map_err(|e| self.fs_error(ServiceOperationV1::Uninstall, path, e))?
            {
                has_runtime_file = true;
            }
        }
        if installed && has_metadata {
            self.coherent_metadata(ServiceOperationV1::Uninstall, paths)?;
        }
        let loaded = self.loaded_or_not_loaded(ServiceOperationV1::Uninstall)?;
        if !installed
            && !has_metadata
            && !has_staged_daemons
            && !has_runtime_file
            && !loaded
            && !options.purge_logs()
        {
            return Ok(ServiceCommandResultV1::Outcome(
                ServiceOutcomeV1::NotInstalledV1(ServiceNotInstalledV1::new(self.clock.now())),
            ));
        }
        if loaded {
            self.bootout(ServiceOperationV1::Uninstall)?;
        }
        if installed {
            self.filesystem
                .remove_file(plist)
                .map_err(|e| self.fs_error(ServiceOperationV1::Uninstall, plist, e))?;
        }
        if has_metadata {
            self.filesystem
                .remove_file(metadata)
                .map_err(|e| self.fs_error(ServiceOperationV1::Uninstall, metadata, e))?;
        }
        for path in runtime_files {
            if self
                .filesystem
                .exists(path)
                .map_err(|e| self.fs_error(ServiceOperationV1::Uninstall, path, e))?
            {
                self.filesystem
                    .remove_file(path)
                    .map_err(|e| self.fs_error(ServiceOperationV1::Uninstall, path, e))?;
            }
        }
        if has_staged_daemons {
            self.reconcile_staged_daemons(ServiceOperationV1::Uninstall, paths, None)?;
        }
        if options.purge_logs() {
            let log = paths.log_path().as_path();
            self.filesystem
                .remove_file(log)
                .map_err(|e| self.fs_error(ServiceOperationV1::Uninstall, log, e))?;
            for index in 1..=SERVICE_LOG_RETAINED_FILES_V1 {
                let rotated = PathBuf::from(format!("{}.{}", log.display(), index));
                self.filesystem
                    .remove_file(&rotated)
                    .map_err(|e| self.fs_error(ServiceOperationV1::Uninstall, &rotated, e))?;
            }
            self.observe(ServiceObservationV1::UninstallLogsPurged);
        } else {
            self.observe(ServiceObservationV1::UninstallLogsPreserved);
        }
        Ok(ServiceCommandResultV1::Outcome(
            ServiceOutcomeV1::ChangedV1(ServiceChangedV1::new(self.clock.now(), None)),
        ))
    }
}

impl<F, L, C> ServiceCommandRunnerV1 for MacosServiceCommandRunnerV1<F, L, C>
where
    F: ServiceFilesystemV1,
    L: LaunchctlRunnerV1,
    C: ServiceClockV1,
{
    fn run(&self, command: ServiceCommandV1) -> Result<ServiceCommandResultV1, ServiceErrorV1> {
        let operation = command.operation();
        let result = (|| {
            let transaction = match &command {
                ServiceCommandV1::Install { .. } | ServiceCommandV1::Update { .. } => {
                    Some(self.lifecycle_transaction(command.operation())?)
                }
                ServiceCommandV1::Start { .. }
                | ServiceCommandV1::Stop { .. }
                | ServiceCommandV1::Restart { .. }
                | ServiceCommandV1::Uninstall { .. }
                | ServiceCommandV1::UninstallWithOptions { .. }
                | ServiceCommandV1::Status { .. } => {
                    Some(self.lifecycle_transaction(command.operation())?)
                }
                ServiceCommandV1::Logs { .. } => None,
            };
            let _transaction = transaction;
            match command {
                ServiceCommandV1::Install { spec, .. } => {
                    self.install_or_update(ServiceOperationV1::Install, spec, false)
                }
                ServiceCommandV1::Update { spec, .. } => {
                    self.install_or_update(ServiceOperationV1::Update, spec, true)
                }
                ServiceCommandV1::Start { paths, .. } => {
                    let plist = paths.launch_agent_path().as_path();
                    if !self
                        .filesystem
                        .exists(plist)
                        .map_err(|e| self.fs_error(ServiceOperationV1::Start, plist, e))?
                    {
                        if self.loaded_or_not_loaded(ServiceOperationV1::Start)? {
                            self.bootout(ServiceOperationV1::Start)?;
                        }
                        return Ok(ServiceCommandResultV1::Outcome(
                            ServiceOutcomeV1::NotInstalledV1(ServiceNotInstalledV1::new(
                                self.clock.now(),
                            )),
                        ));
                    }
                    let metadata = self.coherent_metadata(ServiceOperationV1::Start, &paths)?;
                    if self.loaded_or_not_loaded(ServiceOperationV1::Start)? {
                        return Ok(ServiceCommandResultV1::Outcome(
                            ServiceOutcomeV1::RunningV1(ServiceRunningV1::new(
                                self.clock.now(),
                                None,
                                metadata,
                            )),
                        ));
                    }
                    self.rotate_log(ServiceOperationV1::Start, &paths)?;
                    self.remove_stale_socket(ServiceOperationV1::Start, &paths)?;
                    self.bootstrap(ServiceOperationV1::Start, &paths)?;
                    Ok(ServiceCommandResultV1::Outcome(
                        ServiceOutcomeV1::ChangedV1(ServiceChangedV1::new(
                            self.clock.now(),
                            metadata,
                        )),
                    ))
                }
                ServiceCommandV1::Stop { paths, .. } => {
                    let plist = paths.launch_agent_path().as_path();
                    if !self
                        .filesystem
                        .exists(plist)
                        .map_err(|e| self.fs_error(ServiceOperationV1::Stop, plist, e))?
                    {
                        if self.loaded_or_not_loaded(ServiceOperationV1::Stop)? {
                            self.bootout(ServiceOperationV1::Stop)?;
                        }
                        return Ok(ServiceCommandResultV1::Outcome(
                            ServiceOutcomeV1::NotInstalledV1(ServiceNotInstalledV1::new(
                                self.clock.now(),
                            )),
                        ));
                    }
                    self.coherent_metadata(ServiceOperationV1::Stop, &paths)?;
                    if self.loaded_or_not_loaded(ServiceOperationV1::Stop)? {
                        self.bootout(ServiceOperationV1::Stop)?;
                    }
                    Ok(ServiceCommandResultV1::Outcome(
                        ServiceOutcomeV1::StoppedV1(ServiceStoppedV1::new(
                            self.clock.now(),
                            self.metadata(ServiceOperationV1::Stop, &paths)?,
                        )),
                    ))
                }
                ServiceCommandV1::Restart { paths, .. } => {
                    let plist = paths.launch_agent_path().as_path();
                    if !self
                        .filesystem
                        .exists(plist)
                        .map_err(|e| self.fs_error(ServiceOperationV1::Restart, plist, e))?
                    {
                        if self.loaded_or_not_loaded(ServiceOperationV1::Restart)? {
                            self.bootout(ServiceOperationV1::Restart)?;
                        }
                        return Ok(ServiceCommandResultV1::Outcome(
                            ServiceOutcomeV1::NotInstalledV1(ServiceNotInstalledV1::new(
                                self.clock.now(),
                            )),
                        ));
                    }
                    let metadata = self.coherent_metadata(ServiceOperationV1::Restart, &paths)?;
                    if self.loaded_or_not_loaded(ServiceOperationV1::Restart)? {
                        self.bootout(ServiceOperationV1::Restart)?;
                    }
                    self.remove_stale_socket(ServiceOperationV1::Restart, &paths)?;
                    self.rotate_log(ServiceOperationV1::Restart, &paths)?;
                    self.bootstrap(ServiceOperationV1::Restart, &paths)?;
                    Ok(ServiceCommandResultV1::Outcome(
                        ServiceOutcomeV1::ChangedV1(ServiceChangedV1::new(
                            self.clock.now(),
                            metadata,
                        )),
                    ))
                }
                ServiceCommandV1::Status { paths, .. } => {
                    let metadata = self.coherent_metadata(ServiceOperationV1::Status, &paths)?;
                    if metadata.is_none() {
                        if self.loaded_or_not_loaded(ServiceOperationV1::Status)? {
                            return Err(ServiceErrorV1::InvalidMetadataV1 {
                                message: "service is loaded without a coherent publication"
                                    .to_owned(),
                            });
                        }
                        return Ok(ServiceCommandResultV1::Status(
                            ServiceStatusV1::NotInstalledV1(ServiceNotInstalledV1::new(
                                self.clock.now(),
                            )),
                        ));
                    }
                    self.observe(ServiceObservationV1::LaunchctlSideEffectRequested);
                    let output = self
                        .launchctl
                        .run(&["print".to_owned(), self.loaded_target()])?;
                    self.observe(ServiceObservationV1::LaunchctlSideEffectCompleted);
                    match output {
                        output if launchctl_loaded_state_v1(&output, &self.loaded_target()) => {
                            Ok(ServiceCommandResultV1::Status(ServiceStatusV1::RunningV1(
                                ServiceRunningV1::new(
                                    self.clock.now(),
                                    parse_pid(&output.stdout),
                                    metadata,
                                ),
                            )))
                        }
                        output if launchctl_not_loaded_v1(&output, &self.domain()) => {
                            Ok(ServiceCommandResultV1::Status(ServiceStatusV1::StoppedV1(
                                ServiceStoppedV1::new(self.clock.now(), metadata),
                            )))
                        }
                        output => Err(ServiceErrorV1::LaunchctlFailureV1 {
                            operation: ServiceOperationV1::Status,
                            exit_status: Some(output.exit_status),
                            message: if output.stderr.is_empty() {
                                output.stdout
                            } else {
                                output.stderr
                            },
                        }),
                    }
                }
                ServiceCommandV1::Logs { paths, .. } => {
                    let path = paths.log_path().as_path();
                    if self
                        .filesystem
                        .exists(path)
                        .map_err(|e| self.fs_error(ServiceOperationV1::Logs, path, e))?
                    {
                        Ok(ServiceCommandResultV1::LogLocation(LogLocationV1::new(
                            paths.log_path().clone(),
                        )))
                    } else {
                        Err(ServiceErrorV1::LogUnavailableV1 {
                            message: format!("daemon log does not exist: {}", path.display()),
                        })
                    }
                }
                ServiceCommandV1::Uninstall { paths, .. } => {
                    self.uninstall(&paths, UninstallOptionsV1::default())
                }
                ServiceCommandV1::UninstallWithOptions { paths, options, .. } => {
                    self.uninstall(&paths, options)
                }
            }
        })();
        let result = result.map_err(|error| error.with_operation(operation));
        self.observe(if result.is_ok() {
            ServiceObservationV1::ServiceOutcome(operation)
        } else {
            ServiceObservationV1::Error(operation)
        });
        result
    }
}

/// Generates the exact v1 LaunchAgent template with XML-sensitive values escaped.
pub fn launch_agent_plist_v1(binary: &Path, log_path: &Path) -> Vec<u8> {
    launch_agent_plist_with_generation_v1(binary, log_path, None, None)
}

fn launch_agent_plist_with_generation_v1(
    binary: &Path,
    log_path: &Path,
    generation: Option<&str>,
    daemon_identity: Option<&str>,
) -> Vec<u8> {
    let generation = generation.map_or_else(String::new, |value| {
        format!("\n  <key>PodwayGeneration</key>\n  <string>{value}</string>\n")
    });
    let daemon_identity = daemon_identity.map_or_else(String::new, |value| {
        format!("\n  <key>PodwayDaemonSha256</key>\n  <string>{value}</string>\n")
    });
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{SERVICE_LABEL_V1}</string>{generation}{daemon_identity}\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>--service</string>\n  </array>\n\n  <key>RunAtLoad</key>\n  <true/>\n\n  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n\n  <key>ThrottleInterval</key>\n  <integer>5</integer>\n\n  <key>ProcessType</key>\n  <string>Background</string>\n\n  <key>StandardOutPath</key>\n  <string>{}</string>\n\n  <key>StandardErrorPath</key>\n  <string>{}</string>\n</dict>\n</plist>\n", xml_escape(&binary.display().to_string()), xml_escape(&log_path.display().to_string()), xml_escape(&log_path.display().to_string())).into_bytes()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ServicePublicationStateWireV1 {
    Prepared,
    ReceiptDurable,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ServiceMetadataWireV1 {
    version: u16,
    label: String,
    daemon_binary: String,
    daemon_identity: String,
    artifact_role: ServiceArtifactRoleV1,
    installed_at: u64,
    updated_at: u64,
    publication_state: ServicePublicationStateWireV1,
    generation: String,
}

fn metadata_json_v1(metadata: &ServiceInstallMetadataV1) -> Result<String, ServiceErrorV1> {
    serde_json::to_string(&ServiceMetadataWireV1 {
        version: metadata.version,
        label: metadata.label.clone(),
        daemon_binary: metadata.daemon_binary.display().to_string(),
        daemon_identity: metadata.daemon_identity.clone(),
        artifact_role: metadata.artifact_role,
        installed_at: metadata.installed_at.get(),
        updated_at: metadata.updated_at.get(),
        publication_state: match metadata.publication_state {
            ServicePublicationStateV1::Prepared => ServicePublicationStateWireV1::Prepared,
            ServicePublicationStateV1::ReceiptDurable => {
                ServicePublicationStateWireV1::ReceiptDurable
            }
        },
        generation: metadata.generation.clone().ok_or_else(|| {
            ServiceErrorV1::InvalidMetadataV1 {
                message: "metadata generation is missing".to_owned(),
            }
        })?,
    })
    .map(|json| format!("{json}\n"))
    .map_err(|error| ServiceErrorV1::InvalidMetadataV1 {
        message: format!("could not serialize metadata: {error}"),
    })
}

#[derive(Serialize)]
struct ServiceGenerationPreimageV1<'a> {
    version: u16,
    label: &'a str,
    daemon_binary: String,
    daemon_identity: &'a str,
    artifact_role: ServiceArtifactRoleV1,
    installed_at: u64,
    updated_at: u64,
}

fn publication_generation_v1(
    metadata: &ServiceInstallMetadataV1,
    plist_without_generation: &[u8],
) -> String {
    let stable_metadata = serde_json::to_vec(&ServiceGenerationPreimageV1 {
        version: metadata.version(),
        label: metadata.label(),
        daemon_binary: metadata.daemon_binary().display().to_string(),
        daemon_identity: metadata.daemon_identity(),
        artifact_role: metadata.artifact_role,
        installed_at: metadata.installed_at().get(),
        updated_at: metadata.updated_at().get(),
    })
    .expect("service generation preimage contains only infallible JSON values");
    let mut authenticated =
        Vec::with_capacity(plist_without_generation.len() + stable_metadata.len() + 1);
    authenticated.extend_from_slice(plist_without_generation);
    authenticated.push(b'\n');
    authenticated.extend_from_slice(&stable_metadata);
    sha256_hex_v1(&authenticated)
}

fn authenticated_plist_v1(
    metadata: &ServiceInstallMetadataV1,
    log_path: &Path,
) -> Result<Vec<u8>, ServiceErrorV1> {
    if !is_sha256_hex_v1(metadata.daemon_identity()) {
        return Err(ServiceErrorV1::InvalidMetadataV1 {
            message: "metadata daemon identity is invalid".to_owned(),
        });
    }
    let plist_without_generation = launch_agent_plist_with_generation_v1(
        metadata.daemon_binary(),
        log_path,
        None,
        Some(metadata.daemon_identity()),
    );
    let expected_generation = publication_generation_v1(metadata, &plist_without_generation);
    if metadata.generation.as_deref() != Some(expected_generation.as_str()) {
        return Err(ServiceErrorV1::InvalidMetadataV1 {
            message: "metadata generation is invalid".to_owned(),
        });
    }
    Ok(launch_agent_plist_with_generation_v1(
        metadata.daemon_binary(),
        log_path,
        Some(&expected_generation),
        Some(metadata.daemon_identity()),
    ))
}

fn sha256_hex_v1(input: &[u8]) -> String {
    format!("{:x}", Sha256::digest(input))
}
pub fn validate_native_arm64_macos_macho_v1(bytes: &[u8]) -> Result<(), ServiceErrorV1> {
    const MACH_HEADER_64_BYTES: usize = 32;
    const MACHO_64_LE_MAGIC: [u8; 4] = [0xcf, 0xfa, 0xed, 0xfe];
    const CPU_TYPE_ARM64: u32 = 0x0100_000c;
    const LC_VERSION_MIN_MACOSX: u32 = 0x24;
    const LC_BUILD_VERSION: u32 = 0x32;

    if bytes.len() < MACH_HEADER_64_BYTES || bytes.get(..4) != Some(MACHO_64_LE_MAGIC.as_slice()) {
        return Err(ServiceErrorV1::InvalidExecutableV1 {
            message: "daemon is not a thin 64-bit arm64 Mach-O executable".to_owned(),
        });
    }
    let read_u32 = |offset| {
        bytes
            .get(offset..offset + 4)
            .map(|field| u32::from_le_bytes(field.try_into().expect("four-byte field")))
    };
    if read_u32(4) != Some(CPU_TYPE_ARM64) {
        return Err(ServiceErrorV1::InvalidExecutableV1 {
            message: "daemon Mach-O architecture is not arm64".to_owned(),
        });
    }
    let Some(command_count) = read_u32(16) else {
        return Err(ServiceErrorV1::InvalidExecutableV1 {
            message: "daemon Mach-O header is truncated".to_owned(),
        });
    };
    let Some(command_bytes) = read_u32(20).and_then(|value| usize::try_from(value).ok()) else {
        return Err(ServiceErrorV1::InvalidExecutableV1 {
            message: "daemon Mach-O load-command size is invalid".to_owned(),
        });
    };
    let Some(commands_end) = MACH_HEADER_64_BYTES.checked_add(command_bytes) else {
        return Err(ServiceErrorV1::InvalidExecutableV1 {
            message: "daemon Mach-O load-command size overflows".to_owned(),
        });
    };
    if commands_end > bytes.len()
        || usize::try_from(command_count)
            .ok()
            .is_none_or(|count| count > command_bytes / 8)
    {
        return Err(ServiceErrorV1::InvalidExecutableV1 {
            message: "daemon Mach-O load commands are truncated or invalid".to_owned(),
        });
    }

    let mut offset = MACH_HEADER_64_BYTES;
    let mut has_macos_deployment_command = false;
    for _ in 0..command_count {
        let Some(command) = read_u32(offset) else {
            return Err(ServiceErrorV1::InvalidExecutableV1 {
                message: "daemon Mach-O load command is truncated".to_owned(),
            });
        };
        let Some(size) = read_u32(offset + 4).and_then(|value| usize::try_from(value).ok()) else {
            return Err(ServiceErrorV1::InvalidExecutableV1 {
                message: "daemon Mach-O load command size is invalid".to_owned(),
            });
        };
        let Some(next_offset) = offset.checked_add(size) else {
            return Err(ServiceErrorV1::InvalidExecutableV1 {
                message: "daemon Mach-O load command size overflows".to_owned(),
            });
        };
        if size < 8 || next_offset > commands_end {
            return Err(ServiceErrorV1::InvalidExecutableV1 {
                message: "daemon Mach-O load command is invalid".to_owned(),
            });
        }
        has_macos_deployment_command |= matches!(command, LC_VERSION_MIN_MACOSX | LC_BUILD_VERSION);
        offset = next_offset;
    }
    if !has_macos_deployment_command {
        return Err(ServiceErrorV1::InvalidExecutableV1 {
            message: "daemon Mach-O lacks a macOS deployment load command".to_owned(),
        });
    }
    Ok(())
}

fn is_sha256_hex_v1(value: &str) -> bool {
    value.len() == SERVICE_BINARY_IDENTITY_HEX_LENGTH_V1
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_metadata_v1(bytes: &[u8]) -> Result<ServiceInstallMetadataV1, ServiceErrorV1> {
    if bytes.len() > SERVICE_METADATA_MAX_BYTES_V1 {
        return Err(ServiceErrorV1::InvalidMetadataV1 {
            message: "metadata exceeds maximum size".to_owned(),
        });
    }
    reject_duplicate_metadata_keys_v1(bytes)?;
    let wire: ServiceMetadataWireV1 =
        serde_json::from_slice(bytes).map_err(|_| ServiceErrorV1::InvalidMetadataV1 {
            message: "metadata is malformed".to_owned(),
        })?;
    if wire.version != SERVICE_METADATA_VERSION_V1 || wire.label != SERVICE_LABEL_V1 {
        return Err(ServiceErrorV1::InvalidMetadataV1 {
            message: "metadata version or label is unsupported".to_owned(),
        });
    }
    if !is_sha256_hex_v1(&wire.daemon_identity) || !is_sha256_hex_v1(&wire.generation) {
        return Err(ServiceErrorV1::InvalidMetadataV1 {
            message: "metadata digest is invalid".to_owned(),
        });
    }
    let mut metadata = ServiceInstallMetadataV1::new(
        wire.daemon_binary,
        UnixMillis::new(wire.installed_at),
        UnixMillis::new(wire.updated_at),
    )
    .map_err(|error| ServiceErrorV1::InvalidMetadataV1 {
        message: error.to_string(),
    })?
    .with_daemon_identity(wire.daemon_identity)
    .map_err(|error| ServiceErrorV1::InvalidMetadataV1 {
        message: error.to_string(),
    })?;
    metadata.artifact_role = wire.artifact_role;
    metadata.publication_state = match wire.publication_state {
        ServicePublicationStateWireV1::Prepared => ServicePublicationStateV1::Prepared,
        ServicePublicationStateWireV1::ReceiptDurable => ServicePublicationStateV1::ReceiptDurable,
    };
    metadata.generation = Some(wire.generation);
    Ok(metadata)
}
struct DuplicateFreeMetadataKeysV1;

impl<'de> Visitor<'de> for DuplicateFreeMetadataKeysV1 {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a metadata object with unique keys")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(serde::de::Error::custom(
                    "metadata contains a duplicate key",
                ));
            }
            map.next_value::<IgnoredAny>()?;
        }
        Ok(())
    }
}

fn reject_duplicate_metadata_keys_v1(bytes: &[u8]) -> Result<(), ServiceErrorV1> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    deserializer
        .deserialize_map(DuplicateFreeMetadataKeysV1)
        .and_then(|()| deserializer.end())
        .map_err(|_| ServiceErrorV1::InvalidMetadataV1 {
            message: "metadata is malformed".to_owned(),
        })
}

fn launchctl_not_loaded_v1(output: &LaunchctlOutputV1, domain: &str) -> bool {
    let Some(user_id) = domain.strip_prefix("gui/") else {
        return false;
    };
    let current = format!(
        "Bad request.\nCould not find service \"{SERVICE_LABEL_V1}\" in domain for user gui: {user_id}"
    );
    let legacy = format!("Could not find service \"{SERVICE_LABEL_V1}\" in domain for {domain}");
    output.stdout.is_empty()
        && ((output.exit_status == 113
            && (output.stderr == current || output.stderr == format!("{current}\n")))
            || (output.exit_status == 3
                && (output.stderr == legacy || output.stderr == format!("{legacy}\n"))))
}

fn launchctl_loaded_state_v1(output: &LaunchctlOutputV1, target: &str) -> bool {
    output.exit_status == 0
        && output.stderr.is_empty()
        && output
            .stdout
            .lines()
            .find(|line| !line.trim().is_empty())
            .is_some_and(|line| line.trim() == format!("{target} = {{"))
}
fn parse_pid(output: &str) -> Option<u32> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("pid = ")?.parse().ok())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn native_arm64_macho() -> Vec<u8> {
        let mut bytes = vec![0_u8; 40];
        bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        bytes[4..8].copy_from_slice(&0x0100_000cu32.to_le_bytes());
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&8u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&0x32u32.to_le_bytes());
        bytes[36..40].copy_from_slice(&8u32.to_le_bytes());
        bytes
    }

    fn path(value: &str) -> LocalPlatformPathV1 {
        LocalPlatformPathV1::new(value).expect("test path is valid")
    }

    #[test]
    fn production_native_arm64_macos_macho_is_accepted() {
        assert_eq!(
            validate_native_arm64_macos_macho_v1(&native_arm64_macho()),
            Ok(())
        );
    }

    #[test]
    fn production_matching_version_script_is_rejected_at_service_boundary() {
        let script = b"#!/bin/sh\necho 'podwayd 0.1.0'\n";
        assert!(matches!(
            validate_native_arm64_macos_macho_v1(script),
            Err(ServiceErrorV1::InvalidExecutableV1 { .. })
        ));
    }

    #[test]
    fn production_malformed_x86_fat_and_non_macos_binaries_are_rejected() {
        let mut x86 = native_arm64_macho();
        x86[4..8].copy_from_slice(&0x0100_0007u32.to_le_bytes());
        let mut no_macos_surface = native_arm64_macho();
        no_macos_surface[32..36].copy_from_slice(&1u32.to_le_bytes());
        for bytes in [
            &[][..],
            x86.as_slice(),
            &[0xca, 0xfe, 0xba, 0xbe][..],
            no_macos_surface.as_slice(),
        ] {
            assert!(matches!(
                validate_native_arm64_macos_macho_v1(bytes),
                Err(ServiceErrorV1::InvalidExecutableV1 { .. })
            ));
        }
    }

    struct ValidationFilesystemV1 {
        mutations: Arc<AtomicU64>,
    }

    impl ServiceFilesystemV1 for ValidationFilesystemV1 {
        fn exists(&self, _: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
            Ok(false)
        }

        fn is_executable(&self, _: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
            Ok(true)
        }

        fn create_directory(&self, _: &Path, _: u32) -> Result<(), ServiceFilesystemErrorV1> {
            self.mutations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn read_file_bounded(
            &self,
            _: &Path,
            _: usize,
        ) -> Result<Vec<u8>, ServiceFilesystemErrorV1> {
            Ok(vec![0xcf, 0xfa, 0xed, 0xfe])
        }

        fn write_atomically(
            &self,
            _: &Path,
            _: &[u8],
            _: u32,
        ) -> Result<(), ServiceFilesystemErrorV1> {
            self.mutations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn remove_file(&self, _: &Path) -> Result<(), ServiceFilesystemErrorV1> {
            self.mutations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn list_directory_bounded(
            &self,
            _: &Path,
            _: usize,
        ) -> Result<Vec<PathBuf>, ServiceFilesystemErrorV1> {
            Ok(Vec::new())
        }

        fn remove_directory(&self, _: &Path) -> Result<(), ServiceFilesystemErrorV1> {
            self.mutations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn rotate_file(&self, _: &Path, _: u64, _: u8) -> Result<(), ServiceFilesystemErrorV1> {
            self.mutations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct ValidationLaunchctlV1;

    impl LaunchctlRunnerV1 for ValidationLaunchctlV1 {
        fn run(&self, _: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
            panic!("invalid executable validation must precede launchctl")
        }
    }

    #[test]
    fn executable_validation_precedes_reconciliation_and_publication_mutations() {
        let mutations = Arc::new(AtomicU64::new(0));
        let runner = MacosServiceCommandRunnerV1::new(
            ValidationFilesystemV1 {
                mutations: Arc::clone(&mutations),
            },
            ValidationLaunchctlV1,
            FixedServiceClockV1::new(UnixMillis::new(0)),
            1,
        )
        .expect("non-root test user is valid");
        let paths = ServiceRuntimePathsV1::from_directories(
            "/private/tmp/LaunchAgents",
            "/private/tmp/Podway",
            "/private/tmp/Logs",
            "/private/tmp/podway-runtime",
        )
        .expect("test runtime paths are valid");
        let error = runner
            .install_or_update(
                ServiceOperationV1::Install,
                InstallSpecV1::new(
                    path("/private/tmp/podwayd"),
                    ServiceLabelV1::podwayd(),
                    paths,
                ),
                false,
            )
            .expect_err("truncated Mach-O must be rejected");
        assert!(matches!(error, ServiceErrorV1::InvalidExecutableV1 { .. }));
        assert_eq!(mutations.load(Ordering::SeqCst), 0);
    }
}
