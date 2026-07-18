//! Private, metadata-only workspace-registry persistence for the daemon.
//!
//! The registry is deliberately not an identity, Store, or scheduler authority. It remembers only
//! the last validated root and observation time for a Store-authoritative workspace UUID.

use std::{
    error::Error,
    fmt,
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::observability::{EventCategoryV1, ObservabilityV1, SeverityV1};
use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg, OFlag, open},
    sys::stat::Mode,
    unistd::{getuid, mkdir},
};
use podway_core::{WorkspaceId, canonicalize_json_v1, verify_canonical_json_v1};
use podway_protocol::Rfc3339MillisV1;
use podway_service::ServiceRuntimePathsV1;
use podway_store::ValidatedWorkspaceRootV1;
use serde::{Deserialize, Serialize};

/// The only on-disk schema accepted by the workspace registry.
pub const WORKSPACE_REGISTRY_SCHEMA_V1: &str = "podway.registry/v1";
/// The registry is bounded so startup recovery cannot be amplified by a corrupt local file.
pub const MAX_WORKSPACE_REGISTRY_ENTRIES_V1: usize = 10_000;
/// A registry document includes only metadata and is deliberately bounded before JSON parsing.
pub const MAX_WORKSPACE_REGISTRY_BYTES_V1: u64 = 4 * 1024 * 1024;

const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const MAX_WORKSPACE_ROOT_TEXT_BYTES_V1: usize = 4_096;
const TEMPORARY_NAME_ATTEMPTS_V1: usize = 128;

static TEMPORARY_SEQUENCE_V1: AtomicU64 = AtomicU64::new(0);

/// A single metadata observation in the minimal registry schema.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceRegistryEntryV1 {
    workspace_uuid: WorkspaceId,
    last_known_root: ValidatedWorkspaceRootV1,
    last_seen_at: Rfc3339MillisV1,
}

impl WorkspaceRegistryEntryV1 {
    pub fn new(
        workspace_uuid: WorkspaceId,
        last_known_root: ValidatedWorkspaceRootV1,
        last_seen_at: Rfc3339MillisV1,
    ) -> Result<Self, WorkspaceRegistryValidationErrorV1> {
        if last_known_root.as_encoded().len() > MAX_WORKSPACE_ROOT_TEXT_BYTES_V1 {
            return Err(WorkspaceRegistryValidationErrorV1::WorkspaceRootTooLong {
                maximum: MAX_WORKSPACE_ROOT_TEXT_BYTES_V1,
                actual: last_known_root.as_encoded().len(),
            });
        }

        Ok(Self {
            workspace_uuid,
            last_known_root,
            last_seen_at,
        })
    }

    pub fn workspace_uuid(&self) -> &WorkspaceId {
        &self.workspace_uuid
    }

    pub fn last_known_root(&self) -> &ValidatedWorkspaceRootV1 {
        &self.last_known_root
    }

    pub fn last_seen_at(&self) -> &Rfc3339MillisV1 {
        &self.last_seen_at
    }
}

/// The complete minimal registry document. Entries are strictly sorted by workspace UUID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceRegistryV1 {
    workspaces: Vec<WorkspaceRegistryEntryV1>,
}

impl WorkspaceRegistryV1 {
    pub const fn empty() -> Self {
        Self {
            workspaces: Vec::new(),
        }
    }

    pub fn new(
        workspaces: Vec<WorkspaceRegistryEntryV1>,
    ) -> Result<Self, WorkspaceRegistryValidationErrorV1> {
        if workspaces.len() > MAX_WORKSPACE_REGISTRY_ENTRIES_V1 {
            return Err(WorkspaceRegistryValidationErrorV1::TooManyWorkspaces {
                maximum: MAX_WORKSPACE_REGISTRY_ENTRIES_V1,
                actual: workspaces.len(),
            });
        }

        for entry in &workspaces {
            if entry.last_known_root.as_encoded().len() > MAX_WORKSPACE_ROOT_TEXT_BYTES_V1 {
                return Err(WorkspaceRegistryValidationErrorV1::WorkspaceRootTooLong {
                    maximum: MAX_WORKSPACE_ROOT_TEXT_BYTES_V1,
                    actual: entry.last_known_root.as_encoded().len(),
                });
            }
        }

        if workspaces
            .windows(2)
            .any(|pair| pair[0].workspace_uuid >= pair[1].workspace_uuid)
        {
            return Err(WorkspaceRegistryValidationErrorV1::WorkspacesNotStrictlySorted);
        }

        Ok(Self { workspaces })
    }

    pub fn workspaces(&self) -> &[WorkspaceRegistryEntryV1] {
        &self.workspaces
    }

    pub fn lookup(&self, workspace_uuid: &WorkspaceId) -> Option<&WorkspaceRegistryEntryV1> {
        self.workspaces
            .binary_search_by(|entry| entry.workspace_uuid.cmp(workspace_uuid))
            .ok()
            .map(|index| &self.workspaces[index])
    }
}

/// In-memory registry validation errors, including the ordered-unique entry invariant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceRegistryValidationErrorV1 {
    TooManyWorkspaces { maximum: usize, actual: usize },
    WorkspaceRootTooLong { maximum: usize, actual: usize },
    WorkspacesNotStrictlySorted,
}

