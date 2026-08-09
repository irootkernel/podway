#![forbid(unsafe_code)]

//! Podway daemon composition and Phase 4 runtime foundations.
//!
//! The crate owns the per-user local endpoint, peer validation, bounded blocking budget, and
//! durable-identity scheduler lifecycle. Protocol, Store, Git, config, preset, and service
//! semantics remain in their owning crates.

pub mod blocking;
mod development_v2;
pub mod dispatch;
pub mod endpoint;
pub mod execution;
pub mod native_execution;
pub mod observability;
pub mod peer;
pub mod production;
pub mod read_service;
pub mod registry;
pub mod runtime;
pub mod runtime_workspace;
pub mod scheduler;
pub mod server;
pub mod v2_read_service;
pub mod worker;
pub mod workspace;
use std::{
    error::Error,
    fmt,
    num::{NonZeroU64, NonZeroUsize},
};

use podway_core::WorkspaceId;
use podway_git::{DurableWorktreeIdentityV1, GitResolverContractV1};
use podway_protocol::OperationV1;
use podway_store::StoreContractV1;

pub use observability::{
    ClockErrorV1, ClockV1, EventOperationV1, EventOutcomeV1, EventRecordV1, LogSinkV1,
    ObservabilityCountersV1, ObservabilityFinalizationV1, ObservabilityV1, RotatingFileSinkV1,
    SystemClockV1,
};
pub use podway_config::{WORKSPACE_SCHEMA_V1, WorkspaceConfigV1};
pub use podway_presets::{EmbeddedPreset, list as embedded_presets_v1};
pub use podway_service::ServiceManagerV1;

/// The daemon-wide blocking-work capacity. Protocol frame and JSON limits remain owned by
/// `podway-protocol`, while workspace queue limits remain owned by `podway-config`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DaemonLimitsV1 {
    max_concurrent_blocking_operations: NonZeroUsize,
}

impl DaemonLimitsV1 {
    pub fn new(
        max_concurrent_blocking_operations: usize,
    ) -> Result<Self, DaemonCompositionErrorV1> {
        let max_concurrent_blocking_operations = NonZeroUsize::new(
            max_concurrent_blocking_operations,
        )
        .ok_or(DaemonCompositionErrorV1::InvalidLimit {
            field: "max_concurrent_blocking_operations",
            value: max_concurrent_blocking_operations,
        })?;

        Ok(Self {
            max_concurrent_blocking_operations,
        })
    }

    pub const fn max_concurrent_blocking_operations(self) -> NonZeroUsize {
        self.max_concurrent_blocking_operations
    }
}

/// The route class inferred from the protocol operation before command-specific routing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteClassificationV1 {
    Read,
    MutationAdmission,
    Control,
    BootstrapAdmission,
}

impl RouteClassificationV1 {
    pub const fn from_operation(operation: OperationV1) -> Self {
        match operation {
            OperationV1::Query => Self::Read,
            OperationV1::Mutate => Self::MutationAdmission,
            OperationV1::Control => Self::Control,
            OperationV1::Bootstrap => Self::BootstrapAdmission,
        }
    }

    pub const fn requires_durable_admission(self) -> bool {
        matches!(self, Self::MutationAdmission | Self::BootstrapAdmission)
    }
}

/// A nonzero generation used to prevent a retiring scheduler from removing a replacement.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SchedulerGenerationV1(NonZeroU64);

impl SchedulerGenerationV1 {
    pub const fn initial() -> Self {
        Self(NonZeroU64::MIN)
    }

    pub fn new(value: u64) -> Result<Self, DaemonCompositionErrorV1> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(DaemonCompositionErrorV1::InvalidSchedulerGeneration { value })
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    pub fn next(self) -> Result<Self, DaemonCompositionErrorV1> {
        let value = self
            .get()
            .checked_add(1)
            .ok_or(DaemonCompositionErrorV1::SchedulerGenerationExhausted { generation: self })?;
        Self::new(value)
    }
}

/// The retirement phase for one identity-keyed scheduler generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerRetirementStateV1 {
    Active,
    Retiring,
    Retired,
}

/// Scheduler registry state keyed by daemon-validated durable worktree identity rather than path.
pub struct WorkspaceSchedulerStateV1 {
    identity: DurableWorktreeIdentityV1,
    generation: SchedulerGenerationV1,
    retirement: SchedulerRetirementStateV1,
}

impl WorkspaceSchedulerStateV1 {
    pub fn new(
        identity: DurableWorktreeIdentityV1,
        generation: SchedulerGenerationV1,
        retirement: SchedulerRetirementStateV1,
    ) -> Self {
        Self {
            identity,
            generation,
            retirement,
        }
    }

    pub fn identity(&self) -> &DurableWorktreeIdentityV1 {
        &self.identity
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        self.identity.workspace_id()
    }

    pub const fn generation(&self) -> SchedulerGenerationV1 {
        self.generation
    }

    pub const fn retirement(&self) -> SchedulerRetirementStateV1 {
        self.retirement
    }
}

/// Injected dependencies for a future daemon runtime. Construction only binds components; it does
/// not call them or start daemon behavior.
pub struct DaemonComponentsV1<Store, Resolver, CommandRunner, Clock>
where
    Store: StoreContractV1,
    Resolver: GitResolverContractV1,
{
    store: Store,
    git_resolver: Resolver,
    service_manager: ServiceManagerV1<CommandRunner, Clock>,
}

impl<Store, Resolver, CommandRunner, Clock>
    DaemonComponentsV1<Store, Resolver, CommandRunner, Clock>
where
    Store: StoreContractV1,
    Resolver: GitResolverContractV1,
{
    pub fn new(
        store: Store,
        git_resolver: Resolver,
        service_manager: ServiceManagerV1<CommandRunner, Clock>,
    ) -> Self {
        Self {
            store,
            git_resolver,
            service_manager,
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn git_resolver(&self) -> &Resolver {
        &self.git_resolver
    }

    pub fn service_manager(&self) -> &ServiceManagerV1<CommandRunner, Clock> {
        &self.service_manager
    }

    pub fn into_parts(self) -> (Store, Resolver, ServiceManagerV1<CommandRunner, Clock>) {
        (self.store, self.git_resolver, self.service_manager)
    }
}

/// Failures raised while validating a Phase 0B daemon composition contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DaemonCompositionErrorV1 {
    InvalidLimit { field: &'static str, value: usize },
    InvalidSchedulerGeneration { value: u64 },
    SchedulerGenerationExhausted { generation: SchedulerGenerationV1 },
    SchedulerRetiring { generation: SchedulerGenerationV1 },
}

impl fmt::Display for DaemonCompositionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimit { field, value } => {
                write!(formatter, "{field} must be positive (received {value})")
            }
            Self::InvalidSchedulerGeneration { value } => {
                write!(
                    formatter,
                    "scheduler generation must be positive (received {value})"
                )
            }
            Self::SchedulerGenerationExhausted { generation } => {
                write!(
                    formatter,
                    "scheduler generation {} cannot advance",
                    generation.get()
                )
            }
            Self::SchedulerRetiring { generation } => {
                write!(
                    formatter,
                    "scheduler generation {} remains unavailable while retirement is incomplete",
                    generation.get()
                )
            }
        }
    }
}

impl Error for DaemonCompositionErrorV1 {}
