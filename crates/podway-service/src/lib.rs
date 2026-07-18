#![forbid(unsafe_code)]

//! Platform-service composition contracts for Podway.
//!
//! This crate owns neither daemon lifecycle mechanics nor persistent metadata I/O. It translates
//! direct service lifecycle requests into typed runner commands, keeping command execution,
//! clocks, and runtime paths injectable at the composition boundary.

use podway_core::UnixMillis;
use std::{
    collections::VecDeque,
    error::Error,
    fmt,
    fs::{self, OpenOptions},
    io::{Read, Write},
    panic::{self, AssertUnwindSafe},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

pub const SERVICE_LABEL_V1: &str = "dev.podway.podwayd";
pub const SERVICE_METADATA_VERSION_V1: u16 = 1;
pub const SERVICE_LOG_MAX_BYTES_V1: u64 = 10 * 1024 * 1024;
pub const SERVICE_LOG_RETAINED_FILES_V1: u8 = 5;
/// A non-authoritative, content-free observation emitted by the service adapter.
///
/// Variants are stable categories only; paths, command arguments, process output, metadata, and
/// error messages must never be included in an observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceObservationV1 {
    ServiceOutcome,
    Error,
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
        let _ = options;
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
            socket_path: LocalPlatformPathV1::new(runtime_directory.join("podwayd.sock"))?,
        })
    }

    pub fn for_user(
        home_directory: impl AsRef<Path>,
        temporary_directory: impl AsRef<Path>,
        user_id: u32,
    ) -> Result<Self, ServicePathErrorV1> {
        if user_id == 0 {
            return Err(ServicePathErrorV1::RootUser);
        }

        let home_directory = home_directory.as_ref();
        let temporary_directory = temporary_directory.as_ref();
        validate_service_path(home_directory, "home_directory")?;
        validate_service_path(temporary_directory, "temporary_directory")?;

        let runtime_directory = temporary_directory.join(format!("podway-{user_id}"));
        // macOS Unix-domain socket paths are bounded; retain room for the socket filename.
        let runtime_directory = if runtime_directory.join("podwayd.sock").as_os_str().len() >= 104 {
            PathBuf::from(format!("/tmp/podway-{user_id}"))
        } else {
            runtime_directory
        };
        Self::from_directories(
            home_directory.join("Library/LaunchAgents"),
            home_directory.join("Library/Application Support/Podway"),
            home_directory.join("Library/Logs/Podway"),
            runtime_directory,
        )
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
}

/// Input used to install or update the fixed v1 LaunchAgent definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallSpecV1 {
    daemon_executable_path: LocalPlatformPathV1,
    label: ServiceLabelV1,
    runtime_paths: ServiceRuntimePathsV1,
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
        }
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
    installed_at: UnixMillis,
    updated_at: UnixMillis,
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
            installed_at,
            updated_at,
        })
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

    pub const fn installed_at(&self) -> UnixMillis {
        self.installed_at
    }

    pub const fn updated_at(&self) -> UnixMillis {
        self.updated_at
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
        match self.command_runner.run(command)? {
            ServiceCommandResultV1::Outcome(outcome) if accepts(&outcome) => Ok(outcome),
            ServiceCommandResultV1::Outcome(outcome) => Err(ServiceErrorV1::IoV1 {
                operation: Some(operation),
                message: format!(
                    "runner returned unexpected {} outcome",
                    outcome.kind().as_str()
                ),
            }),
            result => Err(ServiceErrorV1::IoV1 {
                operation: Some(operation),
                message: format!(
                    "runner returned {} result; expected {}",
                    result.kind().as_str(),
                    ServiceCommandResultKindV1::Outcome.as_str()
                ),
            }),
        }
    }
}