impl fmt::Display for WorkspaceRegistryValidationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyWorkspaces { maximum, actual } => {
                write!(
                    formatter,
                    "registry has {actual} workspaces, maximum is {maximum}"
                )
            }
            Self::WorkspaceRootTooLong { maximum, actual } => {
                write!(
                    formatter,
                    "workspace root text is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::WorkspacesNotStrictlySorted => {
                formatter.write_str("registry workspaces are not strictly sorted by UUID")
            }
        }
    }
}

impl Error for WorkspaceRegistryValidationErrorV1 {}

/// A filesystem property that makes a service-owned registry path unsafe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryPathViolationV1 {
    Symlink,
    NotDirectory,
    NotRegularFile,
    WrongOwner {
        expected_uid: u32,
        actual_uid: u32,
    },
    WrongMode {
        expected_mode: u32,
        actual_mode: u32,
    },
}

impl fmt::Display for RegistryPathViolationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Symlink => formatter.write_str("is a symlink"),
            Self::NotDirectory => formatter.write_str("is not a directory"),
            Self::NotRegularFile => formatter.write_str("is not a regular file"),
            Self::WrongOwner {
                expected_uid,
                actual_uid,
            } => write!(
                formatter,
                "has owner UID {actual_uid}, expected current UID {expected_uid}"
            ),
            Self::WrongMode {
                expected_mode,
                actual_mode,
            } => write!(
                formatter,
                "has mode {actual_mode:o}, expected {expected_mode:o}"
            ),
        }
    }
}

/// The reason an existing registry document is rejected instead of repaired or tolerated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryDocumentViolationV1 {
    InvalidJson,
    NonCanonicalJson,
    InvalidShape,
    UnsupportedSchema,
    InvalidWorkspaceRoot,
    InvalidTimestamp,
    InvalidEntryOrder,
}

impl fmt::Display for RegistryDocumentViolationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson => formatter.write_str("is not valid JSON"),
            Self::NonCanonicalJson => formatter.write_str("is not canonical JSON"),
            Self::InvalidShape => formatter.write_str("does not match the strict registry shape"),
            Self::UnsupportedSchema => formatter.write_str("has an unsupported schema"),
            Self::InvalidWorkspaceRoot => formatter.write_str("has an invalid workspace root"),
            Self::InvalidTimestamp => formatter.write_str("has an invalid timestamp"),
            Self::InvalidEntryOrder => {
                formatter.write_str("has invalid workspace ordering or bounds")
            }
        }
    }
}

/// Deterministic publication boundaries available only through store construction for tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryFailpointV1 {
    BeforeRename,
    AfterRenameBeforeParentSync,
}

/// The test-only behavior injected at a named registry publication boundary.
#[derive(Clone)]
pub enum RegistryFailpointActionV1 {
    ReturnError,
    /// Abort immediately at the configured boundary; use only in isolated child-process fixtures.
    AbortProcess,
    Barrier(Arc<Barrier>),
}

/// Failures from strict registry I/O and metadata operations.
#[derive(Debug)]
pub enum RegistryErrorV1 {
    RegistryPathHasNoParent {
        path: PathBuf,
    },
    UnsafeRegistryParent {
        path: PathBuf,
        violation: RegistryPathViolationV1,
    },
    UnsafeRegistryFile {
        path: PathBuf,
        violation: RegistryPathViolationV1,
    },
    UnsafeRegistryLock {
        path: PathBuf,
        violation: RegistryPathViolationV1,
    },
    UnsafeRegistryTemporary {
        path: PathBuf,
        violation: RegistryPathViolationV1,
    },
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    ParentCreate {
        path: PathBuf,
        source: Errno,
    },
    RegistryOpen {
        path: PathBuf,
        source: Errno,
    },
    LockOpen {
        path: PathBuf,
        source: Errno,
    },
    LockAcquire {
        path: PathBuf,
        source: Errno,
    },
    LockRelease {
        path: PathBuf,
        source: Errno,
    },
    RegistryTooLarge {
        path: PathBuf,
        maximum: u64,
        actual: u64,
    },
    InvalidRegistryDocument {
        path: PathBuf,
        violation: RegistryDocumentViolationV1,
    },
    RegistryValidation(WorkspaceRegistryValidationErrorV1),
    WorkspaceRootConflict {
        workspace_uuid: WorkspaceId,
        registered_root: ValidatedWorkspaceRootV1,
        supplied_root: ValidatedWorkspaceRootV1,
    },
    WorkspaceNotRegistered {
        workspace_uuid: WorkspaceId,
    },
    TemporaryNameExhausted {
        parent: PathBuf,
    },
    Failpoint {
        point: RegistryFailpointV1,
    },
    InProcessLockPoisoned,
}

