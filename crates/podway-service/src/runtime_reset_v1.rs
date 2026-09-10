//! Read-only planning for ordinary account-runtime retirement.

mod apply;
mod document;
mod filesystem;
mod locks;

pub use apply::{RuntimeResetApplyV1, RuntimeResetOperatorV1, RuntimeResetServiceLifecycleV1};
pub use document::{
    RuntimeResetErrorV1, RuntimeResetExclusionV1, RuntimeResetModeOutcomeStateV1,
    RuntimeResetModeOutcomeV1, RuntimeResetOperationV1, RuntimeResetPathV1,
    RuntimeResetPlanStatusV1, RuntimeResetPlanV1, RuntimeResetProcessV1, RuntimeResetReasonV1,
    RuntimeResetResourceClassV1, RuntimeResetResourceOutcomeStateV1, RuntimeResetResourceOutcomeV1,
    RuntimeResetResourceV1, RuntimeResetResultStatusV1, RuntimeResetResultV1,
    RuntimeResetSelectionV1, RuntimeResetTargetStateV1, RuntimeResetTargetV1,
};
pub use locks::{RuntimeResetLockV1, RuntimeResetParticipantV1, RuntimeResetRecordViewV1};

use std::ffi::OsStr;

use nix::fcntl::{Flock, FlockArg};
use podway_core::{RuntimeModeV1, UnixMillis};

use crate::{
    LaunchctlRunnerV1, PodwayHomeV1, SERVICE_LABEL_V1, ServiceRuntimePathsV1,
    launchctl_loaded_state_v1, launchctl_not_loaded_v1,
};
use document::{NamespaceBinding, ResetRecord, ResetToken, ServiceBinding};
use filesystem::{Directory, FileKind, metadata_digest};

pub const MAX_RUNTIME_RESET_MODES_V1: usize = 64;
pub const MAX_RUNTIME_RESET_MODE_ENTRIES_V1: usize = 256;
pub const MAX_RUNTIME_RESET_RESOURCES_V1: usize = 4096;
pub const MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1: usize = 512 * 1024;
pub const MAX_RUNTIME_RESET_TOKEN_BYTES_V1: usize = 256 * 1024;
pub const RUNTIME_RESET_TOKEN_LIFETIME_MS_V1: u64 = 600_000;

/// A validated read-only peer observation supplied by the protocol adapter.
#[derive(Clone, Debug)]
pub enum RuntimeResetPeerV1 {
    Offline,
    Unsupported,
    Live {
        process: RuntimeResetProcessV1,
        busy_reason: Option<RuntimeResetReasonV1>,
    },
}

/// The service owner never connects through protocol or opens a worktree Store.
pub trait RuntimeResetInspectorV1 {
    fn inspect(
        &mut self,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<RuntimeResetPeerV1, RuntimeResetErrorV1>;
}

/// Inventory and token revalidation use existing descriptors and never create state.
pub struct RuntimeResetPlannerV1<L, I> {
    home: PodwayHomeV1,
    launchctl: L,
    inspector: I,
}

impl<L: LaunchctlRunnerV1, I: RuntimeResetInspectorV1> RuntimeResetPlannerV1<L, I> {
    pub fn new(home: PodwayHomeV1, launchctl: L, inspector: I) -> Self {
        Self {
            home,
            launchctl,
            inspector,
        }
    }