impl<R, C> ServiceManagerContractV1 for ServiceManagerV1<R, C>
where
    R: ServiceCommandRunnerV1,
    C: ServiceClockV1,
{
    fn install(&self, spec: InstallSpecV1) -> Result<ServiceOutcomeV1, ServiceErrorV1> {
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
        match self.command_runner.run(command)? {
            ServiceCommandResultV1::LogLocation(location)
                if location.path() == self.paths.log_path() =>
            {
                Ok(location)
            }
            ServiceCommandResultV1::LogLocation(_) => Err(ServiceErrorV1::LogUnavailableV1 {
                message: "runner returned a log location outside the configured service runtime"
                    .to_owned(),
            }),
            result => Err(ServiceErrorV1::IoV1 {
                operation: Some(operation),
                message: format!(
                    "runner returned {} result; expected {}",
                    result.kind().as_str(),
                    ServiceCommandResultKindV1::LogLocation.as_str()
                ),
            }),
        }
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
        match self.command_runner.run(command)? {
            ServiceCommandResultV1::Status(status) => Ok(status),
            result => Err(ServiceErrorV1::IoV1 {
                operation: Some(operation),
                message: format!(
                    "runner returned {} result; expected {}",
                    result.kind().as_str(),
                    ServiceCommandResultKindV1::Status.as_str()
                ),
            }),
        }
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
    RootUser,
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
            Self::RootUser => formatter.write_str("a per-user service cannot use the root user"),
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
}

impl fmt::Display for ServiceErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMetadataV1 { message } => {
                write!(formatter, "service metadata failure: {message}")
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
        }
    }
}

impl Error for ServiceErrorV1 {}