impl fmt::Display for RegistryErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RegistryPathHasNoParent { path } => {
                write!(formatter, "registry path {} has no parent", path.display())
            }
            Self::UnsafeRegistryParent { path, violation } => {
                write!(formatter, "registry parent {} {violation}", path.display())
            }
            Self::UnsafeRegistryFile { path, violation } => {
                write!(formatter, "registry file {} {violation}", path.display())
            }
            Self::UnsafeRegistryLock { path, violation } => {
                write!(formatter, "registry lock {} {violation}", path.display())
            }
            Self::UnsafeRegistryTemporary { path, violation } => {
                write!(
                    formatter,
                    "registry temporary {} {violation}",
                    path.display()
                )
            }
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "cannot {operation} {}: {source}", path.display()),
            Self::ParentCreate { path, source } => {
                write!(
                    formatter,
                    "cannot create registry parent {}: {source}",
                    path.display()
                )
            }
            Self::RegistryOpen { path, source } => {
                write!(
                    formatter,
                    "cannot open registry {}: {source}",
                    path.display()
                )
            }
            Self::LockOpen { path, source } => {
                write!(
                    formatter,
                    "cannot open registry lock {}: {source}",
                    path.display()
                )
            }
            Self::LockAcquire { path, source } => {
                write!(
                    formatter,
                    "cannot lock registry {}: {source}",
                    path.display()
                )
            }
            Self::LockRelease { path, source } => {
                write!(
                    formatter,
                    "cannot unlock registry {}: {source}",
                    path.display()
                )
            }
            Self::RegistryTooLarge {
                path,
                maximum,
                actual,
            } => write!(
                formatter,
                "registry {} is {actual} bytes, maximum is {maximum}",
                path.display()
            ),
            Self::InvalidRegistryDocument { path, violation } => {
                write!(formatter, "registry {} {violation}", path.display())
            }
            Self::RegistryValidation(error) => error.fmt(formatter),
            Self::WorkspaceRootConflict {
                workspace_uuid,
                registered_root,
                supplied_root,
            } => write!(
                formatter,
                "workspace {workspace_uuid} is registered at {}, not {}",
                registered_root.as_encoded(),
                supplied_root.as_encoded()
            ),
            Self::WorkspaceNotRegistered { workspace_uuid } => {
                write!(formatter, "workspace {workspace_uuid} is not registered")
            }
            Self::TemporaryNameExhausted { parent } => write!(
                formatter,
                "could not create a unique registry temporary in {}",
                parent.display()
            ),
            Self::Failpoint { point } => {
                write!(formatter, "registry failpoint {point:?} triggered")
            }
            Self::InProcessLockPoisoned => {
                formatter.write_str("registry in-process lock is poisoned")
            }
        }
    }
}

impl Error for RegistryErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::ParentCreate { source, .. }
            | Self::RegistryOpen { source, .. }
            | Self::LockOpen { source, .. }
            | Self::LockAcquire { source, .. }
            | Self::LockRelease { source, .. } => Some(source),
            Self::RegistryValidation(error) => Some(error),
            _ => None,
        }
    }
}

/// A service-path-owned, fail-closed registry store.
///
/// Construction accepts `ServiceRuntimePathsV1` rather than an arbitrary filesystem path so this
/// module cannot become a workspace-path authority. Failpoints are construction-time test seams,
/// never request parameters.
pub struct RegistryStoreV1 {
    registry_path: PathBuf,
    failpoint: Option<RegistryFailpointV1>,
    failpoint_action: RegistryFailpointActionV1,
    in_process_lock: Mutex<()>,
    observability: Option<Arc<Mutex<ObservabilityV1>>>,
}

impl RegistryStoreV1 {
    pub(crate) fn new(paths: &ServiceRuntimePathsV1) -> Self {
        Self::with_observability(paths, None)
    }

    pub(crate) fn with_observability(
        paths: &ServiceRuntimePathsV1,
        observability: Option<Arc<Mutex<ObservabilityV1>>>,
    ) -> Self {
        Self::with_optional_failpoint(
            paths,
            None,
            RegistryFailpointActionV1::ReturnError,
            observability,
        )
    }

    #[cfg(test)]
    #[allow(dead_code)]
    /// Constructs a store with an injected publication boundary for deterministic tests.
    pub(crate) fn with_failpoint(
        paths: &ServiceRuntimePathsV1,
        failpoint: RegistryFailpointV1,
        failpoint_action: RegistryFailpointActionV1,
    ) -> Self {
        Self::with_optional_failpoint(paths, Some(failpoint), failpoint_action, None)
    }

    fn with_optional_failpoint(
        paths: &ServiceRuntimePathsV1,
        failpoint: Option<RegistryFailpointV1>,
        failpoint_action: RegistryFailpointActionV1,
        observability: Option<Arc<Mutex<ObservabilityV1>>>,
    ) -> Self {
        Self {
            registry_path: paths.workspace_registry_path().as_path().to_path_buf(),
            failpoint,
            failpoint_action,
            in_process_lock: Mutex::new(()),
            observability,
        }
    }

    pub fn registry_path(&self) -> &Path {
        &self.registry_path
    }

    /// Loads the complete strict document. A missing file is the empty registry.
    pub fn load(&self) -> Result<WorkspaceRegistryV1, RegistryErrorV1> {
        let result = self.with_locked(|parent, current_uid| {
            read_registry_v1(&self.registry_path, parent, current_uid)
        });
        self.emit(
            EventCategoryV1::MigrationOrIntegrity,
            if result.is_ok() {
                SeverityV1::Debug
            } else {
                SeverityV1::Warn
            },
        );
        result
    }

    /// Performs an exact UUID lookup against a consistently loaded registry document.
    pub fn lookup(
        &self,
        workspace_uuid: &WorkspaceId,
    ) -> Result<Option<WorkspaceRegistryEntryV1>, RegistryErrorV1> {
        self.load()
            .map(|registry| registry.lookup(workspace_uuid).cloned())
    }