    pub fn plan(
        &mut self,
        selection: RuntimeResetSelectionV1,
        now: UnixMillis,
    ) -> Result<RuntimeResetPlanV1, RuntimeResetErrorV1> {
        let mut plan = RuntimeResetPlanV1::empty(&self.home, selection.clone())?;
        let account = Directory::account(&self.home)?;
        let root = account.child_optional(OsStr::new(".podway"), true)?;
        if let Some(root) = &root
            && let Some(operation) = self.recovery_operation(root, &mut plan)?
        {
            plan.status = RuntimeResetPlanStatusV1::RecoveryRequired;
            plan.recovery_operation = Some(operation);
            plan.check_bounds()?;
            return Ok(plan);
        }
        let modes = match &selection {
            RuntimeResetSelectionV1::Mode { mode } => vec![mode.clone()],
            RuntimeResetSelectionV1::AllModes => self.inventory(root.as_ref(), &mut plan)?,
        };
        let mut bindings = Vec::with_capacity(modes.len());
        for mode in &modes {
            let paths = ServiceRuntimePathsV1::for_account_home_mode(
                self.home.account_home(),
                mode.clone(),
                self.home.user_id(),
            )
            .map_err(|_| RuntimeResetErrorV1::unsafe_path().in_mode(mode))?;
            match self.target(root.as_ref(), &paths, &mut plan) {
                Ok(Some(binding)) => bindings.push(binding),
                Ok(None) => {}
                Err(error) if error.reason == RuntimeResetReasonV1::BoundExceeded => {
                    return Err(error);
                }
                Err(error) => plan.targets.push(RuntimeResetTargetV1 {
                    mode: mode.clone(),
                    root: RuntimeResetPathV1::new(
                        paths.podway_home().expect("ordinary root").as_path(),
                    )?,
                    state: RuntimeResetTargetStateV1::Unsafe,
                    reason: Some(error.reason),
                    resources: Vec::new(),
                }),
            }
            plan.check_bounds()?;
        }
        if plan.targets.iter().any(|target| target.state.blocks()) {
            plan.status = RuntimeResetPlanStatusV1::Blocked;
        } else if plan
            .targets
            .iter()
            .any(|target| target.state != RuntimeResetTargetStateV1::Absent)
        {
            plan.check_apply_preserved_bounds(&self.home)?;
            let prospective_resources = plan.prospective_apply_resources(&bindings)?;
            let root = root.as_ref().ok_or_else(RuntimeResetErrorV1::unsafe_path)?;
            let expires_at_ms = now
                .get()
                .checked_add(RUNTIME_RESET_TOKEN_LIFETIME_MS_V1)
                .filter(|value| *value <= 253_402_300_799_999)
                .ok_or_else(RuntimeResetErrorV1::limit)?;
            let token = ResetToken {
                schema: "podway.runtime-reset-token/v1".to_owned(),
                caller_uid: self.home.user_id(),
                account_root: plan.account_root.clone(),
                account_identity: root.identity()?,
                selection,
                mode_inventory: if matches!(plan.selection, RuntimeResetSelectionV1::AllModes) {
                    modes
                } else {
                    Vec::new()
                },
                targets: bindings,
                created_at_ms: now.get(),
                expires_at_ms,
            };
            token.check_prospective_record_bounds(&prospective_resources, &self.home)?;
            plan.plan_token = Some(token.encode()?);
            plan.expires_at_ms = Some(expires_at_ms);
            plan.status = RuntimeResetPlanStatusV1::Ready;
        }
        plan.check_bounds()?;
        Ok(plan)
    }

    /// Rechecks a token before commit. Durable recovery is a separate coordinator operation.
    pub fn validate_precommit_token(
        &mut self,
        encoded: &str,
        selection: &RuntimeResetSelectionV1,
        now: UnixMillis,
    ) -> Result<(), RuntimeResetErrorV1> {
        let mut original = ResetToken::decode(encoded)?;
        if original.selection != *selection {
            return Err(RuntimeResetErrorV1::stale(
                RuntimeResetReasonV1::SelectionChanged,
            ));
        }
        if now.get() < original.created_at_ms || now.get() >= original.expires_at_ms {
            return Err(RuntimeResetErrorV1::stale(RuntimeResetReasonV1::Expired));
        }
        let current = self.plan(selection.clone(), now)?;
        let current = match current.plan_token.as_deref() {
            Some(token) => token,
            None => {
                if let Some(target) = current.targets.iter().find(|target| target.state.blocks()) {
                    let reason = target
                        .reason
                        .unwrap_or(RuntimeResetReasonV1::IdentityChanged);
                    let error = match target.state {
                        RuntimeResetTargetStateV1::Busy => RuntimeResetErrorV1::busy_reason(reason),
                        RuntimeResetTargetStateV1::Unsupported => {
                            RuntimeResetErrorV1::unsupported_reason(reason)
                        }
                        RuntimeResetTargetStateV1::Unsafe => {
                            RuntimeResetErrorV1::unsafe_reason(reason)
                        }
                        _ => unreachable!("only blocking target states are selected"),
                    };
                    return Err(error.in_mode(&target.mode));
                }
                if current.recovery_operation.is_some() {
                    return Err(RuntimeResetErrorV1::in_progress());
                }
                return Err(RuntimeResetErrorV1::stale(
                    RuntimeResetReasonV1::IdentityChanged,
                ));
            }
        };
        let mut current = ResetToken::decode(current)?;
        current.created_at_ms = original.created_at_ms;
        current.expires_at_ms = original.expires_at_ms;
        original.normalize_directory_links();
        current.normalize_directory_links();
        if current != original {
            return Err(RuntimeResetErrorV1::stale(
                RuntimeResetReasonV1::IdentityChanged,
            ));
        }
        Ok(())
    }

