use std::{
    collections::BTreeSet,
    fmt,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use podway_core::{RuntimeModeV1, canonicalize_json_v1, verify_canonical_json_v1};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1, MAX_RUNTIME_RESET_MODES_V1,
    MAX_RUNTIME_RESET_RESOURCES_V1, MAX_RUNTIME_RESET_TOKEN_BYTES_V1,
    RUNTIME_RESET_TOKEN_LIFETIME_MS_V1,
};
use crate::PodwayHomeV1;

type Error = RuntimeResetErrorV1;

const MAX_PRESERVED_RESOURCES: usize = 256;
const MAX_EXCLUSIONS: usize = 257;
pub(super) const CONTROL_RESIDUE_NAMES: [&str; 4] = [
    "runtime-reset.lock",
    "runtime-start.lock",
    "runtime-reset-commit.lock",
    "runtime-reset.json",
];
// The CLI adds the public schema discriminator and RFC 3339 expiry.
const PUBLIC_RESULT_OVERHEAD_BYTES: usize = 128;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeResetSelectionV1 {
    Mode { mode: RuntimeModeV1 },
    AllModes,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeResetPathV1 {
    pub path_bytes_base64url: String,
    pub display: String,
}

impl RuntimeResetPathV1 {
    pub fn new(path: &Path) -> Result<Self, Error> {
        let bytes = path.as_os_str().as_bytes();
        if bytes.len() > 4096 {
            return Err(Error::limit());
        }
        let value = Self {
            path_bytes_base64url: encode_bytes(bytes),
            display: path.to_string_lossy().into_owned(),
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.path_bytes_base64url.len() > 5462 || self.display.chars().count() > 4096 {
            return Err(Error::limit());
        }
        let bytes = decode_bytes(&self.path_bytes_base64url)?;
        if bytes.is_empty()
            || bytes.len() > 4096
            || bytes[0] != b'/'
            || bytes.contains(&0)
            || self.display != String::from_utf8_lossy(&bytes)
            || bytes
                .split(|byte| *byte == b'/')
                .skip(1)
                .any(|part| part.is_empty() || part == b"." || part == b"..")
        {
            return Err(Error::invalid_token());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResetReasonV1 {
    Activity,
    Recovery,
    UnknownActivity,
    ServiceLoaded,
    LockHeld,
    UnsafePath,
    UnexpectedEntry,
    UnsupportedPeer,
    ManagedRuntime,
    IdentityChanged,
    Expired,
    InvalidToken,
    SelectionChanged,
    OperationInProgress,
    BoundExceeded,
    Io,
    ShutdownTimeout,
    ReservationLost,
    ResourceChanged,
    MissingIntent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeResetErrorV1 {
    code: &'static str,
    pub mode: Option<RuntimeModeV1>,
    pub reason: RuntimeResetReasonV1,
    pub result: Option<Box<RuntimeResetResultV1>>,
}

impl Error {
    pub fn code(&self) -> &'static str {
        self.code
    }
    pub(super) fn unsafe_path() -> Self {
        Self::unsafe_reason(RuntimeResetReasonV1::UnsafePath)
    }
    /// Reports a failed path, service, or process observation from an infrastructure adapter.
    pub fn unsafe_reason(reason: RuntimeResetReasonV1) -> Self {
        Self {
            code: "RUNTIME_RESET_UNSAFE",
            mode: None,
            reason,
            result: None,
        }
    }
    /// Reports activity that appeared between planning and live reservation.
    pub fn busy_reason(reason: RuntimeResetReasonV1) -> Self {
        Self {
            code: "RUNTIME_RESET_BUSY",
            mode: None,
            reason,
            result: None,
        }
    }
    pub(super) fn stale(reason: RuntimeResetReasonV1) -> Self {
        Self {
            code: "RUNTIME_RESET_PLAN_STALE",
            mode: None,
            reason,
            result: None,
        }
    }
    pub(super) fn in_progress() -> Self {
        Self {
            code: "RUNTIME_RESET_IN_PROGRESS",
            mode: None,
            reason: RuntimeResetReasonV1::OperationInProgress,
            result: None,
        }
    }
    /// Reports a selected runtime or peer outside the executable reset scope.
    pub fn unsupported_reason(reason: RuntimeResetReasonV1) -> Self {
        Self {
            code: "RUNTIME_RESET_UNSUPPORTED",
            mode: None,
            reason,
            result: None,
        }
    }
    pub(super) fn invalid_token() -> Self {
        Self::stale(RuntimeResetReasonV1::InvalidToken)
    }
    pub(super) fn limit() -> Self {
        Self {
            code: "RUNTIME_RESET_LIMIT_EXCEEDED",
            mode: None,
            reason: RuntimeResetReasonV1::BoundExceeded,
            result: None,
        }
    }
    pub(super) fn in_mode(mut self, mode: &RuntimeModeV1) -> Self {
        self.mode = Some(mode.clone());
        self
    }
    pub(super) fn incomplete(reason: RuntimeResetReasonV1, result: RuntimeResetResultV1) -> Self {
        let mode = result
            .modes
            .iter()
            .find(|mode| mode.state != RuntimeResetModeOutcomeStateV1::Complete)
            .map(|mode| mode.mode.clone());
        Self {
            code: "RUNTIME_RESET_INCOMPLETE",
            mode,
            reason,
            result: Some(Box::new(result)),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {:?}", self.code, self.reason)
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResetResourceClassV1 {
    Registry,
    RuntimeRecovery,
    ServiceMetadata,
    Socket,
    DaemonLog,
    BootstrapLog,
    StateDirectory,
    LogsDirectory,
    ServicePlist,
    LockAnchor,
    ControlResidue,
    NamespaceDirectory,
}

impl RuntimeResetResourceClassV1 {
    pub(super) fn deletion_classes(production: bool) -> Vec<Self> {
        let mut classes = vec![
            Self::Registry,
            Self::RuntimeRecovery,
            Self::ServiceMetadata,
            Self::Socket,
            Self::DaemonLog,
            Self::BootstrapLog,
            Self::StateDirectory,
            Self::LogsDirectory,
        ];
        if production {
            classes.push(Self::ServicePlist);
        }
        classes
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeResetResourceV1 {
    pub class: RuntimeResetResourceClassV1,
    pub path: RuntimeResetPathV1,
}
impl RuntimeResetResourceV1 {
    pub(super) fn new(class: RuntimeResetResourceClassV1, path: &Path) -> Result<Self, Error> {
        Ok(Self {
            class,
            path: RuntimeResetPathV1::new(path)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResetPlanStatusV1 {
    Ready,
    NoChange,
    Blocked,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResetTargetStateV1 {
    LiveIdle,
    Offline,
    Absent,
    Busy,
    Unsupported,
    Unsafe,
}
impl RuntimeResetTargetStateV1 {
    pub(super) fn blocks(self) -> bool {
        matches!(self, Self::Busy | Self::Unsupported | Self::Unsafe)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeResetTargetV1 {
    pub mode: RuntimeModeV1,
    pub root: RuntimeResetPathV1,
    pub state: RuntimeResetTargetStateV1,
    pub reason: Option<RuntimeResetReasonV1>,
    pub resources: Vec<RuntimeResetResourceV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeResetExclusionV1 {
    #[serde(deserialize_with = "required_option")]
    pub path: Option<RuntimeResetPathV1>,
    pub reason: ExclusionReason,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    ExternalManagedRuntimes,
    ManagedNamespace,
}
impl RuntimeResetExclusionV1 {
    pub(super) fn managed(path: &Path) -> Result<Self, Error> {
        Ok(Self {
            path: Some(RuntimeResetPathV1::new(path)?),
            reason: ExclusionReason::ManagedNamespace,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeResetOperationV1 {
    pub operation_id: String,
    pub token_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeResetPlanV1 {
    pub status: RuntimeResetPlanStatusV1,
    pub account_root: RuntimeResetPathV1,
    pub selection: RuntimeResetSelectionV1,
    pub targets: Vec<RuntimeResetTargetV1>,
    pub preserved: Vec<RuntimeResetResourceV1>,
    pub excluded: Vec<RuntimeResetExclusionV1>,
    #[serde(skip)]
    pub expires_at_ms: Option<u64>,
    pub plan_token: Option<String>,
    pub recovery_operation: Option<RuntimeResetOperationV1>,
}
impl RuntimeResetPlanV1 {
    pub(super) fn empty(
        home: &PodwayHomeV1,
        selection: RuntimeResetSelectionV1,
    ) -> Result<Self, Error> {
        Ok(Self {
            status: RuntimeResetPlanStatusV1::NoChange,
            account_root: RuntimeResetPathV1::new(home.as_path())?,
            selection,
            targets: Vec::new(),
            preserved: Vec::new(),
            excluded: vec![RuntimeResetExclusionV1 {
                path: None,
                reason: ExclusionReason::ExternalManagedRuntimes,
            }],
            expires_at_ms: None,
            plan_token: None,
            recovery_operation: None,
        })
    }
    pub(super) fn check_bounds(&self) -> Result<(), Error> {
        if self.targets.len() > MAX_RUNTIME_RESET_MODES_V1
            || self.preserved.len() > MAX_PRESERVED_RESOURCES
            || self.excluded.len() > MAX_EXCLUSIONS
            || self
                .targets
                .iter()
                .map(|target| target.resources.len())
                .sum::<usize>()
                > MAX_RUNTIME_RESET_RESOURCES_V1
            || serde_json::to_vec(self).map_err(|_| Error::limit())?.len()
                + PUBLIC_RESULT_OVERHEAD_BYTES
                > MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1
        {
            return Err(Error::limit());
        }
        Ok(())
    }

    pub(super) fn check_apply_preserved_bounds(&self, home: &PodwayHomeV1) -> Result<(), Error> {
        let maintenance = home.as_path().join("maintenance");
        let mut preserved = self.preserved.clone();
        for name in CONTROL_RESIDUE_NAMES {
            let resource = RuntimeResetResourceV1::new(
                RuntimeResetResourceClassV1::ControlResidue,
                &maintenance.join(name),
            )?;
            if !preserved.contains(&resource) {
                preserved.push(resource);
            }
        }
        if preserved.len() > MAX_PRESERVED_RESOURCES {
            return Err(Error::limit());
        }
        Ok(())
    }

    pub(super) fn prospective_apply_resources(
        &self,
        bindings: &[NamespaceBinding],
    ) -> Result<Vec<Vec<RuntimeResetResourceV1>>, Error> {
        if self.targets.len() != bindings.len() {
            return Err(Error::invalid_token());
        }
        let mut prospective = self.clone();
        for (target, binding) in prospective.targets.iter_mut().zip(bindings) {
            if binding.process.is_none() {
                continue;
            }
            // A live daemon may fill every retained log slot while shutting down. Keep the
            // public plan factual, but reserve the complete allowed inventory before issuing
            // an apply token.
            let root = PathBuf::from(std::ffi::OsStr::from_bytes(&decode_bytes(
                &target.root.path_bytes_base64url,
            )?));
            let logs = root.join("logs");
            let directory =
                RuntimeResetResourceV1::new(RuntimeResetResourceClassV1::LogsDirectory, &logs)?;
            if !target.resources.contains(&directory) {
                target.resources.push(directory);
            }
            for (base, class, retained) in [
                (
                    "podwayd.log",
                    RuntimeResetResourceClassV1::DaemonLog,
                    usize::from(crate::SERVICE_LOG_RETAINED_FILES_V1),
                ),
                (
                    "podwayd-bootstrap.log",
                    RuntimeResetResourceClassV1::BootstrapLog,
                    usize::from(crate::SERVICE_BOOTSTRAP_LOG_RETAINED_FILES_V1),
                ),
            ] {
                for index in 0..retained {
                    let name = if index == 0 {
                        base.to_owned()
                    } else {
                        format!("{base}.{index}")
                    };
                    let resource = RuntimeResetResourceV1::new(class, &logs.join(name))?;
                    if !target.resources.contains(&resource) {
                        target.resources.push(resource);
                    }
                }
            }
        }
        prospective.check_bounds()?;
        Ok(prospective
            .targets
            .into_iter()
            .map(|target| target.resources)
            .collect())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeResetProcessV1 {
    pub pid: u32,
    pub process_id: String,
    pub executable: RuntimeResetPathV1,
    pub started_at_ms: u64,
}
impl RuntimeResetProcessV1 {
    pub(super) fn validate(&self) -> Result<(), Error> {
        validate_uuid(&self.process_id)?;
        self.executable.validate()?;
        if self.pid == 0 || self.pid > i32::MAX as u32 || self.started_at_ms > i64::MAX as u64 {
            return Err(Error::invalid_token());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub uid: u32,
    pub mode: u32,
    pub links: u32,
}
impl FileIdentity {
    pub(super) fn same_directory_anchor(&self, other: &Self) -> bool {
        // Creating or retiring child directories changes link count without replacing the anchor.
        (self.device, self.inode, self.uid, self.mode)
            == (other.device, other.inode, other.uid, other.mode)
    }

    fn validate(&self, uid: u32) -> Result<(), Error> {
        if self.device > i64::MAX as u64
            || self.inode > i64::MAX as u64
            || self.uid != uid
            || self.mode > 65535
            || self.links == 0
        {
            return Err(Error::invalid_token());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServiceBinding {
    pub installed: bool,
    pub loaded: bool,
    #[serde(deserialize_with = "required_option")]
    pub plist_identity: Option<FileIdentity>,
    #[serde(deserialize_with = "required_option")]
    pub plist_sha256: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub metadata_identity: Option<FileIdentity>,
    #[serde(deserialize_with = "required_option")]
    pub metadata_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NamespaceBinding {
    pub mode: RuntimeModeV1,
    pub root: RuntimeResetPathV1,
    #[serde(deserialize_with = "required_option")]
    pub root_identity: Option<FileIdentity>,
    #[serde(deserialize_with = "required_option")]
    pub run_identity: Option<FileIdentity>,
    #[serde(deserialize_with = "required_option")]
    pub state_identity: Option<FileIdentity>,
    #[serde(deserialize_with = "required_option")]
    pub logs_identity: Option<FileIdentity>,
    #[serde(deserialize_with = "required_option")]
    pub lock_identity: Option<FileIdentity>,
    #[serde(deserialize_with = "required_option")]
    pub registry_lock_identity: Option<FileIdentity>,
    #[serde(deserialize_with = "required_option")]
    pub socket_identity: Option<FileIdentity>,
    #[serde(deserialize_with = "required_option")]
    pub process: Option<RuntimeResetProcessV1>,
    pub service: ServiceBinding,
    pub resource_classes: Vec<RuntimeResetResourceClassV1>,
}
impl NamespaceBinding {
    pub(super) fn empty(mode: RuntimeModeV1, root: RuntimeResetPathV1) -> Self {
        Self {
            mode,
            root,
            root_identity: None,
            run_identity: None,
            state_identity: None,
            logs_identity: None,
            lock_identity: None,
            registry_lock_identity: None,
            socket_identity: None,
            process: None,
            service: ServiceBinding::default(),
            resource_classes: Vec::new(),
        }
    }

    pub(super) fn is_absent(&self) -> bool {
        self.run_identity.is_none()
            && self.state_identity.is_none()
            && self.logs_identity.is_none()
            && self.lock_identity.is_none()
            && self.registry_lock_identity.is_none()
            && self.socket_identity.is_none()
            && self.process.is_none()
            && !self.service.installed
            && !self.service.loaded
            && self.service.plist_identity.is_none()
            && self.service.metadata_identity.is_none()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResetToken {
    pub schema: String,
    pub caller_uid: u32,
    pub account_root: RuntimeResetPathV1,
    pub account_identity: FileIdentity,
    pub selection: RuntimeResetSelectionV1,
    pub mode_inventory: Vec<RuntimeModeV1>,
    pub targets: Vec<NamespaceBinding>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}
impl ResetToken {
    pub(super) fn encode(&self) -> Result<String, Error> {
        self.validate()?;
        let canonical = canonicalize_json_v1(self).map_err(|_| Error::invalid_token())?;
        let encoded = format!(
            "{}.{:x}",
            encode_bytes(canonical.as_bytes()),
            Sha256::digest(canonical.as_bytes())
        );
        if encoded.len() > MAX_RUNTIME_RESET_TOKEN_BYTES_V1 {
            return Err(Error::limit());
        }
        Ok(encoded)
    }
    pub(super) fn decode(encoded: &str) -> Result<Self, Error> {
        if encoded.len() > MAX_RUNTIME_RESET_TOKEN_BYTES_V1 {
            return Err(Error::limit());
        }
        let (payload, digest) = encoded.split_once('.').ok_or_else(Error::invalid_token)?;
        let bytes = decode_bytes(payload)?;
        if digest != format!("{:x}", Sha256::digest(&bytes)) {
            return Err(Error::invalid_token());
        }
        verify_canonical_json_v1(&bytes).map_err(|_| Error::invalid_token())?;
        let token: Self = serde_json::from_slice(&bytes).map_err(|_| Error::invalid_token())?;
        token.validate()?;
        if canonicalize_json_v1(&token)
            .map_err(|_| Error::invalid_token())?
            .as_bytes()
            != bytes
        {
            return Err(Error::invalid_token());
        }
        Ok(token)
    }
    fn validate(&self) -> Result<(), Error> {
        if self.schema != "podway.runtime-reset-token/v1"
            || self.caller_uid == 0
            || self
                .created_at_ms
                .checked_add(RUNTIME_RESET_TOKEN_LIFETIME_MS_V1)
                != Some(self.expires_at_ms)
            || self.expires_at_ms > 253_402_300_799_999
            || self.targets.is_empty()
        {
            return Err(Error::invalid_token());
        }
        if self.targets.len() > MAX_RUNTIME_RESET_MODES_V1
            || self.mode_inventory.len() > MAX_RUNTIME_RESET_MODES_V1
        {
            return Err(Error::limit());
        }
        self.account_root.validate()?;
        self.account_identity.validate(self.caller_uid)?;
        let account = decode_bytes(&self.account_root.path_bytes_base64url)?;
        let mut modes = BTreeSet::new();
        for target in &self.targets {
            if !modes.insert(target.mode.as_str()) {
                return Err(Error::invalid_token());
            }
            target.root.validate()?;
            let mut expected = account.clone();
            if !target.mode.is_production() {
                expected.extend_from_slice(format!("/modes/{}", target.mode.as_str()).as_bytes());
            }
            if decode_bytes(&target.root.path_bytes_base64url)? != expected
                || target.resource_classes
                    != RuntimeResetResourceClassV1::deletion_classes(target.mode.is_production())
            {
                return Err(Error::invalid_token());
            }
            for identity in [
                &target.root_identity,
                &target.run_identity,
                &target.state_identity,
                &target.logs_identity,
                &target.lock_identity,
                &target.registry_lock_identity,
                &target.socket_identity,
                &target.service.plist_identity,
                &target.service.metadata_identity,
            ]
            .into_iter()
            .flatten()
            {
                identity.validate(self.caller_uid)?;
            }
            if let Some(process) = &target.process {
                process.validate()?;
            }
            for digest in [
                &target.service.plist_sha256,
                &target.service.metadata_sha256,
            ]
            .into_iter()
            .flatten()
            {
                validate_digest(digest)?;
            }
            if target.service.installed != target.service.plist_identity.is_some()
                || target.service.plist_identity.is_some() != target.service.plist_sha256.is_some()
                || target.service.metadata_identity.is_some()
                    != target.service.metadata_sha256.is_some()
                || (!target.mode.is_production()
                    && (target.service.installed || target.service.loaded))
            {
                return Err(Error::invalid_token());
            }
        }
        match &self.selection {
            RuntimeResetSelectionV1::Mode { mode }
                if !self.mode_inventory.is_empty()
                    || self.targets.len() != 1
                    || self.targets[0].mode != *mode =>
            {
                return Err(Error::invalid_token());
            }
            RuntimeResetSelectionV1::AllModes => {
                let observed: Vec<_> = self
                    .targets
                    .iter()
                    .map(|target| target.mode.clone())
                    .collect();
                let mut sorted = observed.clone();
                sorted.sort_by_key(|mode| (!mode.is_production(), mode.as_str().to_owned()));
                if observed != self.mode_inventory
                    || observed != sorted
                    || !observed[0].is_production()
                {
                    return Err(Error::invalid_token());
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn check_prospective_record_bounds(
        &self,
        prospective_resources: &[Vec<RuntimeResetResourceV1>],
        home: &PodwayHomeV1,
    ) -> Result<(), Error> {
        if prospective_resources.len() != self.targets.len() {
            return Err(Error::invalid_token());
        }
        let identity = FileIdentity {
            device: i64::MAX as u64,
            inode: i64::MAX as u64,
            uid: home.user_id(),
            mode: u16::MAX.into(),
            links: u32::MAX,
        };
        // Maximum-width identities and the longest entry state make this a serialized upper
        // bound for the final in-progress record rather than an estimate of current bytes.
        let mut modes = Vec::with_capacity(self.targets.len());
        for (target, resources) in self.targets.iter().zip(prospective_resources) {
            let root = PathBuf::from(std::ffi::OsStr::from_bytes(&decode_bytes(
                &target.root.path_bytes_base64url,
            )?));
            let mut paths = BTreeSet::new();
            let mut entries = Vec::with_capacity(resources.len());
            for resource in resources {
                if resource.class == RuntimeResetResourceClassV1::ServicePlist {
                    continue;
                }
                let path = PathBuf::from(std::ffi::OsStr::from_bytes(&decode_bytes(
                    &resource.path.path_bytes_base64url,
                )?));
                let relative = path
                    .strip_prefix(&root)
                    .map_err(|_| Error::invalid_token())?;
                if relative.as_os_str().is_empty()
                    || !paths.insert(relative.as_os_str().as_bytes().to_vec())
                {
                    return Err(Error::invalid_token());
                }
                let kind = match resource.class {
                    RuntimeResetResourceClassV1::StateDirectory
                    | RuntimeResetResourceClassV1::LogsDirectory => EntryKind::Directory,
                    RuntimeResetResourceClassV1::Socket => EntryKind::Socket,
                    _ => EntryKind::Regular,
                };
                entries.push(RecordEntry {
                    class: resource.class,
                    relative_path_bytes_base64url: encode_bytes(relative.as_os_str().as_bytes()),
                    identity: identity.clone(),
                    kind,
                    state: EntryState::Completed,
                });
            }
            modes.push(RecordMode {
                mode: target.mode.clone(),
                phase: if target.is_absent() {
                    ModePhase::Complete
                } else {
                    ModePhase::InventoryReady
                },
                reservation_id: target
                    .process
                    .as_ref()
                    .map(|_| "00000000-0000-4000-8000-000000000000".to_owned()),
                entries,
            });
        }
        let encoded = self.encode()?;
        let operation_id = "00000000-0000-4000-8000-000000000000".to_owned();
        let token_sha256 = format!("sha256:{:x}", Sha256::digest(encoded.as_bytes()));
        ResetRecord {
            schema: "podway.runtime-reset-record/v1".to_owned(),
            operation_id: operation_id.clone(),
            token_sha256: token_sha256.clone(),
            phase: RecordPhase::InProgress,
            plan: Some(self.clone()),
            modes,
            result: None,
        }
        .encode(home)?;

        let modes = self
            .targets
            .iter()
            .zip(prospective_resources)
            .map(|(target, resources)| RuntimeResetModeOutcomeV1 {
                mode: target.mode.clone(),
                state: RuntimeResetModeOutcomeStateV1::Complete,
                resources: resources
                    .iter()
                    .map(|resource| RuntimeResetResourceOutcomeV1 {
                        class: resource.class,
                        path: resource.path.clone(),
                        // This is the longest valid terminal state and therefore bounds both
                        // removed and already-absent outcomes.
                        state: RuntimeResetResourceOutcomeStateV1::AlreadyAbsent,
                    })
                    .collect(),
            })
            .collect();
        let mut preserved = Vec::new();
        for target in &self.targets {
            let root = PathBuf::from(std::ffi::OsStr::from_bytes(&decode_bytes(
                &target.root.path_bytes_base64url,
            )?));
            preserved.push(RuntimeResetResourceV1::new(
                RuntimeResetResourceClassV1::NamespaceDirectory,
                &root,
            )?);
            if target.run_identity.is_some() {
                preserved.push(RuntimeResetResourceV1::new(
                    RuntimeResetResourceClassV1::NamespaceDirectory,
                    &root.join("run"),
                )?);
            }
            if target.lock_identity.is_some() {
                preserved.push(RuntimeResetResourceV1::new(
                    RuntimeResetResourceClassV1::LockAnchor,
                    &root.join("run/podwayd.lock"),
                )?);
            }
            if target.registry_lock_identity.is_some() {
                preserved.push(RuntimeResetResourceV1::new(
                    RuntimeResetResourceClassV1::NamespaceDirectory,
                    &root.join("state"),
                )?);
                preserved.push(RuntimeResetResourceV1::new(
                    RuntimeResetResourceClassV1::LockAnchor,
                    &root.join("state/workspaces.json.lock"),
                )?);
            }
        }
        for name in CONTROL_RESIDUE_NAMES {
            preserved.push(RuntimeResetResourceV1::new(
                RuntimeResetResourceClassV1::ControlResidue,
                &home.as_path().join("maintenance").join(name),
            )?);
        }
        preserved.sort_by(|left, right| {
            left.path
                .path_bytes_base64url
                .as_bytes()
                .cmp(right.path.path_bytes_base64url.as_bytes())
        });
        preserved.dedup();
        ResetRecord {
            schema: "podway.runtime-reset-record/v1".to_owned(),
            operation_id: operation_id.clone(),
            token_sha256,
            phase: RecordPhase::Completed,
            plan: None,
            modes: Vec::new(),
            result: Some(RuntimeResetResultV1 {
                schema: "podway.runtime-reset-result/v1".to_owned(),
                status: RuntimeResetResultStatusV1::Complete,
                operation_id,
                selection: self.selection.clone(),
                modes,
                preserved,
                excluded: vec![RuntimeResetExclusionV1 {
                    path: None,
                    reason: ExclusionReason::ExternalManagedRuntimes,
                }],
            }),
        }
        .encode(home)?;
        Ok(())
    }

    pub(super) fn normalize_directory_links(&mut self) {
        self.account_identity.links = 1;
        for target in &mut self.targets {
            for identity in [
                &mut target.root_identity,
                &mut target.run_identity,
                &mut target.state_identity,
                &mut target.logs_identity,
            ]
            .into_iter()
            .flatten()
            {
                identity.links = 1;
            }
        }
    }
}

pub(super) fn validate_uuid(value: &str) -> Result<(), Error> {
    let uuid = uuid::Uuid::parse_str(value).map_err(|_| Error::invalid_token())?;
    if uuid.hyphenated().to_string() != value {
        return Err(Error::invalid_token());
    }
    Ok(())
}
fn validate_digest(value: &str) -> Result<(), Error> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or_else(Error::invalid_token)?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::invalid_token());
    }
    Ok(())
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
pub(super) fn encode_bytes(bytes: &[u8]) -> String {
    let mut result = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        result.push(ALPHABET[(chunk[0] >> 2) as usize] as char);
        result.push(
            ALPHABET[(((chunk[0] & 3) << 4) | (chunk.get(1).copied().unwrap_or(0) >> 4)) as usize]
                as char,
        );
        if chunk.len() > 1 {
            result.push(
                ALPHABET
                    [(((chunk[1] & 15) << 2) | (chunk.get(2).copied().unwrap_or(0) >> 6)) as usize]
                    as char,
            );
        }
        if chunk.len() > 2 {
            result.push(ALPHABET[(chunk[2] & 63) as usize] as char);
        }
    }
    result
}
pub(super) fn decode_bytes(value: &str) -> Result<Vec<u8>, Error> {
    if value.is_empty() || value.len() % 4 == 1 {
        return Err(Error::invalid_token());
    }
    let mut result = Vec::with_capacity(value.len() / 4 * 3 + 2);
    for chunk in value.as_bytes().chunks(4) {
        let mut sextets = [0u8; 4];
        for (index, byte) in chunk.iter().enumerate() {
            sextets[index] = ALPHABET
                .iter()
                .position(|item| item == byte)
                .ok_or_else(Error::invalid_token)? as u8;
        }
        result.push((sextets[0] << 2) | (sextets[1] >> 4));
        if chunk.len() > 2 {
            result.push((sextets[1] << 4) | (sextets[2] >> 2));
        }
        if chunk.len() > 3 {
            result.push((sextets[2] << 6) | sextets[3]);
        }
    }
    if encode_bytes(&result) != value {
        return Err(Error::invalid_token());
    }
    Ok(result)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResetRecord {
    pub(super) schema: String,
    pub(super) operation_id: String,
    pub(super) token_sha256: String,
    pub(super) phase: RecordPhase,
    #[serde(deserialize_with = "required_option")]
    pub(super) plan: Option<ResetToken>,
    pub(super) modes: Vec<RecordMode>,
    #[serde(deserialize_with = "required_option")]
    pub(super) result: Option<RuntimeResetResultV1>,
}
#[derive(Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum RecordPhase {
    InProgress,
    Completed,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecordMode {
    pub(super) mode: RuntimeModeV1,
    pub(super) phase: ModePhase,
    #[serde(deserialize_with = "required_option")]
    pub(super) reservation_id: Option<String>,
    pub(super) entries: Vec<RecordEntry>,
}
#[derive(Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ModePhase {
    Pending,
    StopIntended,
    Stopped,
    InventoryIntended,
    InventoryReady,
    Complete,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecordEntry {
    pub(super) class: RuntimeResetResourceClassV1,
    pub(super) relative_path_bytes_base64url: String,
    pub(super) identity: FileIdentity,
    pub(super) kind: EntryKind,
    pub(super) state: EntryState,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum EntryKind {
    Regular,
    Directory,
    Socket,
}
#[derive(Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum EntryState {
    Pending,
    Intended,
    Completed,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeResetResultV1 {
    pub schema: String,
    pub status: RuntimeResetResultStatusV1,
    pub operation_id: String,
    pub selection: RuntimeResetSelectionV1,
    pub modes: Vec<RuntimeResetModeOutcomeV1>,
    pub preserved: Vec<RuntimeResetResourceV1>,
    pub excluded: Vec<RuntimeResetExclusionV1>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResetResultStatusV1 {
    Complete,
    AlreadyApplied,
    Incomplete,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeResetModeOutcomeV1 {
    pub mode: RuntimeModeV1,
    pub state: RuntimeResetModeOutcomeStateV1,
    pub resources: Vec<RuntimeResetResourceOutcomeV1>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResetModeOutcomeStateV1 {
    Complete,
    Pending,
    Incomplete,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeResetResourceOutcomeV1 {
    pub class: RuntimeResetResourceClassV1,
    pub path: RuntimeResetPathV1,
    pub state: RuntimeResetResourceOutcomeStateV1,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResetResourceOutcomeStateV1 {
    Removed,
    AlreadyAbsent,
    Pending,
}

impl ResetRecord {
    pub(super) fn decode_for_apply(bytes: &[u8], home: &PodwayHomeV1) -> Result<Self, Error> {
        Self::decode(bytes, home)
    }

    pub(super) fn encode(&self, home: &PodwayHomeV1) -> Result<Vec<u8>, Error> {
        self.validate(home)?;
        let value = canonicalize_json_v1(self).map_err(|_| Error::limit())?;
        if value.len() > MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1 {
            return Err(Error::limit());
        }
        Ok(value.into_bytes())
    }
    pub(super) fn view(
        bytes: &[u8],
        home: &PodwayHomeV1,
    ) -> Result<Option<super::RuntimeResetRecordViewV1>, Error> {
        let record = Self::decode(bytes, home)?;
        if record.phase == RecordPhase::Completed {
            return Ok(None);
        }
        let plan = record
            .plan
            .as_ref()
            .expect("validated in-progress record has a plan");
        Ok(Some(super::RuntimeResetRecordViewV1 {
            operation: RuntimeResetOperationV1 {
                operation_id: record.operation_id.clone(),
                token_sha256: record.token_sha256.clone(),
            },
            participants: record
                .modes
                .iter()
                .zip(&plan.targets)
                .map(|(mode, target)| super::RuntimeResetParticipantV1 {
                    mode: mode.mode.clone(),
                    root: target.root.clone(),
                    process: target.process.clone(),
                    reservation_id: mode.reservation_id.clone(),
                    stop_intended: mode.phase == ModePhase::StopIntended,
                })
                .collect(),
            account_identity: plan.account_identity.clone(),
        }))
    }

    pub(super) fn operation(
        bytes: &[u8],
        home: &PodwayHomeV1,
    ) -> Result<Option<RuntimeResetOperationV1>, Error> {
        let record = Self::decode(bytes, home)?;
        Ok(
            (record.phase == RecordPhase::InProgress).then_some(RuntimeResetOperationV1 {
                operation_id: record.operation_id,
                token_sha256: record.token_sha256,
            }),
        )
    }
    fn decode(bytes: &[u8], home: &PodwayHomeV1) -> Result<Self, Error> {
        if bytes.len() > MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1 {
            return Err(Error::limit());
        }
        let record: Self = serde_json::from_slice(bytes).map_err(|_| Error::unsafe_path())?;
        record.validate(home).map_err(|error| {
            if error.reason == RuntimeResetReasonV1::BoundExceeded {
                error
            } else {
                Error::unsafe_path()
            }
        })?;
        Ok(record)
    }
    fn validate(&self, home: &PodwayHomeV1) -> Result<(), Error> {
        validate_uuid(&self.operation_id)?;
        validate_digest(&self.token_sha256)?;
        if self.schema != "podway.runtime-reset-record/v1" {
            return Err(Error::invalid_token());
        }
        if self.modes.len() > MAX_RUNTIME_RESET_MODES_V1
            || self
                .modes
                .iter()
                .map(|mode| mode.entries.len())
                .sum::<usize>()
                > MAX_RUNTIME_RESET_RESOURCES_V1
        {
            return Err(Error::limit());
        }
        match self.phase {
            RecordPhase::InProgress => {
                let plan = self.plan.as_ref().ok_or_else(Error::invalid_token)?;
                plan.validate()?;
                if plan.caller_uid != home.user_id()
                    || plan.account_root != RuntimeResetPathV1::new(home.as_path())?
                    || self.result.is_some()
                    || self.modes.len() != plan.targets.len()
                    || self.token_sha256
                        != format!("sha256:{:x}", Sha256::digest(plan.encode()?.as_bytes()))
                {
                    return Err(Error::invalid_token());
                }
                for (mode, target) in self.modes.iter().zip(&plan.targets) {
                    if mode.mode != target.mode {
                        return Err(Error::invalid_token());
                    }
                    if let Some(reservation) = &mode.reservation_id {
                        validate_uuid(reservation)?;
                    }
                    if matches!(
                        mode.phase,
                        ModePhase::Pending
                            | ModePhase::StopIntended
                            | ModePhase::Stopped
                            | ModePhase::InventoryIntended
                    ) && !mode.entries.is_empty()
                    {
                        return Err(Error::invalid_token());
                    }
                    let mut paths = BTreeSet::new();
                    for entry in &mode.entries {
                        entry.identity.validate(home.user_id())?;
                        let path = decode_bytes(&entry.relative_path_bytes_base64url)?;
                        if path.len() > 4096 || !paths.insert(path.clone()) {
                            return Err(Error::invalid_token());
                        }
                        let components: Vec<_> = path.split(|byte| *byte == b'/').collect();
                        if components.len() > 8
                            || components.iter().any(|part| {
                                part.is_empty()
                                    || *part == b"."
                                    || *part == b".."
                                    || part.contains(&0)
                            })
                        {
                            return Err(Error::invalid_token());
                        }
                        let path = Path::new(std::ffi::OsStr::from_bytes(&path));
                        let class = if components.len() == 1 && path == Path::new("state") {
                            RuntimeResetResourceClassV1::StateDirectory
                        } else if components.len() == 1 && path == Path::new("logs") {
                            RuntimeResetResourceClassV1::LogsDirectory
                        } else if path == Path::new("run/podwayd.sock") {
                            RuntimeResetResourceClassV1::Socket
                        } else if components.len() == 2 && components[0] == b"state" {
                            super::filesystem::state_resource_class(
                                path.file_name().ok_or_else(Error::invalid_token)?,
                            )
                            .ok_or_else(Error::invalid_token)?
                        } else if components.len() == 2 && components[0] == b"logs" {
                            super::filesystem::log_resource_class(
                                path.file_name().ok_or_else(Error::invalid_token)?,
                            )
                            .ok_or_else(Error::invalid_token)?
                        } else {
                            return Err(Error::invalid_token());
                        };
                        let kind = match class {
                            RuntimeResetResourceClassV1::StateDirectory
                            | RuntimeResetResourceClassV1::LogsDirectory => EntryKind::Directory,
                            RuntimeResetResourceClassV1::Socket => EntryKind::Socket,
                            _ => EntryKind::Regular,
                        };
                        if class != entry.class
                            || kind != entry.kind
                            || (mode.phase == ModePhase::Complete
                                && entry.state != EntryState::Completed)
                        {
                            return Err(Error::invalid_token());
                        }
                    }
                }
            }
            RecordPhase::Completed => {
                let result = self.result.as_ref().ok_or_else(Error::invalid_token)?;
                if self.plan.is_some()
                    || !self.modes.is_empty()
                    || result.schema != "podway.runtime-reset-result/v1"
                    || result.status != RuntimeResetResultStatusV1::Complete
                    || result.operation_id != self.operation_id
                    || result.modes.is_empty()
                {
                    return Err(Error::invalid_token());
                }
                if result.modes.len() > MAX_RUNTIME_RESET_MODES_V1
                    || result.preserved.len() > MAX_PRESERVED_RESOURCES
                    || result.excluded.is_empty()
                    || result.excluded.len() > MAX_EXCLUSIONS
                    || result
                        .modes
                        .iter()
                        .map(|mode| mode.resources.len())
                        .sum::<usize>()
                        > MAX_RUNTIME_RESET_RESOURCES_V1
                {
                    return Err(Error::limit());
                }
                let mut modes = BTreeSet::new();
                for mode in &result.modes {
                    if !modes.insert(mode.mode.as_str())
                        || mode.state != RuntimeResetModeOutcomeStateV1::Complete
                    {
                        return Err(Error::invalid_token());
                    }
                    for resource in &mode.resources {
                        resource.path.validate()?;
                        if resource.state == RuntimeResetResourceOutcomeStateV1::Pending
                            || !RuntimeResetResourceClassV1::deletion_classes(
                                mode.mode.is_production(),
                            )
                            .contains(&resource.class)
                        {
                            return Err(Error::invalid_token());
                        }
                    }
                }
                if let RuntimeResetSelectionV1::Mode { mode } = &result.selection
                    && (result.modes.len() != 1 || result.modes[0].mode != *mode)
                {
                    return Err(Error::invalid_token());
                }
                for resource in &result.preserved {
                    resource.path.validate()?;
                }
                for exclusion in &result.excluded {
                    match (&exclusion.path, exclusion.reason) {
                        (Some(path), ExclusionReason::ManagedNamespace) => path.validate()?,
                        (None, ExclusionReason::ExternalManagedRuntimes) => {}
                        _ => return Err(Error::invalid_token()),
                    }
                }
            }
        }
        Ok(())
    }
}

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

    fn token() -> ResetToken {
        let root = RuntimeResetPathV1::new(Path::new(OsStr::from_bytes(b"/account-\xff/.podway")))
            .unwrap();
        let identity = FileIdentity {
            device: 1,
            inode: 2,
            uid: 501,
            mode: 0o40700,
            links: 2,
        };
        let mut target = NamespaceBinding::empty(RuntimeModeV1::production(), root.clone());
        target.root_identity = Some(identity.clone());
        target.resource_classes = RuntimeResetResourceClassV1::deletion_classes(true);
        ResetToken {
            schema: "podway.runtime-reset-token/v1".to_owned(),
            caller_uid: 501,
            account_root: root,
            account_identity: identity,
            selection: RuntimeResetSelectionV1::Mode {
                mode: RuntimeModeV1::production(),
            },
            mode_inventory: Vec::new(),
            targets: vec![target],
            created_at_ms: 1000,
            expires_at_ms: 601000,
        }
    }

    #[test]
    fn plan_document_bound_rejects_bytes_before_resource_count_limit() {
        let home = PodwayHomeV1::from_account_home("/account", 501).unwrap();
        let mut plan = RuntimeResetPlanV1::empty(&home, RuntimeResetSelectionV1::AllModes).unwrap();
        let path = RuntimeResetPathV1::new(Path::new(&format!("/{}", "a".repeat(4000)))).unwrap();
        for _ in 0..80 {
            plan.excluded.push(RuntimeResetExclusionV1 {
                path: Some(path.clone()),
                reason: ExclusionReason::ManagedNamespace,
            });
        }
        assert!(plan.excluded.len() < MAX_EXCLUSIONS);
        assert!(serde_json::to_vec(&plan).unwrap().len() > MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1);
        assert_eq!(
            plan.check_bounds().unwrap_err().code(),
            "RUNTIME_RESET_LIMIT_EXCEEDED"
        );
        plan.excluded.truncate(2);
        plan.check_bounds().unwrap();
    }

    #[test]
    fn prospective_live_inventory_reserves_every_shutdown_log_rotation() {
        let home = PodwayHomeV1::from_account_home("/account", 501).unwrap();
        let mode = RuntimeModeV1::development();
        let root = RuntimeResetPathV1::new(Path::new("/account/.podway/modes/dev")).unwrap();
        let mut plan =
            RuntimeResetPlanV1::empty(&home, RuntimeResetSelectionV1::Mode { mode: mode.clone() })
                .unwrap();
        plan.targets.push(RuntimeResetTargetV1 {
            mode: mode.clone(),
            root: root.clone(),
            state: RuntimeResetTargetStateV1::LiveIdle,
            reason: None,
            resources: Vec::new(),
        });
        let mut binding = NamespaceBinding::empty(mode, root);
        binding.process = Some(RuntimeResetProcessV1 {
            pid: 42,
            process_id: "00000000-0000-4000-8000-000000000042".to_owned(),
            executable: RuntimeResetPathV1::new(Path::new("/account/bin/podwayd")).unwrap(),
            started_at_ms: 1,
        });
        binding.resource_classes = RuntimeResetResourceClassV1::deletion_classes(false);

        let resources = plan.prospective_apply_resources(&[binding]).unwrap();
        assert!(plan.targets[0].resources.is_empty());
        assert_eq!(
            resources[0]
                .iter()
                .filter(|resource| resource.class == RuntimeResetResourceClassV1::DaemonLog)
                .count(),
            usize::from(crate::SERVICE_LOG_RETAINED_FILES_V1)
        );
        assert_eq!(
            resources[0]
                .iter()
                .filter(|resource| resource.class == RuntimeResetResourceClassV1::BootstrapLog)
                .count(),
            usize::from(crate::SERVICE_BOOTSTRAP_LOG_RETAINED_FILES_V1)
        );
        assert_eq!(
            resources[0]
                .iter()
                .filter(|resource| resource.class == RuntimeResetResourceClassV1::LogsDirectory)
                .count(),
            1
        );
    }

    #[test]
    fn prospective_record_size_rejects_before_the_resource_count_limit() {
        let home = PodwayHomeV1::from_account_home("/account", 501).unwrap();
        let mut token = token();
        token.account_root = RuntimeResetPathV1::new(home.as_path()).unwrap();
        token.targets[0].root = token.account_root.clone();
        let resources = (0..(MAX_RUNTIME_RESET_RESOURCES_V1 - 1))
            .map(|index| {
                RuntimeResetResourceV1::new(
                    RuntimeResetResourceClassV1::Registry,
                    &home
                        .as_path()
                        .join(format!("state/.podway-registry-v1-1-{index}.tmp")),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(resources.len(), MAX_RUNTIME_RESET_RESOURCES_V1 - 1);
        assert_eq!(
            token
                .check_prospective_record_bounds(&[resources], &home)
                .unwrap_err()
                .code(),
            "RUNTIME_RESET_LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn prospective_completed_receipt_size_is_rejected_before_apply() {
        let account_root = format!("/{}", "a".repeat(99));
        let home = PodwayHomeV1::from_account_home(&account_root, 501).unwrap();
        let mut token = token();
        token.account_root = RuntimeResetPathV1::new(home.as_path()).unwrap();
        token.targets[0].root = token.account_root.clone();
        token.targets[0].registry_lock_identity = Some(token.account_identity.clone());
        let resources = (0..1_290)
            .map(|index| {
                RuntimeResetResourceV1::new(
                    RuntimeResetResourceClassV1::Registry,
                    &home
                        .as_path()
                        .join(format!("state/.podway-registry-v1-1-{index}.tmp")),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            token
                .check_prospective_record_bounds(&[resources], &home)
                .unwrap_err()
                .code(),
            "RUNTIME_RESET_LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn account_record_reader_rejects_malformed_or_unbound_recovery() {
        let mut token = token();
        let home = PodwayHomeV1::from_account_home("/account", 501).unwrap();
        token.account_root = RuntimeResetPathV1::new(home.as_path()).unwrap();
        token.targets[0].root = token.account_root.clone();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let token_sha256 = format!(
            "sha256:{:x}",
            Sha256::digest(token.encode().unwrap().as_bytes())
        );
        let record = serde_json::json!({
            "schema": "podway.runtime-reset-record/v1", "operation_id": operation_id,
            "token_sha256": token_sha256, "phase": "in_progress", "plan": token,
            "modes": [{"mode": "prod", "phase": "pending", "reservation_id": null, "entries": []}], "result": null,
        });
        let operation = ResetRecord::operation(&serde_json::to_vec(&record).unwrap(), &home)
            .unwrap()
            .unwrap();
        assert_eq!(operation.operation_id, operation_id);
        for (key, value) in [
            (
                "token_sha256",
                serde_json::json!(format!("sha256:{}", "0".repeat(64))),
            ),
            ("phase", serde_json::json!("completed")),
            ("plan", serde_json::Value::Null),
        ] {
            let mut altered = record.clone();
            altered[key] = value;
            assert!(ResetRecord::operation(&serde_json::to_vec(&altered).unwrap(), &home).is_err());
        }
        let mut altered = record.clone();
        altered.as_object_mut().unwrap().remove("result");
        assert!(ResetRecord::operation(&serde_json::to_vec(&altered).unwrap(), &home).is_err());
        let completed = serde_json::json!({
            "schema": "podway.runtime-reset-record/v1", "operation_id": operation_id,
            "token_sha256": token_sha256, "phase": "completed", "plan": null, "modes": [],
            "result": {"schema": "podway.runtime-reset-result/v1", "status": "complete", "operation_id": operation_id,
                "selection": {"kind": "mode", "mode": "prod"}, "modes": [{"mode":"prod", "state":"complete", "resources":[]}],
                "preserved": [], "excluded": [{"path":null, "reason":"external_managed_runtimes"}]},
        });
        assert!(
            ResetRecord::operation(&serde_json::to_vec(&completed).unwrap(), &home)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn token_decoder_rejects_inconsistent_identity_and_selection_bindings() {
        let mut baseline = token();
        baseline.account_root = RuntimeResetPathV1::new(Path::new("/account/.podway")).unwrap();
        baseline.targets[0].root = baseline.account_root.clone();
        let mut named = NamespaceBinding::empty(
            RuntimeModeV1::development(),
            RuntimeResetPathV1::new(Path::new("/account/.podway/modes/dev")).unwrap(),
        );
        named.root_identity = Some(baseline.account_identity.clone());
        named.resource_classes = RuntimeResetResourceClassV1::deletion_classes(false);
        baseline.selection = RuntimeResetSelectionV1::AllModes;
        baseline.mode_inventory = vec![RuntimeModeV1::production(), RuntimeModeV1::development()];
        baseline.targets.push(named);
        assert_eq!(
            ResetToken::decode(&baseline.encode().unwrap()).unwrap(),
            baseline
        );
        for mutation in 0..8 {
            let mut altered = baseline.clone();
            match mutation {
                0 => altered.caller_uid = 0,
                1 => altered.account_identity.uid += 1,
                2 => altered.targets[1].root_identity.as_mut().unwrap().uid += 1,
                3 => {
                    altered.targets[1].root =
                        RuntimeResetPathV1::new(Path::new("/account/.podway/modes/qa")).unwrap()
                }
                4 => altered.targets[1].service.loaded = true,
                5 => {
                    altered.targets[1].service.installed = true;
                    altered.targets[1].service.plist_identity =
                        Some(altered.account_identity.clone());
                    altered.targets[1].service.plist_sha256 =
                        Some(format!("sha256:{}", "a".repeat(64)));
                }
                6 => {
                    altered.targets.reverse();
                    altered.mode_inventory.reverse();
                }
                7 => altered.mode_inventory[1] = RuntimeModeV1::production(),
                _ => unreachable!(),
            }
            // Bypass encode's validation to exercise the public token trust boundary.
            let bytes = canonicalize_json_v1(&altered).unwrap();
            let encoded = format!(
                "{}.{:x}",
                encode_bytes(bytes.as_bytes()),
                Sha256::digest(bytes.as_bytes())
            );
            assert_eq!(
                ResetToken::decode(&encoded).unwrap_err().reason,
                RuntimeResetReasonV1::InvalidToken,
                "mutation {mutation}"
            );
        }
    }

    #[test]
    fn canonical_tokens_preserve_raw_paths_and_reject_alternate_encodings() {
        let token = token();
        let encoded = token.encode().unwrap();
        assert_eq!(ResetToken::decode(&encoded).unwrap(), token);
        assert_eq!(
            decode_bytes(&token.account_root.path_bytes_base64url).unwrap(),
            b"/account-\xff/.podway"
        );
        assert!(token.account_root.display.contains('\u{fffd}'));
        let (payload, digest) = encoded.split_once('.').unwrap();
        assert!(ResetToken::decode(&format!("{payload}=.{digest}")).is_err());
        assert!(ResetToken::decode(&format!("{payload}.{}", "0".repeat(64))).is_err());
        let mut document = serde_json::to_value(&token).unwrap();
        for field in ["process", "socket_identity"] {
            let mut altered = document.clone();
            altered["targets"][0].as_object_mut().unwrap().remove(field);
            let bytes = canonicalize_json_v1(&altered).unwrap();
            let encoded = format!(
                "{}.{:x}",
                encode_bytes(bytes.as_bytes()),
                Sha256::digest(bytes.as_bytes())
            );
            assert_eq!(
                ResetToken::decode(&encoded).unwrap_err().reason,
                RuntimeResetReasonV1::InvalidToken
            );
        }
        document
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_owned(), serde_json::Value::Null);
        let bytes = canonicalize_json_v1(&document).unwrap();
        let encoded = format!(
            "{}.{:x}",
            encode_bytes(bytes.as_bytes()),
            Sha256::digest(bytes.as_bytes())
        );
        assert!(ResetToken::decode(&encoded).is_err());
        let bytes = serde_json::to_string_pretty(&token).unwrap();
        let encoded = format!(
            "{}.{:x}",
            encode_bytes(bytes.as_bytes()),
            Sha256::digest(bytes.as_bytes())
        );
        assert!(ResetToken::decode(&encoded).is_err());
        for bytes in [vec![0], vec![0, 255], vec![255, 254, 253], vec![1, 2, 3, 4]] {
            assert_eq!(decode_bytes(&encode_bytes(&bytes)).unwrap(), bytes);
        }
        assert!(decode_bytes("AB").is_err());
        assert!(decode_bytes("AAB").is_err());
    }
}