    /// Adds a new workspace observation or refreshes the timestamp for the exact same root.
    ///
    /// A UUID associated with a different root is rejected; callers must use
    /// [`Self::move_workspace`] with the exact previous root.
    pub(crate) fn insert_or_refresh(
        &self,
        workspace_uuid: WorkspaceId,
        last_known_root: ValidatedWorkspaceRootV1,
        last_seen_at: Rfc3339MillisV1,
    ) -> Result<WorkspaceRegistryEntryV1, RegistryErrorV1> {
        let entry = WorkspaceRegistryEntryV1::new(
            workspace_uuid.clone(),
            last_known_root.clone(),
            last_seen_at,
        )
        .map_err(RegistryErrorV1::RegistryValidation)?;

        self.with_locked(|parent, current_uid| {
            let mut registry = read_registry_v1(&self.registry_path, parent, current_uid)?;
            match registry
                .workspaces
                .binary_search_by(|current| current.workspace_uuid.cmp(&workspace_uuid))
            {
                Ok(index) => {
                    let current = &mut registry.workspaces[index];
                    if current.last_known_root != last_known_root {
                        return Err(RegistryErrorV1::WorkspaceRootConflict {
                            workspace_uuid,
                            registered_root: current.last_known_root.clone(),
                            supplied_root: last_known_root,
                        });
                    }
                    current.last_seen_at = entry.last_seen_at.clone();
                }
                Err(index) => {
                    if registry.workspaces.len() == MAX_WORKSPACE_REGISTRY_ENTRIES_V1 {
                        return Err(RegistryErrorV1::RegistryValidation(
                            WorkspaceRegistryValidationErrorV1::TooManyWorkspaces {
                                maximum: MAX_WORKSPACE_REGISTRY_ENTRIES_V1,
                                actual: registry.workspaces.len() + 1,
                            },
                        ));
                    }
                    registry.workspaces.insert(index, entry.clone());
                }
            }
            persist_registry_v1(self, parent, current_uid, &registry)?;
            Ok(entry)
        })
    }

    /// Moves one workspace only when its registry root exactly matches `previous_root`.
    pub(crate) fn move_workspace(
        &self,
        workspace_uuid: &WorkspaceId,
        previous_root: &ValidatedWorkspaceRootV1,
        new_root: ValidatedWorkspaceRootV1,
        last_seen_at: Rfc3339MillisV1,
    ) -> Result<WorkspaceRegistryEntryV1, RegistryErrorV1> {
        let updated = WorkspaceRegistryEntryV1::new(workspace_uuid.clone(), new_root, last_seen_at)
            .map_err(RegistryErrorV1::RegistryValidation)?;

        self.with_locked(|parent, current_uid| {
            let mut registry = read_registry_v1(&self.registry_path, parent, current_uid)?;
            let index = registry
                .workspaces
                .binary_search_by(|entry| entry.workspace_uuid.cmp(workspace_uuid))
                .map_err(|_| RegistryErrorV1::WorkspaceNotRegistered {
                    workspace_uuid: workspace_uuid.clone(),
                })?;
            let current = &registry.workspaces[index];
            if current.last_known_root != *previous_root {
                return Err(RegistryErrorV1::WorkspaceRootConflict {
                    workspace_uuid: workspace_uuid.clone(),
                    registered_root: current.last_known_root.clone(),
                    supplied_root: previous_root.clone(),
                });
            }
            registry.workspaces[index] = updated.clone();
            persist_registry_v1(self, parent, current_uid, &registry)?;
            Ok(updated)
        })
    }