fn validate_service_path(path: &Path, field: &'static str) -> Result<(), ServicePathErrorV1> {
    if path.as_os_str().is_empty() {
        return Err(ServicePathErrorV1::Empty { field });
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
            Component::Normal(component) if component == ".podway" => {
                return Err(ServicePathErrorV1::WorkspaceLocal {
                    field,
                    path: path.to_path_buf(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}
/// Filesystem boundary used by the macOS LaunchAgent adapter. Implementations must perform
/// `write_atomically` as a same-directory replace so a partially written plist is never loaded.
pub trait ServiceFilesystemV1: Send + Sync {
    fn exists(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1>;
    fn is_executable(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1>;
    fn create_directory(&self, path: &Path, mode: u32) -> Result<(), ServiceFilesystemErrorV1>;
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, ServiceFilesystemErrorV1>;
    fn write_atomically(
        &self,
        path: &Path,
        contents: &[u8],
        mode: u32,
    ) -> Result<(), ServiceFilesystemErrorV1>;
    fn remove_file(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1>;
    fn remove_directory_contents(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1>;
    fn rotate_file(
        &self,
        path: &Path,
        maximum_bytes: u64,
        retained_files: u8,
    ) -> Result<(), ServiceFilesystemErrorV1>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceFilesystemErrorV1 {
    pub permission_denied: bool,
    pub message: String,
}

impl ServiceFilesystemErrorV1 {
    pub fn permission(message: impl Into<String>) -> Self {
        Self {
            permission_denied: true,
            message: message.into(),
        }
    }
    pub fn other(message: impl Into<String>) -> Self {
        Self {
            permission_denied: false,
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
static DURABILITY_FAILPOINT_V1: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

impl StdServiceFilesystemV1 {
    #[cfg(any(test, debug_assertions))]
    pub fn inject_durability_failpoint_for_testing(failpoint: DurabilityFailpointV1) {
        DURABILITY_FAILPOINT_V1.store(failpoint as u8 + 1, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(any(test, debug_assertions))]
    fn fail_at_durability_boundary(failpoint: DurabilityFailpointV1) {
        if DURABILITY_FAILPOINT_V1.load(std::sync::atomic::Ordering::SeqCst) == failpoint as u8 + 1
        {
            std::process::exit(86);
        }
    }

    fn error(error: std::io::Error) -> ServiceFilesystemErrorV1 {
        ServiceFilesystemErrorV1 {
            permission_denied: error.kind() == std::io::ErrorKind::PermissionDenied,
            message: error.to_string(),
        }
    }

    fn set_mode(path: &Path, mode: u32) -> Result<(), ServiceFilesystemErrorV1> {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(Self::error)
    }
}

impl ServiceFilesystemV1 for StdServiceFilesystemV1 {
    fn exists(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
        Ok(path.exists())
    }

    fn is_executable(&self, path: &Path) -> Result<bool, ServiceFilesystemErrorV1> {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(path) {
            Ok(metadata) => Ok(metadata.is_file() && metadata.permissions().mode() & 0o111 != 0),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(Self::error(error)),
        }
    }

    fn create_directory(&self, path: &Path, mode: u32) -> Result<(), ServiceFilesystemErrorV1> {
        fs::create_dir_all(path).map_err(Self::error)?;
        Self::set_mode(path, mode)
    }

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, ServiceFilesystemErrorV1> {
        let mut file = fs::File::open(path).map_err(Self::error)?;
        let mut contents = Vec::new();
        file.read_to_end(&mut contents).map_err(Self::error)?;
        Ok(contents)
    }

    fn write_atomically(
        &self,
        path: &Path,
        contents: &[u8],
        mode: u32,
    ) -> Result<(), ServiceFilesystemErrorV1> {
        let parent = path.parent().ok_or_else(|| {
            ServiceFilesystemErrorV1::other("service file has no parent directory")
        })?;
        let file_name = path
            .file_name()
            .ok_or_else(|| ServiceFilesystemErrorV1::other("service file has no file name"))?;
        for attempt in 0..32_u8 {
            let temporary =
                parent.join(format!(".{}.{}.tmp", file_name.to_string_lossy(), attempt));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(mut file) => {
                    let result = (|| {
                        file.write_all(contents).map_err(Self::error)?;
                        #[cfg(any(test, debug_assertions))]
                        Self::fail_at_durability_boundary(
                            DurabilityFailpointV1::AfterTemporaryWrite,
                        );
                        file.sync_all().map_err(Self::error)?;
                        Self::set_mode(&temporary, mode)?;
                        #[cfg(any(test, debug_assertions))]
                        Self::fail_at_durability_boundary(
                            DurabilityFailpointV1::AfterFileSyncAndMode,
                        );
                        #[cfg(any(test, debug_assertions))]
                        Self::fail_at_durability_boundary(DurabilityFailpointV1::BeforeRename);
                        fs::rename(&temporary, path).map_err(Self::error)?;
                        #[cfg(any(test, debug_assertions))]
                        Self::fail_at_durability_boundary(DurabilityFailpointV1::AfterRename);
                        fs::File::open(parent)
                            .and_then(|directory| directory.sync_all())
                            .map_err(Self::error)?;
                        #[cfg(any(test, debug_assertions))]
                        Self::fail_at_durability_boundary(
                            DurabilityFailpointV1::AfterParentDirectorySync,
                        );
                        Ok(())
                    })();
                    if result.is_err() {
                        let _ = fs::remove_file(&temporary);
                    }
                    return result;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(Self::error(error)),
            }
        }
        Err(ServiceFilesystemErrorV1::other(
            "could not allocate an atomic service-file temporary path",
        ))
    }

    fn remove_file(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Self::error(error)),
        }
    }

    fn remove_directory_contents(&self, path: &Path) -> Result<(), ServiceFilesystemErrorV1> {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(Self::error(error)),
        };
        for entry in entries {
            let entry = entry.map_err(Self::error)?;
            let file_type = entry.file_type().map_err(Self::error)?;
            if file_type.is_dir() {
                fs::remove_dir_all(entry.path()).map_err(Self::error)?;
            } else {
                fs::remove_file(entry.path()).map_err(Self::error)?;
            }
        }
        Ok(())
    }
    fn rotate_file(
        &self,
        path: &Path,
        maximum_bytes: u64,
        retained_files: u8,
    ) -> Result<(), ServiceFilesystemErrorV1> {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(Self::error(error)),
        };
        if metadata.len() <= maximum_bytes {
            return Ok(());
        }
        if retained_files == 0 {
            return fs::remove_file(path).map_err(Self::error);
        }
        let rotated_path = |index: u8| {
            let mut value = path.as_os_str().to_os_string();
            value.push(format!(".{index}"));
            PathBuf::from(value)
        };
        let oldest = rotated_path(retained_files);
        match fs::remove_file(&oldest) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Self::error(error)),
        }
        for index in (1..retained_files).rev() {
            let source = rotated_path(index);
            if source.exists() {
                fs::rename(source, rotated_path(index + 1)).map_err(Self::error)?;
            }
        }
        fs::rename(path, rotated_path(1)).map_err(Self::error)?;
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(Self::error)?;
        Self::set_mode(path, 0o600)
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
}
/// The process-backed `launchctl` adapter used by CLI composition on macOS.
#[derive(Clone, Debug)]
pub struct SystemLaunchctlRunnerV1 {
    executable: PathBuf,
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
        }
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }
}

impl LaunchctlRunnerV1 for SystemLaunchctlRunnerV1 {
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        let output = Command::new(&self.executable)
            .args(arguments)
            .output()
            .map_err(|error| ServiceErrorV1::IoV1 {
                operation: None,
                message: format!("could not execute {}: {error}", self.executable.display()),
            })?;
        Ok(LaunchctlOutputV1 {
            exit_status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Production LaunchAgent command runner. It owns only the fixed, per-user service files supplied
/// in `ServiceRuntimePathsV1`; no workspace path is accepted or traversed.
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
        paths: &ServiceRuntimePathsV1,
    ) -> Result<Option<ServiceInstallMetadataV1>, ServiceErrorV1> {
        let path = paths.metadata_index_path().as_path();
        if !self
            .filesystem
            .exists(path)
            .map_err(|e| self.fs_error(ServiceOperationV1::Status, path, e))?
        {
            return Ok(None);
        }
        let bytes = self
            .filesystem
            .read_file(path)
            .map_err(|e| self.fs_error(ServiceOperationV1::Status, path, e))?;
        parse_metadata_v1(&bytes).map(Some)
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

    fn bootstrap(
        &self,
        op: ServiceOperationV1,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<(), ServiceErrorV1> {
        self.launch(
            op,
            vec![
                "bootstrap".to_owned(),
                self.domain(),
                paths.launch_agent_path().as_path().display().to_string(),
            ],
        )?;
        Ok(())
    }

    fn bootout(&self, op: ServiceOperationV1) -> Result<(), ServiceErrorV1> {
        self.launch(op, vec!["bootout".to_owned(), self.loaded_target()])?;
        Ok(())
    }
    fn remove_stale_socket(
        &self,
        op: ServiceOperationV1,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<(), ServiceErrorV1> {
        let socket = paths.socket_path().as_path();
        if self
            .filesystem
            .exists(socket)
            .map_err(|error| self.fs_error(op, socket, error))?
        {
            self.filesystem
                .remove_file(socket)
                .map_err(|error| self.fs_error(op, socket, error))?;
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
            return Err(ServiceErrorV1::InvalidMetadataV1 {
                message: format!("daemon binary is not executable: {}", binary.display()),
            });
        }
        let existing = self.metadata(paths)?;
        if update_only && existing.is_none() {
            return Ok(ServiceCommandResultV1::Outcome(
                ServiceOutcomeV1::NotInstalledV1(ServiceNotInstalledV1::new(self.clock.now())),
            ));
        }
        let plist = launch_agent_plist_v1(binary, paths.log_path().as_path());
        let plist_path = paths.launch_agent_path().as_path();
        let exists = self
            .filesystem
            .exists(plist_path)
            .map_err(|e| self.fs_error(op, plist_path, e))?;
        let unchanged = existing
            .as_ref()
            .is_some_and(|m| m.daemon_binary() == binary)
            && exists
            && self
                .filesystem
                .read_file(plist_path)
                .map_err(|e| self.fs_error(op, plist_path, e))?
                == plist;
        if unchanged {
            return Ok(ServiceCommandResultV1::Outcome(
                ServiceOutcomeV1::AlreadyInDesiredStateV1(ServiceAlreadyV1::new(
                    self.clock.now(),
                    existing.expect("metadata checked"),
                )),
            ));
        }
        self.ensure_directories(op, paths)?;
        if exists {
            self.bootout(op)?;
            self.remove_stale_socket(op, paths)?;
        }
        self.filesystem
            .write_atomically(plist_path, &plist, 0o600)
            .map_err(|e| self.fs_error(op, plist_path, e))?;
        self.observe(ServiceObservationV1::AtomicPlistPublished);
        self.rotate_log(op, paths)?;
        self.bootstrap(op, paths)?;
        let now = self.clock.now();
        let metadata = ServiceInstallMetadataV1::new(
            binary,
            existing
                .as_ref()
                .map_or(now, ServiceInstallMetadataV1::installed_at),
            now,
        )
        .map_err(|e| ServiceErrorV1::InvalidMetadataV1 {
            message: e.to_string(),
        })?;
        let metadata_path = paths.metadata_index_path().as_path();
        self.filesystem
            .write_atomically(metadata_path, metadata_json_v1(&metadata).as_bytes(), 0o600)
            .map_err(|e| self.fs_error(op, metadata_path, e))?;
        self.observe(ServiceObservationV1::AtomicMetadataPublished);
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
        if !installed && !has_metadata && !has_runtime_file && !options.purge_logs() {
            return Ok(ServiceCommandResultV1::Outcome(
                ServiceOutcomeV1::NotInstalledV1(ServiceNotInstalledV1::new(self.clock.now())),
            ));
        }
        if installed {
            self.bootout(ServiceOperationV1::Uninstall)?;
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
        if options.purge_logs() {
            let log_directory = paths
                .log_path()
                .as_path()
                .parent()
                .expect("validated service log path has a parent");
            self.filesystem
                .remove_directory_contents(log_directory)
                .map_err(|e| self.fs_error(ServiceOperationV1::Uninstall, log_directory, e))?;
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
        let result = (|| match command {
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
                    return Ok(ServiceCommandResultV1::Outcome(
                        ServiceOutcomeV1::NotInstalledV1(ServiceNotInstalledV1::new(
                            self.clock.now(),
                        )),
                    ));
                }
                self.rotate_log(ServiceOperationV1::Start, &paths)?;
                self.bootstrap(ServiceOperationV1::Start, &paths)?;
                Ok(ServiceCommandResultV1::Outcome(
                    ServiceOutcomeV1::ChangedV1(ServiceChangedV1::new(
                        self.clock.now(),
                        self.metadata(&paths)?,
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
                    return Ok(ServiceCommandResultV1::Outcome(
                        ServiceOutcomeV1::NotInstalledV1(ServiceNotInstalledV1::new(
                            self.clock.now(),
                        )),
                    ));
                }
                self.bootout(ServiceOperationV1::Stop)?;
                Ok(ServiceCommandResultV1::Outcome(
                    ServiceOutcomeV1::StoppedV1(ServiceStoppedV1::new(
                        self.clock.now(),
                        self.metadata(&paths)?,
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
                    return Ok(ServiceCommandResultV1::Outcome(
                        ServiceOutcomeV1::NotInstalledV1(ServiceNotInstalledV1::new(
                            self.clock.now(),
                        )),
                    ));
                }
                self.bootout(ServiceOperationV1::Restart)?;
                self.remove_stale_socket(ServiceOperationV1::Restart, &paths)?;
                self.rotate_log(ServiceOperationV1::Restart, &paths)?;
                self.bootstrap(ServiceOperationV1::Restart, &paths)?;
                Ok(ServiceCommandResultV1::Outcome(
                    ServiceOutcomeV1::ChangedV1(ServiceChangedV1::new(
                        self.clock.now(),
                        self.metadata(&paths)?,
                    )),
                ))
            }
            ServiceCommandV1::Status { paths, .. } => {
                let metadata = self.metadata(&paths)?;
                if metadata.is_none() {
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
                    LaunchctlOutputV1 {
                        exit_status: 0,
                        stdout,
                        ..
                    } => Ok(ServiceCommandResultV1::Status(ServiceStatusV1::RunningV1(
                        ServiceRunningV1::new(self.clock.now(), parse_pid(&stdout), metadata),
                    ))),
                    _ => Ok(ServiceCommandResultV1::Status(ServiceStatusV1::StoppedV1(
                        ServiceStoppedV1::new(self.clock.now(), metadata),
                    ))),
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
        })();
        self.observe(if result.is_ok() {
            ServiceObservationV1::ServiceOutcome
        } else {
            ServiceObservationV1::Error
        });
        result
    }
}

/// Generates the exact v1 LaunchAgent template with XML-sensitive values escaped.
pub fn launch_agent_plist_v1(binary: &Path, log_path: &Path) -> Vec<u8> {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{SERVICE_LABEL_V1}</string>\n\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>--service</string>\n  </array>\n\n  <key>RunAtLoad</key>\n  <true/>\n\n  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n\n  <key>ThrottleInterval</key>\n  <integer>5</integer>\n\n  <key>ProcessType</key>\n  <string>Background</string>\n\n  <key>StandardOutPath</key>\n  <string>{}</string>\n\n  <key>StandardErrorPath</key>\n  <string>{}</string>\n</dict>\n</plist>\n", xml_escape(&binary.display().to_string()), xml_escape(&log_path.display().to_string()), xml_escape(&log_path.display().to_string())).into_bytes()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
fn unescape_json(value: &str) -> String {
    value.replace("\\\"", "\"").replace("\\\\", "\\")
}

fn metadata_json_v1(metadata: &ServiceInstallMetadataV1) -> String {
    format!(
        "{{\"version\":{},\"label\":\"{}\",\"daemon_binary\":\"{}\",\"installed_at\":{},\"updated_at\":{}}}\n",
        metadata.version(),
        SERVICE_LABEL_V1,
        json_escape(&metadata.daemon_binary().display().to_string()),
        metadata.installed_at().get(),
        metadata.updated_at().get()
    )
}

fn parse_metadata_v1(bytes: &[u8]) -> Result<ServiceInstallMetadataV1, ServiceErrorV1> {
    let text = std::str::from_utf8(bytes).map_err(|_| ServiceErrorV1::InvalidMetadataV1 {
        message: "metadata is not UTF-8 JSON".to_owned(),
    })?;
    let field = |name: &str| {
        json_field(text, name).ok_or_else(|| ServiceErrorV1::InvalidMetadataV1 {
            message: format!("metadata missing {name}"),
        })
    };
    let version =
        field("version")?
            .parse::<u16>()
            .map_err(|_| ServiceErrorV1::InvalidMetadataV1 {
                message: "metadata version is invalid".to_owned(),
            })?;
    if version != SERVICE_METADATA_VERSION_V1 || field("label")? != SERVICE_LABEL_V1 {
        return Err(ServiceErrorV1::InvalidMetadataV1 {
            message: "metadata version or label is unsupported".to_owned(),
        });
    }
    let installed =
        field("installed_at")?
            .parse()
            .map_err(|_| ServiceErrorV1::InvalidMetadataV1 {
                message: "metadata installed_at is invalid".to_owned(),
            })?;
    let updated = field("updated_at")?
        .parse()
        .map_err(|_| ServiceErrorV1::InvalidMetadataV1 {
            message: "metadata updated_at is invalid".to_owned(),
        })?;
    ServiceInstallMetadataV1::new(
        unescape_json(field("daemon_binary")?),
        UnixMillis::new(installed),
        UnixMillis::new(updated),
    )
    .map_err(|e| ServiceErrorV1::InvalidMetadataV1 {
        message: e.to_string(),
    })
}

fn json_field<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("\"{name}\":");
    let tail = text.split_once(&marker)?.1.trim_start();
    if let Some(value) = tail.strip_prefix('"') {
        Some(value.split_once('"')?.0)
    } else {
        Some(tail.split([',', '}']).next()?.trim())
    }
}

fn parse_pid(output: &str) -> Option<u32> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("pid = ")?.parse().ok())
}