    fn inventory(
        &self,
        root: Option<&Directory>,
        plan: &mut RuntimeResetPlanV1,
    ) -> Result<Vec<RuntimeModeV1>, RuntimeResetErrorV1> {
        let mut modes = vec![RuntimeModeV1::production()];
        let Some(root) = root else {
            return Ok(modes);
        };
        let Some(directory) = root.child_optional(OsStr::new("modes"), true)? else {
            return Ok(modes);
        };
        for name in directory.entries(MAX_RUNTIME_RESET_MODE_ENTRIES_V1)? {
            let mode = name
                .to_str()
                .and_then(|name| RuntimeModeV1::new(name).ok())
                .filter(|mode| !mode.is_production())
                .ok_or_else(|| {
                    RuntimeResetErrorV1::unsafe_reason(RuntimeResetReasonV1::UnexpectedEntry)
                })?;
            if let Ok(Some(namespace)) = directory.child_optional(&name, true)
                && namespace.managed(&mode)?
            {
                plan.excluded
                    .push(RuntimeResetExclusionV1::managed(namespace.path())?);
                continue;
            }
            modes.push(mode);
            if modes.len() > MAX_RUNTIME_RESET_MODES_V1 {
                return Err(RuntimeResetErrorV1::limit());
            }
        }
        modes[1..].sort_by(|left, right| left.as_str().as_bytes().cmp(right.as_str().as_bytes()));
        Ok(modes)
    }

    fn recovery_operation(
        &self,
        root: &Directory,
        plan: &mut RuntimeResetPlanV1,
    ) -> Result<Option<RuntimeResetOperationV1>, RuntimeResetErrorV1> {
        let Some(maintenance) = root.child_optional(OsStr::new("maintenance"), true)? else {
            return Ok(None);
        };
        for name in [
            "runtime-reset.lock",
            "runtime-start.lock",
            "runtime-reset-commit.lock",
        ] {
            if maintenance.regular_optional(OsStr::new(name))?.is_some() {
                plan.preserved.push(RuntimeResetResourceV1::new(
                    RuntimeResetResourceClassV1::ControlResidue,
                    &maintenance.path().join(name),
                )?);
            }
        }
        let Some(bytes) = maintenance.read_optional(
            OsStr::new("runtime-reset.json"),
            MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1,
        )?
        else {
            return Ok(None);
        };
        plan.preserved.push(RuntimeResetResourceV1::new(
            RuntimeResetResourceClassV1::ControlResidue,
            &maintenance.path().join("runtime-reset.json"),
        )?);
        ResetRecord::operation(&bytes, &self.home)
    }