    /// Atomically replaces the registered reset source UUID at `exact_root` with `target_uuid`.
    ///
    /// The reset marker is the recovery authority for this transition; this method only publishes
    /// the corresponding metadata observation. It never composes removal and insertion into two
    /// documents, so readers observe either the prior UUID or the reset target UUID at the root.
    pub(crate) fn replace_for_reset(
        &self,
        previous_uuid: &WorkspaceId,
        target_uuid: WorkspaceId,
        exact_root: ValidatedWorkspaceRootV1,
        seen_at: Rfc3339MillisV1,
    ) -> Result<WorkspaceRegistryEntryV1, RegistryErrorV1> {
        let target =
            WorkspaceRegistryEntryV1::new(target_uuid.clone(), exact_root.clone(), seen_at)
                .map_err(RegistryErrorV1::RegistryValidation)?;

        self.with_locked(|parent, current_uid| {
            let mut registry = read_registry_v1(&self.registry_path, parent, current_uid)?;
            let previous_index = registry
                .workspaces
                .binary_search_by(|entry| entry.workspace_uuid.cmp(previous_uuid));
            let target_index = registry
                .workspaces
                .binary_search_by(|entry| entry.workspace_uuid.cmp(&target_uuid));
            if let Some(conflict) = registry.workspaces.iter().find(|entry| {
                entry.last_known_root == exact_root
                    && entry.workspace_uuid != *previous_uuid
                    && entry.workspace_uuid != target_uuid
            }) {
                return Err(RegistryErrorV1::WorkspaceRootConflict {
                    workspace_uuid: target_uuid.clone(),
                    registered_root: conflict.last_known_root.clone(),
                    supplied_root: exact_root,
                });
            }

            if previous_uuid == &target_uuid {
                let index =
                    previous_index.map_err(|_| RegistryErrorV1::WorkspaceNotRegistered {
                        workspace_uuid: previous_uuid.clone(),
                    })?;
                {
                    let current = &mut registry.workspaces[index];
                    if current.last_known_root != exact_root {
                        return Err(RegistryErrorV1::WorkspaceRootConflict {
                            workspace_uuid: previous_uuid.clone(),
                            registered_root: current.last_known_root.clone(),
                            supplied_root: exact_root,
                        });
                    }
                    current.last_seen_at = target.last_seen_at.clone();
                }
                let updated = registry.workspaces[index].clone();
                persist_registry_v1(self, parent, current_uid, &registry)?;
                return Ok(updated);
            }

            if let Ok(index) = target_index {
                let current = &registry.workspaces[index];
                if current.last_known_root != exact_root {
                    return Err(RegistryErrorV1::WorkspaceRootConflict {
                        workspace_uuid: target_uuid,
                        registered_root: current.last_known_root.clone(),
                        supplied_root: exact_root,
                    });
                }
                if let Ok(previous_index) = previous_index {
                    let previous = &registry.workspaces[previous_index];
                    if previous.last_known_root != exact_root {
                        return Err(RegistryErrorV1::WorkspaceRootConflict {
                            workspace_uuid: previous_uuid.clone(),
                            registered_root: previous.last_known_root.clone(),
                            supplied_root: exact_root,
                        });
                    }
                    registry.workspaces.remove(previous_index);
                    let index = registry
                        .workspaces
                        .binary_search_by(|entry| entry.workspace_uuid.cmp(&target_uuid))
                        .expect("target entry remains after removing a different UUID");
                    registry.workspaces[index].last_seen_at = target.last_seen_at.clone();
                } else {
                    registry.workspaces[index].last_seen_at = target.last_seen_at.clone();
                }
                persist_registry_v1(self, parent, current_uid, &registry)?;
                return Ok(target);
            }

            let previous_index =
                previous_index.map_err(|_| RegistryErrorV1::WorkspaceNotRegistered {
                    workspace_uuid: previous_uuid.clone(),
                })?;
            let previous = &registry.workspaces[previous_index];
            if previous.last_known_root != exact_root {
                return Err(RegistryErrorV1::WorkspaceRootConflict {
                    workspace_uuid: previous_uuid.clone(),
                    registered_root: previous.last_known_root.clone(),
                    supplied_root: exact_root,
                });
            }

            registry.workspaces.remove(previous_index);
            let target_index = registry
                .workspaces
                .binary_search_by(|entry| entry.workspace_uuid.cmp(&target_uuid))
                .unwrap_or_else(|index| index);
            registry.workspaces.insert(target_index, target.clone());
            persist_registry_v1(self, parent, current_uid, &registry)?;
            Ok(target)
        })
    }
    #[cfg(test)]
    #[allow(dead_code)]
    /// Removes the exact UUID metadata entry, returning it when it existed.
    pub(crate) fn remove(
        &self,
        workspace_uuid: &WorkspaceId,
    ) -> Result<Option<WorkspaceRegistryEntryV1>, RegistryErrorV1> {
        self.with_locked(|parent, current_uid| {
            let mut registry = read_registry_v1(&self.registry_path, parent, current_uid)?;
            let Ok(index) = registry
                .workspaces
                .binary_search_by(|entry| entry.workspace_uuid.cmp(workspace_uuid))
            else {
                return Ok(None);
            };
            let removed = registry.workspaces.remove(index);
            persist_registry_v1(self, parent, current_uid, &registry)?;
            Ok(Some(removed))
        })
    }

    fn with_locked<T>(
        &self,
        operation: impl FnOnce(&Path, u32) -> Result<T, RegistryErrorV1>,
    ) -> Result<T, RegistryErrorV1> {
        let _in_process_guard = self
            .in_process_lock
            .lock()
            .map_err(|_| RegistryErrorV1::InProcessLockPoisoned)?;
        let parent = self.parent_directory()?;
        let current_uid = getuid().as_raw();
        ensure_private_parent_v1(&parent, current_uid)?;

        let lock_path = self.lock_path(&parent)?;
        let lock_file = open_registry_lock_v1(&lock_path, current_uid)?;
        let lock = Flock::lock(lock_file, FlockArg::LockExclusive).map_err(|(_, source)| {
            RegistryErrorV1::LockAcquire {
                path: lock_path.clone(),
                source,
            }
        })?;

        let result = (|| {
            ensure_private_parent_v1(&parent, current_uid)?;
            operation(&parent, current_uid)
        })();
        if result.is_err() {
            self.emit(EventCategoryV1::MigrationOrIntegrity, SeverityV1::Warn);
        }
        let unlock = Flock::unlock(lock).map_err(|(_, source)| RegistryErrorV1::LockRelease {
            path: lock_path,
            source,
        });

        match (result, unlock) {
            (Ok(value), Ok(file)) => {
                drop(file);
                Ok(value)
            }
            (Err(error), Ok(file)) => {
                drop(file);
                Err(error)
            }
            (Ok(_), Err(error)) | (Err(_), Err(error)) => Err(error),
        }
    }

