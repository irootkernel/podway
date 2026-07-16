#![forbid(unsafe_code)]

//! Platform-service composition contracts for Podway.
//!
//! This crate owns neither daemon lifecycle mechanics nor persistent metadata I/O. It translates
//! direct service lifecycle requests into typed runner commands, keeping command execution,
//! clocks, and runtime paths injectable at the composition boundary.

use std::{
    collections::VecDeque,
    error::Error,
    fmt,
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use podway_core::UnixMillis;

pub const SERVICE_LABEL_V1: &str = "dev.podway.podwayd";
pub const SERVICE_METADATA_VERSION_V1: u16 = 1;

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

        Self::from_directories(
            home_directory.join("Library/LaunchAgents"),
            home_directory.join("Library/Application Support/Podway"),
            home_directory.join("Library/Logs/Podway"),
            temporary_directory.join(format!("podway-{user_id}")),
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

/// Selects a bounded, named service log location; it never requests log content.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LogQueryV1 {
    stream: ServiceLogStreamV1,
}

impl LogQueryV1 {
    pub const fn new(stream: ServiceLogStreamV1) -> Self {
        Self { stream }
    }

    pub const fn stream(self) -> ServiceLogStreamV1 {
        self.stream
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
            Self::Uninstall { .. } => ServiceOperationV1::Uninstall,
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