    fn target(
        &mut self,
        root: Option<&Directory>,
        paths: &ServiceRuntimePathsV1,
        plan: &mut RuntimeResetPlanV1,
    ) -> Result<Option<NamespaceBinding>, RuntimeResetErrorV1> {
        let mode = paths.mode();
        let namespace = if mode.is_production() {
            root.map(Directory::duplicate).transpose()?
        } else if let Some(root) = root {
            root.child_optional(OsStr::new("modes"), true)?
                .map(|modes| modes.child_optional(OsStr::new(mode.as_str()), true))
                .transpose()?
                .flatten()
        } else {
            None
        };
        if let Some(namespace) = &namespace
            && namespace.managed(mode)?
        {
            plan.excluded
                .push(RuntimeResetExclusionV1::managed(namespace.path())?);
            return Ok(None);
        }
        let namespace_path = paths.podway_home().expect("ordinary root").as_path();
        let mut target = RuntimeResetTargetV1 {
            mode: mode.clone(),
            root: RuntimeResetPathV1::new(namespace_path)?,
            state: RuntimeResetTargetStateV1::Absent,
            reason: None,
            resources: Vec::new(),
        };
        let mut binding = NamespaceBinding::empty(mode.clone(), target.root.clone());
        if mode.is_production() {
            binding.service = self.production_service(paths)?;
            if binding.service.installed {
                target.resources.push(RuntimeResetResourceV1::new(
                    RuntimeResetResourceClassV1::ServicePlist,
                    paths.launch_agent_path().as_path(),
                )?);
            }
        }
        if let Some(namespace) = &namespace {
            binding.root_identity = Some(namespace.identity()?);
            plan.preserved.push(RuntimeResetResourceV1::new(
                RuntimeResetResourceClassV1::NamespaceDirectory,
                namespace.path(),
            )?);
            let run = namespace.child_optional(OsStr::new("run"), true)?;
            let state = namespace.child_optional(OsStr::new("state"), true)?;
            let logs = namespace.child_optional(OsStr::new("logs"), true)?;
            binding.run_identity = run.as_ref().map(Directory::identity).transpose()?;
            binding.state_identity = state.as_ref().map(Directory::identity).transpose()?;
            binding.logs_identity = logs.as_ref().map(Directory::identity).transpose()?;
            if let Some(run) = &run {
                plan.preserved.push(RuntimeResetResourceV1::new(
                    RuntimeResetResourceClassV1::NamespaceDirectory,
                    run.path(),
                )?);
                for name in run.entries(MAX_RUNTIME_RESET_RESOURCES_V1)? {
                    if name == "podwayd.lock" {
                        binding.lock_identity = run.regular_optional(&name)?;
                        plan.preserved.push(RuntimeResetResourceV1::new(
                            RuntimeResetResourceClassV1::LockAnchor,
                            &run.path().join(&name),
                        )?);
                    } else if name == "podwayd.sock" {
                        binding.socket_identity = run.identity_optional(&name, FileKind::Socket)?;
                        target.resources.push(RuntimeResetResourceV1::new(
                            RuntimeResetResourceClassV1::Socket,
                            &run.path().join(&name),
                        )?);
                    } else {
                        return Err(RuntimeResetErrorV1::unsafe_reason(
                            RuntimeResetReasonV1::UnexpectedEntry,
                        ));
                    }
                }
            }
            if let Some(state) = &state {
                let mut retained = false;
                for name in state.entries(MAX_RUNTIME_RESET_RESOURCES_V1)? {
                    if name == "workspaces.json.lock" {
                        binding.registry_lock_identity = state.regular_optional(&name)?;
                        retained = true;
                        plan.preserved.push(RuntimeResetResourceV1::new(
                            RuntimeResetResourceClassV1::LockAnchor,
                            &state.path().join(&name),
                        )?);
                        continue;
                    }
                    let class = filesystem::state_resource_class(&name).ok_or_else(|| {
                        RuntimeResetErrorV1::unsafe_reason(RuntimeResetReasonV1::UnexpectedEntry)
                    })?;
                    state.regular_optional(&name)?;
                    if name == "service.json" {
                        let (identity, digest) =
                            metadata_digest(state, &name, crate::SERVICE_METADATA_MAX_BYTES_V1)?;
                        binding.service.metadata_identity = Some(identity);
                        binding.service.metadata_sha256 = Some(digest);
                    }
                    target.resources.push(RuntimeResetResourceV1::new(
                        class,
                        &state.path().join(&name),
                    )?);
                }
                let class = if retained {
                    RuntimeResetResourceClassV1::NamespaceDirectory
                } else {
                    RuntimeResetResourceClassV1::StateDirectory
                };
                let resource = RuntimeResetResourceV1::new(class, state.path())?;
                if retained {
                    plan.preserved.push(resource);
                } else {
                    target.resources.push(resource);
                }
            }
            if let Some(logs) = &logs {
                for name in logs.entries(MAX_RUNTIME_RESET_RESOURCES_V1)? {
                    let class = filesystem::log_resource_class(&name).ok_or_else(|| {
                        RuntimeResetErrorV1::unsafe_reason(RuntimeResetReasonV1::UnexpectedEntry)
                    })?;
                    logs.regular_optional(&name)?;
                    target.resources.push(RuntimeResetResourceV1::new(
                        class,
                        &logs.path().join(&name),
                    )?);
                }
                target.resources.push(RuntimeResetResourceV1::new(
                    RuntimeResetResourceClassV1::LogsDirectory,
                    logs.path(),
                )?);
            }
            if !target.resources.is_empty() || binding.service.loaded {
                let lock = run
                    .as_ref()
                    .ok_or_else(RuntimeResetErrorV1::unsafe_path)?
                    .open_regular(OsStr::new("podwayd.lock"))?;
                let lock = match Flock::lock(lock, FlockArg::LockExclusiveNonblock) {
                    Ok(lock) => Some(lock),
                    Err((_, nix::errno::Errno::EWOULDBLOCK)) => None,
                    Err(_) => {
                        return Err(RuntimeResetErrorV1::unsafe_reason(RuntimeResetReasonV1::Io));
                    }
                };
                match self.inspector.inspect(paths)? {
                    RuntimeResetPeerV1::Offline if lock.is_some() && !binding.service.loaded => {
                        target.state = RuntimeResetTargetStateV1::Offline;
                    }
                    RuntimeResetPeerV1::Offline => {
                        target.state = RuntimeResetTargetStateV1::Unsafe;
                        target.reason = Some(if binding.service.loaded {
                            RuntimeResetReasonV1::ServiceLoaded
                        } else {
                            RuntimeResetReasonV1::LockHeld
                        });
                    }
                    RuntimeResetPeerV1::Unsupported => {
                        target.state = RuntimeResetTargetStateV1::Unsupported;
                        target.reason = Some(RuntimeResetReasonV1::UnsupportedPeer);
                    }
                    RuntimeResetPeerV1::Live {
                        process,
                        busy_reason,
                    } => {
                        process.validate()?;
                        if lock.is_some() {
                            return Err(RuntimeResetErrorV1::unsafe_reason(
                                RuntimeResetReasonV1::IdentityChanged,
                            ));
                        }
                        binding.process = Some(process);
                        target.state = if busy_reason.is_some() {
                            RuntimeResetTargetStateV1::Busy
                        } else {
                            RuntimeResetTargetStateV1::LiveIdle
                        };
                        target.reason = busy_reason;
                    }
                }
                drop(lock);
            }
        } else if !target.resources.is_empty() || binding.service.loaded {
            target.state = RuntimeResetTargetStateV1::Unsafe;
            target.reason = Some(RuntimeResetReasonV1::UnsafePath);
        }
        binding.resource_classes =
            RuntimeResetResourceClassV1::deletion_classes(mode.is_production());
        plan.targets.push(target);
        Ok(Some(binding))
    }