    fn parent_directory(&self) -> Result<PathBuf, RegistryErrorV1> {
        self.registry_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| RegistryErrorV1::RegistryPathHasNoParent {
                path: self.registry_path.clone(),
            })
    }

    fn lock_path(&self, parent: &Path) -> Result<PathBuf, RegistryErrorV1> {
        let file_name = self.registry_path.file_name().ok_or_else(|| {
            RegistryErrorV1::RegistryPathHasNoParent {
                path: self.registry_path.clone(),
            }
        })?;
        let mut lock_name = file_name.to_os_string();
        lock_name.push(".lock");
        Ok(parent.join(lock_name))
    }

    fn emit(&self, category: EventCategoryV1, severity: SeverityV1) {
        let observer = self
            .observability
            .as_ref()
            .and_then(|observability| observability.try_lock().ok());
        if let Some(observability) = observer {
            observability.emit(category, severity);
        }
    }
    fn trigger_failpoint(&self, point: RegistryFailpointV1) -> Result<(), RegistryErrorV1> {
        if self.failpoint != Some(point) {
            return Ok(());
        }
        match &self.failpoint_action {
            RegistryFailpointActionV1::ReturnError => Err(RegistryErrorV1::Failpoint { point }),
            RegistryFailpointActionV1::AbortProcess => std::process::abort(),
            RegistryFailpointActionV1::Barrier(barrier) => {
                barrier.wait();
                Ok(())
            }
        }
    }
}

fn ensure_private_parent_v1(path: &Path, current_uid: u32) -> Result<(), RegistryErrorV1> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(source) if source.kind() == io::ErrorKind::NotFound => match mkdir(path, Mode::S_IRWXU)
        {
            Ok(()) | Err(Errno::EEXIST) => {}
            Err(source) => {
                return Err(RegistryErrorV1::ParentCreate {
                    path: path.to_path_buf(),
                    source,
                });
            }
        },
        Err(source) => {
            return Err(RegistryErrorV1::Io {
                operation: "inspect registry parent",
                path: path.to_path_buf(),
                source,
            });
        }
    }

    let metadata = fs::symlink_metadata(path).map_err(|source| RegistryErrorV1::Io {
        operation: "inspect registry parent",
        path: path.to_path_buf(),
        source,
    })?;
    validate_parent_v1(path, &metadata, current_uid)
}

fn open_registry_lock_v1(path: &Path, current_uid: u32) -> Result<File, RegistryErrorV1> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        validate_lock_file_v1(path, &metadata, current_uid)?;
    }

    let descriptor = open(
        path,
        OFlag::O_CLOEXEC | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_RDWR,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .map_err(|source| RegistryErrorV1::LockOpen {
        path: path.to_path_buf(),
        source,
    })?;
    let file = File::from(descriptor);
    let metadata = file.metadata().map_err(|source| RegistryErrorV1::Io {
        operation: "inspect registry lock",
        path: path.to_path_buf(),
        source,
    })?;
    validate_lock_file_v1(path, &metadata, current_uid)?;
    Ok(file)
}

fn read_registry_v1(
    registry_path: &Path,
    _parent: &Path,
    current_uid: u32,
) -> Result<WorkspaceRegistryV1, RegistryErrorV1> {
    let metadata = match fs::symlink_metadata(registry_path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(WorkspaceRegistryV1::empty());
        }
        Err(source) => {
            return Err(RegistryErrorV1::Io {
                operation: "inspect registry file",
                path: registry_path.to_path_buf(),
                source,
            });
        }
    };
    validate_registry_file_v1(registry_path, &metadata, current_uid)?;

    let descriptor = open(
        registry_path,
        OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .map_err(|source| RegistryErrorV1::RegistryOpen {
        path: registry_path.to_path_buf(),
        source,
    })?;
    let file = File::from(descriptor);
    let opened_metadata = file.metadata().map_err(|source| RegistryErrorV1::Io {
        operation: "inspect open registry file",
        path: registry_path.to_path_buf(),
        source,
    })?;
    validate_registry_file_v1(registry_path, &opened_metadata, current_uid)?;
    if opened_metadata.len() > MAX_WORKSPACE_REGISTRY_BYTES_V1 {
        return Err(RegistryErrorV1::RegistryTooLarge {
            path: registry_path.to_path_buf(),
            maximum: MAX_WORKSPACE_REGISTRY_BYTES_V1,
            actual: opened_metadata.len(),
        });
    }

    let mut bytes = Vec::with_capacity(opened_metadata.len() as usize);
    file.take(MAX_WORKSPACE_REGISTRY_BYTES_V1 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| RegistryErrorV1::Io {
            operation: "read registry file",
            path: registry_path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > MAX_WORKSPACE_REGISTRY_BYTES_V1 {
        return Err(RegistryErrorV1::RegistryTooLarge {
            path: registry_path.to_path_buf(),
            maximum: MAX_WORKSPACE_REGISTRY_BYTES_V1,
            actual: bytes.len() as u64,
        });
    }

    decode_registry_v1(registry_path, &bytes)
}

fn decode_registry_v1(
    registry_path: &Path,
    bytes: &[u8],
) -> Result<WorkspaceRegistryV1, RegistryErrorV1> {
    let canonical_violation = match verify_canonical_json_v1(bytes) {
        Ok(()) => None,
        Err(podway_core::CanonicalJsonErrorV1::InvalidJson(_)) => {
            Some(RegistryDocumentViolationV1::InvalidJson)
        }
        Err(_) => Some(RegistryDocumentViolationV1::NonCanonicalJson),
    };
    if let Some(violation) = canonical_violation {
        return Err(RegistryErrorV1::InvalidRegistryDocument {
            path: registry_path.to_path_buf(),
            violation,
        });
    }

    let document: SerializedRegistryV1 =
        serde_json::from_slice(bytes).map_err(|_| RegistryErrorV1::InvalidRegistryDocument {
            path: registry_path.to_path_buf(),
            violation: RegistryDocumentViolationV1::InvalidShape,
        })?;
    if document.schema != WORKSPACE_REGISTRY_SCHEMA_V1 {
        return Err(RegistryErrorV1::InvalidRegistryDocument {
            path: registry_path.to_path_buf(),
            violation: RegistryDocumentViolationV1::UnsupportedSchema,
        });
    }

    let mut entries = Vec::with_capacity(document.workspaces.len());
    for entry in document.workspaces {
        let workspace_uuid = WorkspaceId::new(entry.workspace_uuid).map_err(|_| {
            RegistryErrorV1::InvalidRegistryDocument {
                path: registry_path.to_path_buf(),
                violation: RegistryDocumentViolationV1::InvalidShape,
            }
        })?;
        let last_known_root = ValidatedWorkspaceRootV1::from_encoded(entry.last_known_root)
            .map_err(|_| RegistryErrorV1::InvalidRegistryDocument {
                path: registry_path.to_path_buf(),
                violation: RegistryDocumentViolationV1::InvalidWorkspaceRoot,
            })?;
        let last_seen_at = Rfc3339MillisV1::new(entry.last_seen_at).map_err(|_| {
            RegistryErrorV1::InvalidRegistryDocument {
                path: registry_path.to_path_buf(),
                violation: RegistryDocumentViolationV1::InvalidTimestamp,
            }
        })?;
        entries.push(
            WorkspaceRegistryEntryV1::new(workspace_uuid, last_known_root, last_seen_at).map_err(
                |_| RegistryErrorV1::InvalidRegistryDocument {
                    path: registry_path.to_path_buf(),
                    violation: RegistryDocumentViolationV1::InvalidEntryOrder,
                },
            )?,
        );
    }

    WorkspaceRegistryV1::new(entries).map_err(|_| RegistryErrorV1::InvalidRegistryDocument {
        path: registry_path.to_path_buf(),
        violation: RegistryDocumentViolationV1::InvalidEntryOrder,
    })
}

fn persist_registry_v1(
    store: &RegistryStoreV1,
    parent: &Path,
    current_uid: u32,
    registry: &WorkspaceRegistryV1,
) -> Result<(), RegistryErrorV1> {
    let bytes = encode_registry_v1(registry)?;
    let temporary = create_temporary_v1(parent, current_uid)?;
    let temporary_path = temporary.path.clone();

    let mut file = temporary.file;
    if let Err(source) = file.write_all(&bytes) {
        drop(file);
        remove_owned_temporary_v1(&temporary_path, temporary.identity);
        return Err(RegistryErrorV1::Io {
            operation: "write registry temporary",
            path: temporary_path,
            source,
        });
    }
    if let Err(source) = file.sync_all() {
        drop(file);
        remove_owned_temporary_v1(&temporary_path, temporary.identity);
        return Err(RegistryErrorV1::Io {
            operation: "sync registry temporary",
            path: temporary_path,
            source,
        });
    }
    if let Err(error) = store.trigger_failpoint(RegistryFailpointV1::BeforeRename) {
        drop(file);
        remove_owned_temporary_v1(&temporary_path, temporary.identity);
        return Err(error);
    }

    drop(file);
    if let Err(source) = fs::rename(&temporary_path, &store.registry_path) {
        remove_owned_temporary_v1(&temporary_path, temporary.identity);
        return Err(RegistryErrorV1::Io {
            operation: "atomically rename registry temporary",
            path: store.registry_path.clone(),
            source,
        });
    }
    store.trigger_failpoint(RegistryFailpointV1::AfterRenameBeforeParentSync)?;
    sync_parent_v1(parent)?;
    store.emit(EventCategoryV1::MoveOrRepair, SeverityV1::Info);
    Ok(())
}

fn encode_registry_v1(registry: &WorkspaceRegistryV1) -> Result<Vec<u8>, RegistryErrorV1> {
    let document = SerializableRegistryRefV1 {
        schema: WORKSPACE_REGISTRY_SCHEMA_V1,
        workspaces: registry
            .workspaces
            .iter()
            .map(SerializableEntryRefV1::from)
            .collect(),
    };
    canonicalize_json_v1(&document)
        .map(String::into_bytes)
        .map_err(|_| {
            RegistryErrorV1::RegistryValidation(
                WorkspaceRegistryValidationErrorV1::WorkspacesNotStrictlySorted,
            )
        })
}

fn create_temporary_v1(
    parent: &Path,
    current_uid: u32,
) -> Result<TemporaryRegistryFileV1, RegistryErrorV1> {
    for _ in 0..TEMPORARY_NAME_ATTEMPTS_V1 {
        let sequence = TEMPORARY_SEQUENCE_V1.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".podway-registry-v1-{}-{sequence}.tmp",
            std::process::id()
        ));
        let descriptor = match open(
            &path,
            OFlag::O_CLOEXEC | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_WRONLY,
            Mode::S_IRUSR | Mode::S_IWUSR,
        ) {
            Ok(descriptor) => descriptor,
            Err(Errno::EEXIST) => continue,
            Err(source) => {
                return Err(RegistryErrorV1::Io {
                    operation: "create registry temporary",
                    path,
                    source: source.into(),
                });
            }
        };
        let file = File::from(descriptor);
        let metadata = file.metadata().map_err(|source| RegistryErrorV1::Io {
            operation: "inspect registry temporary",
            path: path.clone(),
            source,
        })?;
        validate_temporary_file_v1(&path, &metadata, current_uid)?;
        return Ok(TemporaryRegistryFileV1 {
            file,
            path,
            identity: FileIdentityV1::from_metadata(&metadata),
        });
    }

    Err(RegistryErrorV1::TemporaryNameExhausted {
        parent: parent.to_path_buf(),
    })
}