    fn production_service(
        &self,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<ServiceBinding, RuntimeResetErrorV1> {
        let mut service = ServiceBinding::default();
        let account = Directory::account(&self.home)?;
        let agents = account
            .child_optional(OsStr::new("Library"), false)?
            .map(|library| library.child_optional(OsStr::new("LaunchAgents"), false))
            .transpose()?
            .flatten();
        if let Some(agents) = agents {
            let name = paths
                .launch_agent_path()
                .as_path()
                .file_name()
                .expect("fixed plist name");
            if agents.regular_optional(name)?.is_some() {
                let (identity, digest) =
                    metadata_digest(&agents, name, crate::SERVICE_PLIST_MAX_BYTES_V1)?;
                service.installed = true;
                service.plist_identity = Some(identity);
                service.plist_sha256 = Some(digest);
            }
        }
        let domain = format!("gui/{}", self.home.user_id());
        let target = format!("{domain}/{SERVICE_LABEL_V1}");
        let output = self
            .launchctl
            .run(&["print".to_owned(), target.clone()])
            .map_err(|_| RuntimeResetErrorV1::unsafe_reason(RuntimeResetReasonV1::Io))?;
        if launchctl_loaded_state_v1(&output, &target) {
            service.loaded = true;
        } else if !launchctl_not_loaded_v1(&output, &domain) {
            return Err(RuntimeResetErrorV1::unsafe_reason(
                RuntimeResetReasonV1::ServiceLoaded,
            ));
        }
        Ok(service)
    }
}