fn remove_owned_temporary_v1(path: &Path, identity: FileIdentityV1) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_file()
        && metadata.uid() == identity.owner_uid
        && metadata.dev() == identity.device
        && metadata.ino() == identity.inode
    {
        let _ = fs::remove_file(path);
    }
}

fn sync_parent_v1(parent: &Path) -> Result<(), RegistryErrorV1> {
    let descriptor = open(
        parent,
        OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .map_err(|source| RegistryErrorV1::Io {
        operation: "open registry parent for sync",
        path: parent.to_path_buf(),
        source: source.into(),
    })?;
    File::from(descriptor)
        .sync_all()
        .map_err(|source| RegistryErrorV1::Io {
            operation: "sync registry parent",
            path: parent.to_path_buf(),
            source,
        })
}

fn validate_parent_v1(
    path: &Path,
    metadata: &Metadata,
    current_uid: u32,
) -> Result<(), RegistryErrorV1> {
    let violation = if metadata.file_type().is_symlink() {
        Some(RegistryPathViolationV1::Symlink)
    } else if !metadata.is_dir() {
        Some(RegistryPathViolationV1::NotDirectory)
    } else if metadata.uid() != current_uid {
        Some(RegistryPathViolationV1::WrongOwner {
            expected_uid: current_uid,
            actual_uid: metadata.uid(),
        })
    } else {
        let actual_mode = metadata.permissions().mode() & 0o777;
        (actual_mode != PRIVATE_DIRECTORY_MODE).then_some(RegistryPathViolationV1::WrongMode {
            expected_mode: PRIVATE_DIRECTORY_MODE,
            actual_mode,
        })
    };
    violation.map_or(Ok(()), |violation| {
        Err(RegistryErrorV1::UnsafeRegistryParent {
            path: path.to_path_buf(),
            violation,
        })
    })
}

fn validate_registry_file_v1(
    path: &Path,
    metadata: &Metadata,
    current_uid: u32,
) -> Result<(), RegistryErrorV1> {
    validate_file_v1(path, metadata, current_uid, |path, violation| {
        RegistryErrorV1::UnsafeRegistryFile { path, violation }
    })
}

fn validate_lock_file_v1(
    path: &Path,
    metadata: &Metadata,
    current_uid: u32,
) -> Result<(), RegistryErrorV1> {
    validate_file_v1(path, metadata, current_uid, |path, violation| {
        RegistryErrorV1::UnsafeRegistryLock { path, violation }
    })
}

fn validate_temporary_file_v1(
    path: &Path,
    metadata: &Metadata,
    current_uid: u32,
) -> Result<(), RegistryErrorV1> {
    validate_file_v1(path, metadata, current_uid, |path, violation| {
        RegistryErrorV1::UnsafeRegistryTemporary { path, violation }
    })
}

fn validate_file_v1(
    path: &Path,
    metadata: &Metadata,
    current_uid: u32,
    error: impl FnOnce(PathBuf, RegistryPathViolationV1) -> RegistryErrorV1,
) -> Result<(), RegistryErrorV1> {
    let violation = if metadata.file_type().is_symlink() {
        Some(RegistryPathViolationV1::Symlink)
    } else if !metadata.file_type().is_file() {
        Some(RegistryPathViolationV1::NotRegularFile)
    } else if metadata.uid() != current_uid {
        Some(RegistryPathViolationV1::WrongOwner {
            expected_uid: current_uid,
            actual_uid: metadata.uid(),
        })
    } else {
        let actual_mode = metadata.permissions().mode() & 0o777;
        (actual_mode != PRIVATE_FILE_MODE).then_some(RegistryPathViolationV1::WrongMode {
            expected_mode: PRIVATE_FILE_MODE,
            actual_mode,
        })
    };
    violation.map_or(Ok(()), |violation| {
        Err(error(path.to_path_buf(), violation))
    })
}

#[derive(Clone, Copy)]
struct FileIdentityV1 {
    device: u64,
    inode: u64,
    owner_uid: u32,
}

impl FileIdentityV1 {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner_uid: metadata.uid(),
        }
    }
}

struct TemporaryRegistryFileV1 {
    file: File,
    path: PathBuf,
    identity: FileIdentityV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedRegistryV1 {
    schema: String,
    workspaces: Vec<SerializedEntryV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedEntryV1 {
    workspace_uuid: String,
    last_known_root: String,
    last_seen_at: String,
}

#[derive(Serialize)]
struct SerializableRegistryRefV1<'a> {
    schema: &'a str,
    workspaces: Vec<SerializableEntryRefV1<'a>>,
}

#[derive(Serialize)]
struct SerializableEntryRefV1<'a> {
    workspace_uuid: &'a WorkspaceId,
    last_known_root: &'a str,
    last_seen_at: &'a Rfc3339MillisV1,
}

impl<'a> From<&'a WorkspaceRegistryEntryV1> for SerializableEntryRefV1<'a> {
    fn from(entry: &'a WorkspaceRegistryEntryV1) -> Self {
        Self {
            workspace_uuid: &entry.workspace_uuid,
            last_known_root: entry.last_known_root.as_encoded(),
            last_seen_at: &entry.last_seen_at,
        }
    }
}
