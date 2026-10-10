//! Host-independent, read-only workspace resolution.
//!
//! Git first establishes a validated worktree snapshot. The Store binding is then inspected
//! without mutation, and Git is resolved again with the durable binding before a caller may open
//! or update SQLite. Paths remain diagnostic and routing data. The hosting caller can derive
//! execution identity from the Store-validated UUID and Git administration fingerprints.

use std::{
    error::Error,
    fmt,
    fs::{self, File, Metadata},
    io,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::{
    ffi::OsString,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
};

#[cfg(unix)]
use nix::{
    dir::Dir,
    errno::Errno,
    fcntl::{AtFlags, OFlag, open, openat},
    sys::stat::{Mode, SFlag},
    unistd::{UnlinkatFlags, getuid, unlinkat},
};
use podway_core::WorkspaceId;
use podway_git::{
    DiagnosticPathDisplayV1, DurableWorktreeIdentityV1 as GitWorktreeIdentityV1,
    GitInvariantViolationV1, GitResolveErrorV1, GitResolverContractV1, LosslessPathV1,
    NativeGitResolverV1, SelectorValidationErrorV1, ValidatedWorktreeV1, WorkspaceIdentityStateV1,
    WorktreeMoveMetadataV1, WorktreeSelectorV1,
};
use podway_store::{
    DurableWorktreeIdentityV1 as StoreWorktreeIdentityV1, StoreValueErrorV1,
    ValidatedWorkspaceRootV1, WorkspaceBindingV1,
    direct_store::{
        DirectStoreClassificationV1, DirectStoreErrorV1, DirectStoreV1, DirectWorkspaceLayoutV1,
        classify_direct_store_v1,
    },
};

const STATE_DATABASE_FILE_NAME_V1: &str = "state.sqlite3";
const STORED_ROOT_DIAGNOSTIC_V1: &str = "store-validated workspace root";
const MAX_WORKSPACE_REMOVAL_ENTRIES_V1: usize = 100_000;
const MAX_WORKSPACE_REMOVAL_DEPTH_V1: usize = 128;

#[derive(Debug)]
pub enum WorkspaceRemovalFilesystemErrorV1 {
    UnsupportedPlatform,
    Path {
        operation: &'static str,
        source: io::Error,
    },
    UnsafePath,
    MarkerMissing,
    MarkerChanged,
    MaintenanceBusy,
    NotEmptyResidual,
}

impl fmt::Display for WorkspaceRemovalFilesystemErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => formatter.write_str("workspace removal requires Unix"),
            Self::Path { operation, source } => write!(formatter, "cannot {operation}: {source}"),
            Self::UnsafePath => formatter.write_str("workspace removal path is unsafe"),
            Self::MarkerMissing => formatter.write_str("workspace removal marker is missing"),
            Self::MarkerChanged => formatter.write_str("workspace removal marker changed"),
            Self::MaintenanceBusy => formatter.write_str("workspace maintenance exclusion is held"),
            Self::NotEmptyResidual => {
                formatter.write_str("Podway directory is not an empty removal residual")
            }
        }
    }
}

impl Error for WorkspaceRemovalFilesystemErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RemovalObjectIdentityV1 {
    device: u64,
    inode: u64,
    owner_uid: u32,
}

#[cfg(unix)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct RemovalPathComponentV1 {
    name: OsString,
    identity: RemovalObjectIdentityV1,
}

#[cfg(unix)]
impl RemovalObjectIdentityV1 {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner_uid: metadata.uid(),
        }
    }

    fn matches_stat(self, stat: &nix::sys::stat::FileStat) -> bool {
        self.device == stat.st_dev as u64
            && self.inode == stat.st_ino
            && self.owner_uid == stat.st_uid
    }
}

/// A `.podway` directory opened relative to one freshly validated Git worktree root.
pub struct ValidatedPodwayRemovalDirectoryV1 {
    #[cfg(unix)]
    root: File,
    #[cfg(unix)]
    podway: File,
    #[cfg(unix)]
    podway_identity: RemovalObjectIdentityV1,
    #[cfg(unix)]
    current_uid: u32,
}

impl ValidatedPodwayRemovalDirectoryV1 {
    pub fn open(
        worktree: &ValidatedWorktreeV1,
    ) -> Result<Option<Self>, WorkspaceRemovalFilesystemErrorV1> {
        #[cfg(unix)]
        {
            let root_bytes = worktree
                .roots()
                .worktree_root()
                .decode_path_bytes()
                .map_err(|_| WorkspaceRemovalFilesystemErrorV1::UnsafePath)?;
            let root_path = PathBuf::from(OsString::from_vec(root_bytes));
            let root = File::from(
                open(
                    &root_path,
                    OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
                    Mode::empty(),
                )
                .map_err(|source| removal_path_error("open worktree root", source.into()))?,
            );
            NativeGitResolverV1::new()
                .validate_open_worktree_root(worktree, &root)
                .map_err(|_| WorkspaceRemovalFilesystemErrorV1::UnsafePath)?;
            let podway = match openat(
                &root,
                ".podway",
                OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
                Mode::empty(),
            ) {
                Ok(directory) => File::from(directory),
                Err(Errno::ENOENT) => return Ok(None),
                Err(Errno::ELOOP | Errno::ENOTDIR) => {
                    return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
                }
                Err(source) => {
                    return Err(removal_path_error("open .podway directory", source.into()));
                }
            };
            let root_metadata = root
                .metadata()
                .map_err(|source| removal_path_error("inspect worktree root", source))?;
            let podway_metadata = podway
                .metadata()
                .map_err(|source| removal_path_error("inspect .podway directory", source))?;
            let current_uid = getuid().as_raw();
            if !podway_metadata.is_dir()
                || podway_metadata.uid() != current_uid
                || podway_metadata.dev() != root_metadata.dev()
            {
                return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
            }
            Ok(Some(Self {
                root,
                podway_identity: RemovalObjectIdentityV1::from_metadata(&podway_metadata),
                podway,
                current_uid,
            }))
        }
        #[cfg(not(unix))]
        {
            let _ = worktree;
            Err(WorkspaceRemovalFilesystemErrorV1::UnsupportedPlatform)
        }
    }

    /// Direct maintenance retains its native exclusion files and typed markers.
    /// Only the top-level runtime entries are retained; matching nested names are removed.
    pub fn clear_direct_contents(
        &self,
        whole_podway: bool,
    ) -> Result<(), WorkspaceRemovalFilesystemErrorV1> {
        #[cfg(unix)]
        {
            let runtime = self
                .open_child_directory(&self.podway, "runtime")?
                .ok_or(WorkspaceRemovalFilesystemErrorV1::UnsafePath)?;
            let identity = RemovalObjectIdentityV1::from_metadata(
                &runtime
                    .metadata()
                    .map_err(|source| removal_path_error("inspect runtime directory", source))?,
            );
            let relative = [RemovalPathComponentV1 {
                name: OsString::from("runtime"),
                identity,
            }];
            let retained = remove_directory_contents_retaining_v1(
                &self.root,
                if whole_podway { &self.podway } else { &runtime },
                self.podway_identity,
                self.current_uid,
                self.podway_identity.device,
                if whole_podway { &[] } else { &relative },
                &mut RemovalTraversalBudgetV1::new(),
                &[
                    "maintenance.lock",
                    "maintenance-intent.lock",
                    "maintenance-intent.json",
                    "reset-all.marker",
                    "removal.marker",
                ],
            )?;
            if !retained {
                return Err(WorkspaceRemovalFilesystemErrorV1::MarkerMissing);
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = whole_podway;
            Err(WorkspaceRemovalFilesystemErrorV1::UnsupportedPlatform)
        }
    }

    /// A legacy configuration marker is removed only by an explicit reset-all.
    pub fn remove_direct_legacy_configuration_marker(
        &self,
    ) -> Result<(), WorkspaceRemovalFilesystemErrorV1> {
        #[cfg(unix)]
        {
            validate_removal_ancestry_v1(
                &self.root,
                &self.podway,
                self.podway_identity,
                self.current_uid,
                self.podway_identity.device,
                &[],
            )?;
            match unlinkat(&self.podway, "mode-switch.json", UnlinkatFlags::NoRemoveDir) {
                Ok(()) | Err(Errno::ENOENT) => {}
                Err(source) => {
                    return Err(removal_path_error(
                        "remove legacy configuration marker",
                        source.into(),
                    ));
                }
            }
            self.podway
                .sync_all()
                .map_err(|source| removal_path_error("sync .podway directory", source))
        }
        #[cfg(not(unix))]
        {
            Err(WorkspaceRemovalFilesystemErrorV1::UnsupportedPlatform)
        }
    }

    /// Keep the operation marker until the intent and named locks are durably removed.
    /// A concurrently repopulated directory is retained and fails closed.
    pub fn finish_direct_removal(
        &self,
        before_last_marker: impl FnOnce() -> Result<(), WorkspaceRemovalFilesystemErrorV1>,
        after_first_marker: impl FnOnce(),
        after_last_marker: impl FnOnce(),
        after_runtime_directory: impl FnOnce(),
    ) -> Result<(), WorkspaceRemovalFilesystemErrorV1> {
        #[cfg(unix)]
        {
            let runtime = self
                .open_child_directory(&self.podway, "runtime")?
                .ok_or(WorkspaceRemovalFilesystemErrorV1::MarkerMissing)?;
            let identity = RemovalObjectIdentityV1::from_metadata(
                &runtime
                    .metadata()
                    .map_err(|source| removal_path_error("inspect runtime directory", source))?,
            );
            let relative = [RemovalPathComponentV1 {
                name: OsString::from("runtime"),
                identity,
            }];
            let allowed = [
                "maintenance.lock",
                "maintenance-intent.lock",
                "maintenance-intent.json",
                "removal.marker",
            ];
            for name in removal_directory_entries(&runtime, &mut RemovalTraversalBudgetV1::new())? {
                if !allowed
                    .iter()
                    .any(|allowed| name.as_bytes() == allowed.as_bytes())
                {
                    return Err(WorkspaceRemovalFilesystemErrorV1::NotEmptyResidual);
                }
            }
            before_last_marker()?;
            for name in [
                "maintenance-intent.lock",
                "maintenance.lock",
                "maintenance-intent.json",
            ] {
                validate_removal_ancestry_v1(
                    &self.root,
                    &runtime,
                    self.podway_identity,
                    self.current_uid,
                    self.podway_identity.device,
                    &relative,
                )?;
                unlinkat(&runtime, name, UnlinkatFlags::NoRemoveDir).map_err(|source| {
                    removal_path_error("remove retained direct maintenance file", source.into())
                })?;
            }
            runtime
                .sync_all()
                .map_err(|source| removal_path_error("sync runtime directory", source))?;
            after_first_marker();
            validate_removal_ancestry_v1(
                &self.root,
                &runtime,
                self.podway_identity,
                self.current_uid,
                self.podway_identity.device,
                &relative,
            )?;
            unlinkat(&runtime, "removal.marker", UnlinkatFlags::NoRemoveDir).map_err(|source| {
                removal_path_error("remove direct removal marker", source.into())
            })?;
            runtime
                .sync_all()
                .map_err(|source| removal_path_error("sync runtime directory", source))?;
            after_last_marker();
            validate_removal_ancestry_v1(
                &self.root,
                &runtime,
                self.podway_identity,
                self.current_uid,
                self.podway_identity.device,
                &relative,
            )?;
            unlinkat(&self.podway, "runtime", UnlinkatFlags::RemoveDir).map_err(|source| {
                removal_path_error("remove direct runtime directory", source.into())
            })?;
            self.podway
                .sync_all()
                .map_err(|source| removal_path_error("sync .podway directory", source))?;
            after_runtime_directory();
            self.remove_podway_directory()
        }
        #[cfg(not(unix))]
        {
            let _ = (
                before_last_marker,
                after_first_marker,
                after_last_marker,
                after_runtime_directory,
            );
            Err(WorkspaceRemovalFilesystemErrorV1::UnsupportedPlatform)
        }
    }

    pub fn remove_empty_residual(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(), WorkspaceRemovalFilesystemErrorV1> {
        #[cfg(unix)]
        {
            let runtime = self.open_child_directory(&self.podway, "runtime")?;
            if let Some(runtime) = runtime.as_ref() {
                // Match Store's ExclusiveMaintenanceLeaseV1 admission budget and cadence.
                loop {
                    match runtime.try_lock() {
                        Ok(()) => break,
                        Err(std::fs::TryLockError::WouldBlock) => {
                            let remaining =
                                deadline.saturating_duration_since(std::time::Instant::now());
                            if remaining.is_zero() {
                                return Err(WorkspaceRemovalFilesystemErrorV1::MaintenanceBusy);
                            }
                            std::thread::sleep(
                                remaining.min(std::time::Duration::from_micros(500)),
                            );
                        }
                        Err(std::fs::TryLockError::Error(source)) => {
                            return Err(removal_path_error("lock empty runtime directory", source));
                        }
                    }
                }
                let identity =
                    RemovalObjectIdentityV1::from_metadata(&runtime.metadata().map_err(
                        |source| removal_path_error("inspect empty runtime directory", source),
                    )?);
                validate_removal_ancestry_v1(
                    &self.root,
                    runtime,
                    self.podway_identity,
                    self.current_uid,
                    self.podway_identity.device,
                    &[RemovalPathComponentV1 {
                        name: OsString::from("runtime"),
                        identity,
                    }],
                )?;
            }
            let mut inspection_budget = RemovalTraversalBudgetV1::new();
            if !inspect_empty_directories_v1(
                &self.podway,
                self.current_uid,
                self.podway_identity.device,
                0,
                &mut inspection_budget,
            )? {
                return Err(WorkspaceRemovalFilesystemErrorV1::NotEmptyResidual);
            }
            remove_empty_directories_v1(
                &self.root,
                &self.podway,
                self.podway_identity,
                self.current_uid,
                self.podway_identity.device,
                &[],
                &mut RemovalTraversalBudgetV1::new(),
            )?;
            self.remove_podway_directory()
        }
        #[cfg(not(unix))]
        {
            Err(WorkspaceRemovalFilesystemErrorV1::UnsupportedPlatform)
        }
    }

    #[cfg(unix)]
    fn open_child_directory(
        &self,
        parent: &File,
        name: &str,
    ) -> Result<Option<File>, WorkspaceRemovalFilesystemErrorV1> {
        let descriptor = match openat(
            parent,
            name,
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        ) {
            Ok(descriptor) => descriptor,
            Err(Errno::ENOENT) => return Ok(None),
            Err(Errno::ELOOP | Errno::ENOTDIR) => {
                return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
            }
            Err(source) => {
                return Err(removal_path_error(
                    "open Podway subdirectory",
                    source.into(),
                ));
            }
        };
        let directory = File::from(descriptor);
        validate_removal_directory(&directory, self.current_uid, self.podway_identity.device)?;
        Ok(Some(directory))
    }

    #[cfg(unix)]
    fn remove_podway_directory(&self) -> Result<(), WorkspaceRemovalFilesystemErrorV1> {
        let stat = nix::sys::stat::fstatat(&self.root, ".podway", AtFlags::AT_SYMLINK_NOFOLLOW)
            .map_err(|source| removal_path_error("reinspect .podway directory", source.into()))?;
        if !self.podway_identity.matches_stat(&stat) {
            return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
        }
        unlinkat(&self.root, ".podway", UnlinkatFlags::RemoveDir)
            .map_err(|source| removal_path_error("remove .podway directory", source.into()))?;
        self.root
            .sync_all()
            .map_err(|source| removal_path_error("sync worktree root", source))
    }
}

fn removal_path_error(
    operation: &'static str,
    source: io::Error,
) -> WorkspaceRemovalFilesystemErrorV1 {
    WorkspaceRemovalFilesystemErrorV1::Path { operation, source }
}

#[cfg(unix)]
fn validate_removal_directory(
    directory: &File,
    current_uid: u32,
    expected_device: u64,
) -> Result<(), WorkspaceRemovalFilesystemErrorV1> {
    let metadata = directory
        .metadata()
        .map_err(|source| removal_path_error("inspect Podway directory", source))?;
    if !metadata.is_dir() || metadata.uid() != current_uid || metadata.dev() != expected_device {
        return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
    }
    Ok(())
}

#[cfg(unix)]
fn removal_directory_entries(
    directory: &File,
    budget: &mut RemovalTraversalBudgetV1,
) -> Result<Vec<OsString>, WorkspaceRemovalFilesystemErrorV1> {
    let clone = directory
        .try_clone()
        .map_err(|source| removal_path_error("clone Podway directory descriptor", source))?;
    let owned: OwnedFd = clone.into();
    let mut directory = Dir::from_fd(owned)
        .map_err(|source| removal_path_error("read Podway directory", source.into()))?;
    let mut entries = Vec::new();
    for entry in directory.iter() {
        let entry =
            entry.map_err(|source| removal_path_error("read Podway directory", source.into()))?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        budget.consume()?;
        entries.push(OsString::from_vec(name.to_vec()));
    }
    Ok(entries)
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
fn remove_directory_contents_retaining_v1(
    root: &File,
    directory: &File,
    podway_identity: RemovalObjectIdentityV1,
    current_uid: u32,
    expected_device: u64,
    relative: &[RemovalPathComponentV1],
    budget: &mut RemovalTraversalBudgetV1,
    retained_runtime_files: &[&str],
) -> Result<bool, WorkspaceRemovalFilesystemErrorV1> {
    if relative.len() > MAX_WORKSPACE_REMOVAL_DEPTH_V1 {
        return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
    }
    validate_removal_ancestry_v1(
        root,
        directory,
        podway_identity,
        current_uid,
        expected_device,
        relative,
    )?;
    let mut retained_marker = false;
    for name in removal_directory_entries(directory, budget)? {
        let is_marker = relative.len() == 1
            && relative[0].name.as_bytes() == b"runtime"
            && retained_runtime_files
                .iter()
                .any(|retained| name.as_bytes() == retained.as_bytes());
        if is_marker {
            retained_marker = true;
            continue;
        }
        let stat =
            nix::sys::stat::fstatat(directory, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
                .map_err(|source| removal_path_error("inspect Podway entry", source.into()))?;
        validate_removal_ancestry_v1(
            root,
            directory,
            podway_identity,
            current_uid,
            expected_device,
            relative,
        )?;
        let kind = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
        if kind == SFlag::S_IFDIR {
            let child = File::from(
                openat(
                    directory,
                    name.as_os_str(),
                    OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
                    Mode::empty(),
                )
                .map_err(|source| removal_path_error("open Podway subdirectory", source.into()))?,
            );
            validate_removal_directory(&child, current_uid, expected_device)?;
            let child_metadata = child
                .metadata()
                .map_err(|source| removal_path_error("inspect Podway subdirectory", source))?;
            let child_identity = RemovalObjectIdentityV1::from_metadata(&child_metadata);
            if !child_identity.matches_stat(&stat) {
                return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
            }
            let mut child_relative = relative.to_vec();
            child_relative.push(RemovalPathComponentV1 {
                name: name.clone(),
                identity: child_identity,
            });
            let child_retained = remove_directory_contents_retaining_v1(
                root,
                &child,
                podway_identity,
                current_uid,
                expected_device,
                &child_relative,
                budget,
                retained_runtime_files,
            )?;
            if child_retained {
                retained_marker = true;
            } else {
                validate_removal_ancestry_v1(
                    root,
                    directory,
                    podway_identity,
                    current_uid,
                    expected_device,
                    relative,
                )?;
                let current = nix::sys::stat::fstatat(
                    directory,
                    name.as_os_str(),
                    AtFlags::AT_SYMLINK_NOFOLLOW,
                )
                .map_err(|source| {
                    removal_path_error("reinspect Podway subdirectory", source.into())
                })?;
                if !child_identity.matches_stat(&current) {
                    return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
                }
                unlinkat(directory, name.as_os_str(), UnlinkatFlags::RemoveDir).map_err(
                    |source| removal_path_error("remove Podway subdirectory", source.into()),
                )?;
            }
        } else if kind == SFlag::S_IFREG || kind == SFlag::S_IFLNK {
            validate_removal_ancestry_v1(
                root,
                directory,
                podway_identity,
                current_uid,
                expected_device,
                relative,
            )?;
            unlinkat(directory, name.as_os_str(), UnlinkatFlags::NoRemoveDir)
                .map_err(|source| removal_path_error("remove Podway entry", source.into()))?;
        } else {
            return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
        }
    }
    directory
        .sync_all()
        .map_err(|source| removal_path_error("sync Podway directory", source))?;
    Ok(retained_marker)
}

#[cfg(unix)]
fn remove_empty_directories_v1(
    root: &File,
    directory: &File,
    podway_identity: RemovalObjectIdentityV1,
    current_uid: u32,
    expected_device: u64,
    relative: &[RemovalPathComponentV1],
    budget: &mut RemovalTraversalBudgetV1,
) -> Result<(), WorkspaceRemovalFilesystemErrorV1> {
    if relative.len() > MAX_WORKSPACE_REMOVAL_DEPTH_V1 {
        return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
    }
    validate_removal_ancestry_v1(
        root,
        directory,
        podway_identity,
        current_uid,
        expected_device,
        relative,
    )?;
    for name in removal_directory_entries(directory, budget)? {
        let stat =
            nix::sys::stat::fstatat(directory, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
                .map_err(|source| removal_path_error("inspect removal residual", source.into()))?;
        let kind = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
        if kind != SFlag::S_IFDIR {
            return Err(WorkspaceRemovalFilesystemErrorV1::NotEmptyResidual);
        }
        let child = File::from(
            openat(
                directory,
                name.as_os_str(),
                OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
                Mode::empty(),
            )
            .map_err(|source| removal_path_error("open removal residual", source.into()))?,
        );
        validate_removal_directory(&child, current_uid, expected_device)?;
        let metadata = child
            .metadata()
            .map_err(|source| removal_path_error("inspect removal residual", source))?;
        let identity = RemovalObjectIdentityV1::from_metadata(&metadata);
        if !identity.matches_stat(&stat) {
            return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
        }
        let mut child_relative = relative.to_vec();
        child_relative.push(RemovalPathComponentV1 {
            name: name.clone(),
            identity,
        });
        remove_empty_directories_v1(
            root,
            &child,
            podway_identity,
            current_uid,
            expected_device,
            &child_relative,
            budget,
        )?;
        validate_removal_ancestry_v1(
            root,
            directory,
            podway_identity,
            current_uid,
            expected_device,
            relative,
        )?;
        let current =
            nix::sys::stat::fstatat(directory, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
                .map_err(|source| {
                    removal_path_error("reinspect removal residual", source.into())
                })?;
        if !identity.matches_stat(&current) {
            return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
        }
        unlinkat(directory, name.as_os_str(), UnlinkatFlags::RemoveDir)
            .map_err(|source| removal_path_error("remove empty residual", source.into()))?;
    }
    directory
        .sync_all()
        .map_err(|source| removal_path_error("sync removal residual", source))?;
    Ok(())
}

#[cfg(unix)]
fn validate_removal_ancestry_v1(
    root: &File,
    directory: &File,
    podway_identity: RemovalObjectIdentityV1,
    current_uid: u32,
    expected_device: u64,
    relative: &[RemovalPathComponentV1],
) -> Result<(), WorkspaceRemovalFilesystemErrorV1> {
    let mut current = File::from(
        openat(
            root,
            ".podway",
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map_err(|source| removal_path_error("reopen .podway directory", source.into()))?,
    );
    validate_removal_directory(&current, current_uid, expected_device)?;
    let metadata = current
        .metadata()
        .map_err(|source| removal_path_error("reinspect .podway directory", source))?;
    if RemovalObjectIdentityV1::from_metadata(&metadata) != podway_identity {
        return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
    }
    for component in relative {
        let stat = nix::sys::stat::fstatat(
            &current,
            component.name.as_os_str(),
            AtFlags::AT_SYMLINK_NOFOLLOW,
        )
        .map_err(|source| removal_path_error("reinspect Podway ancestry", source.into()))?;
        if !component.identity.matches_stat(&stat) {
            return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
        }
        current = File::from(
            openat(
                &current,
                component.name.as_os_str(),
                OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
                Mode::empty(),
            )
            .map_err(|source| removal_path_error("reopen Podway ancestry", source.into()))?,
        );
        validate_removal_directory(&current, current_uid, expected_device)?;
        let reopened = current
            .metadata()
            .map_err(|source| removal_path_error("reinspect Podway ancestry", source))?;
        if RemovalObjectIdentityV1::from_metadata(&reopened) != component.identity {
            return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
        }
    }
    let expected = relative
        .last()
        .map(|component| component.identity)
        .unwrap_or(podway_identity);
    let actual = directory
        .metadata()
        .map_err(|source| removal_path_error("reinspect deletion directory", source))?;
    if RemovalObjectIdentityV1::from_metadata(&actual) != expected {
        return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
    }
    Ok(())
}

#[cfg(unix)]
fn inspect_empty_directories_v1(
    directory: &File,
    current_uid: u32,
    expected_device: u64,
    depth: usize,
    budget: &mut RemovalTraversalBudgetV1,
) -> Result<bool, WorkspaceRemovalFilesystemErrorV1> {
    if depth > MAX_WORKSPACE_REMOVAL_DEPTH_V1 {
        return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
    }
    for name in removal_directory_entries(directory, budget)? {
        let stat =
            nix::sys::stat::fstatat(directory, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
                .map_err(|source| removal_path_error("inspect removal residual", source.into()))?;
        let kind = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
        if kind != SFlag::S_IFDIR {
            return Ok(false);
        }
        let child = File::from(
            openat(
                directory,
                name.as_os_str(),
                OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
                Mode::empty(),
            )
            .map_err(|source| removal_path_error("open removal residual", source.into()))?,
        );
        validate_removal_directory(&child, current_uid, expected_device)?;
        let metadata = child
            .metadata()
            .map_err(|source| removal_path_error("inspect removal residual", source))?;
        if !RemovalObjectIdentityV1::from_metadata(&metadata).matches_stat(&stat) {
            return Err(WorkspaceRemovalFilesystemErrorV1::UnsafePath);
        }
        if !inspect_empty_directories_v1(&child, current_uid, expected_device, depth + 1, budget)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(unix)]
struct RemovalTraversalBudgetV1 {
    remaining: usize,
}

#[cfg(unix)]
impl RemovalTraversalBudgetV1 {
    const fn new() -> Self {
        Self {
            remaining: MAX_WORKSPACE_REMOVAL_ENTRIES_V1,
        }
    }

    fn consume(&mut self) -> Result<(), WorkspaceRemovalFilesystemErrorV1> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(WorkspaceRemovalFilesystemErrorV1::UnsafePath)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceGitObservationV1 {
    Preliminary,
    BoundRevalidation,
    CandidateRevalidation,
}

/// Exact failures that prevent a workspace from receiving direct execution authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceResolutionErrorV1 {
    Selector {
        source: SelectorValidationErrorV1,
    },
    Git {
        observation: WorkspaceGitObservationV1,
        source: GitResolveErrorV1,
    },
    ExistingBindingMissing,
    WorkspaceConfigInvalid,
    BootstrapBindingAlreadyPresent,
    ExpectedWorkspaceUuidMismatch {
        expected: WorkspaceId,
        actual: WorkspaceId,
    },
    GitStoreFingerprintMismatch {
        stored: StoreWorktreeIdentityV1,
        observed_common_directory_fingerprint: podway_core::Sha256Digest,
        observed_worktree_administration_fingerprint: podway_core::Sha256Digest,
    },
    PreliminaryIdentityWasNotCandidate {
        state: WorkspaceIdentityStateV1,
    },
    RevalidatedIdentityStateMismatch {
        expected: WorkspaceIdentityStateV1,
        actual: WorkspaceIdentityStateV1,
    },
    RevalidatedStoreIdentityMismatch {
        expected: Box<StoreWorktreeIdentityV1>,
        actual: Box<StoreWorktreeIdentityV1>,
    },
    StoredRootPathInvalid {
        source: SelectorValidationErrorV1,
    },
    WorkspaceRootPathInvalid {
        source: SelectorValidationErrorV1,
    },
    RuntimeDirectoryPathInvalid {
        source: SelectorValidationErrorV1,
    },
    RuntimeDirectoryPathUnsupportedPlatform,
    RuntimeDatabasePathChangedDuringResolution,
    WorkspaceRootConversion {
        source: StoreValueErrorV1,
    },
}

impl fmt::Display for WorkspaceResolutionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Selector { .. } => formatter.write_str("workspace selector is invalid"),
            Self::Git { observation, .. } => {
                write!(formatter, "{observation} Git observation failed")
            }
            Self::WorkspaceConfigInvalid => {
                formatter.write_str("workspace configuration is invalid")
            }
            Self::ExistingBindingMissing => {
                formatter.write_str("workspace database is missing for existing resolution")
            }
            Self::BootstrapBindingAlreadyPresent => {
                formatter.write_str("workspace database already exists for bootstrap resolution")
            }
            Self::ExpectedWorkspaceUuidMismatch { .. } => {
                formatter.write_str("workspace UUID does not match the expected durable UUID")
            }
            Self::GitStoreFingerprintMismatch { .. } => formatter.write_str(
                "workspace database binding does not match the preliminary Git observation",
            ),
            Self::PreliminaryIdentityWasNotCandidate { .. } => formatter
                .write_str("preliminary Git observation did not produce a candidate identity"),
            Self::RevalidatedIdentityStateMismatch { .. } => {
                formatter.write_str("revalidated Git identity has an unexpected binding state")
            }
            Self::RevalidatedStoreIdentityMismatch { .. } => {
                formatter.write_str("revalidated Git identity disagrees with the durable binding")
            }
            Self::StoredRootPathInvalid { .. } => formatter
                .write_str("stored validated root cannot be used as a canonical native path"),
            Self::WorkspaceRootPathInvalid { .. } => {
                formatter.write_str("validated workspace root cannot be decoded as a native path")
            }
            Self::RuntimeDirectoryPathInvalid { .. } => formatter
                .write_str("validated runtime directory cannot be decoded as a native path"),
            Self::RuntimeDirectoryPathUnsupportedPlatform => {
                formatter.write_str("runtime database paths require Unix native path support")
            }
            Self::RuntimeDatabasePathChangedDuringResolution => {
                formatter.write_str("runtime database path changed during Git revalidation")
            }
            Self::WorkspaceRootConversion { .. } => formatter
                .write_str("validated workspace root cannot be converted for Store binding"),
        }
    }
}

impl Error for WorkspaceResolutionErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::WorkspaceRootConversion { source } => Some(source),
            Self::Selector { .. }
            | Self::Git { .. }
            | Self::StoredRootPathInvalid { .. }
            | Self::WorkspaceRootPathInvalid { .. }
            | Self::RuntimeDirectoryPathInvalid { .. }
            | Self::WorkspaceConfigInvalid
            | Self::ExistingBindingMissing
            | Self::BootstrapBindingAlreadyPresent
            | Self::ExpectedWorkspaceUuidMismatch { .. }
            | Self::GitStoreFingerprintMismatch { .. }
            | Self::PreliminaryIdentityWasNotCandidate { .. }
            | Self::RevalidatedIdentityStateMismatch { .. }
            | Self::RevalidatedStoreIdentityMismatch { .. }
            | Self::RuntimeDirectoryPathUnsupportedPlatform
            | Self::RuntimeDatabasePathChangedDuringResolution => None,
        }
    }
}

impl fmt::Display for WorkspaceGitObservationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preliminary => formatter.write_str("preliminary"),
            Self::BoundRevalidation => formatter.write_str("bound revalidation"),
            Self::CandidateRevalidation => formatter.write_str("candidate revalidation"),
        }
    }
}

/// A fully validated workspace prepared for the caller's subsequent Store open or update.
///
/// Display text is never identity. Canonical Git path bytes feed the durable fingerprints, while
/// `database_path` is only the exact local SQLite location opened after resolution succeeds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedWorkspaceV1 {
    worktree: ValidatedWorktreeV1,
    store_identity: StoreWorktreeIdentityV1,
    workspace_root: ValidatedWorkspaceRootV1,
    database_path: PathBuf,
    move_metadata: WorktreeMoveMetadataV1,
}

impl ResolvedWorkspaceV1 {
    fn new(
        worktree: ValidatedWorktreeV1,
        database_path: PathBuf,
    ) -> Result<Self, WorkspaceResolutionErrorV1> {
        let store_identity = store_identity_from_git(worktree.identity());
        let workspace_root = store_root_from_worktree(&worktree)?;
        let move_metadata = worktree.move_metadata().clone();
        Ok(Self {
            worktree,
            store_identity,
            workspace_root,
            database_path,
            move_metadata,
        })
    }

    pub fn worktree(&self) -> &ValidatedWorktreeV1 {
        &self.worktree
    }

    pub fn store_identity(&self) -> &StoreWorktreeIdentityV1 {
        &self.store_identity
    }

    pub fn workspace_root(&self) -> &ValidatedWorkspaceRootV1 {
        &self.workspace_root
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn move_metadata(&self) -> &WorktreeMoveMetadataV1 {
        &self.move_metadata
    }
}

type DirectStoreBoundaryObserverV1 =
    std::sync::Arc<dyn Fn(podway_store::direct_store::DirectTransactionBoundaryV1) + Send + Sync>;

/// Direct resolution never invokes the daemon-era migration or binding inspector.
pub struct DirectWorkspaceResolverV1<R> {
    git_resolver: R,
    manifest: podway_core::Sha256Digest,
    store_boundary_observer: Option<DirectStoreBoundaryObserverV1>,
    artifact_hash_observer: Option<crate::native_execution::NativeArtifactHashObserverV1>,
}

#[derive(Debug)]
pub enum DirectWorkspaceResolutionErrorV1 {
    Resolution(WorkspaceResolutionErrorV1),
    Store(DirectStoreErrorV1),
}

impl fmt::Display for DirectWorkspaceResolutionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolution(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl Error for DirectWorkspaceResolutionErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Resolution(error) => Some(error),
            Self::Store(error) => Some(error),
        }
    }
}

impl From<WorkspaceResolutionErrorV1> for DirectWorkspaceResolutionErrorV1 {
    fn from(error: WorkspaceResolutionErrorV1) -> Self {
        Self::Resolution(error)
    }
}

impl From<DirectStoreErrorV1> for DirectWorkspaceResolutionErrorV1 {
    fn from(error: DirectStoreErrorV1) -> Self {
        Self::Store(error)
    }
}

impl<R: GitResolverContractV1> DirectWorkspaceResolverV1<R> {
    pub fn new(git_resolver: R, manifest: podway_core::Sha256Digest) -> Self {
        Self {
            git_resolver,
            manifest,
            store_boundary_observer: None,
            artifact_hash_observer: None,
        }
    }

    /// Explicit instrumentation of actual Store boundaries; no product flags.
    #[doc(hidden)]
    pub fn with_store_boundary_observer_v1(
        mut self,
        observer: impl Fn(podway_store::direct_store::DirectTransactionBoundaryV1)
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.store_boundary_observer = Some(std::sync::Arc::new(observer));
        self
    }

    fn observed_store(&self, store: DirectStoreV1) -> DirectStoreV1 {
        match &self.store_boundary_observer {
            Some(observer) => {
                let observer = std::sync::Arc::clone(observer);
                store.with_boundary_observer(move |boundary| observer(boundary))
            }
            None => store,
        }
    }

    /// Observes actual artifact streaming for native qualification; no product flags.
    #[doc(hidden)]
    pub fn with_artifact_hash_observer_v1(
        mut self,
        observer: impl Fn(&LosslessPathV1, u64) + Send + Sync + 'static,
    ) -> Self {
        self.artifact_hash_observer = Some(std::sync::Arc::new(observer));
        self
    }

    pub(crate) fn native_workspace(
        &self,
        resolved: ResolvedWorkspaceV1,
    ) -> crate::native_execution::NativeDirectWorkspaceV1 {
        crate::native_execution::NativeDirectWorkspaceV1::new(resolved)
            .with_hash_observer(self.artifact_hash_observer.clone())
    }

    fn resolve_preliminary(
        &self,
        selector: &WorktreeSelectorV1,
    ) -> Result<ValidatedWorktreeV1, WorkspaceResolutionErrorV1> {
        let candidate = preliminary_selector(selector)?;
        let mut retries = 7;
        loop {
            let source = match self.git_resolver.resolve(candidate.clone()) {
                Ok(worktree) => return Ok(worktree),
                Err(source) => source,
            };
            let transient = matches!(
                source,
                GitResolveErrorV1::Invariant {
                    problem: podway_git::GitInvariantViolationV1::MetadataChangedDuringResolution,
                } | GitResolveErrorV1::Representation {
                    problem: podway_git::GitRepresentationProblemV1::UnsupportedRepositoryLayout,
                }
            );
            if !transient || retries == 0 {
                return Err(WorkspaceResolutionErrorV1::Git {
                    observation: WorkspaceGitObservationV1::Preliminary,
                    source,
                });
            }
            // Removal and init can replace containment between stat and open.
            // Restart discovery; only a complete stable snapshot is authority.
            retries -= 1;
            std::thread::sleep(std::time::Duration::from_micros(500));
        }
    }

    /// Resolve Git authority without inspecting SQLite or inventing a durable UUID.
    pub fn resolve_candidate(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<(ValidatedWorktreeV1, DirectStoreV1), DirectWorkspaceResolutionErrorV1> {
        let worktree = self.resolve_preliminary(&selector)?;
        require_candidate_identity(&worktree)?;
        let layout = DirectWorkspaceLayoutV1::new(store_root_from_worktree(&worktree)?)?;
        let store = DirectStoreV1::new(
            layout,
            self.manifest.clone(),
            worktree.identity().common_directory_fingerprint().clone(),
            worktree
                .identity()
                .worktree_administration_fingerprint()
                .clone(),
        );
        Ok((worktree, self.observed_store(store)))
    }

    /// Existing configuration is admitted before opening SQLite. Missing configuration retains
    /// default admission so fresh initialization and Store absence keep their existing behavior.
    pub fn validate_workspace_config(
        &self,
        worktree: &ValidatedWorktreeV1,
    ) -> Result<(), WorkspaceResolutionErrorV1> {
        let mut bytes = worktree
            .roots()
            .worktree_root()
            .decode_path_bytes()
            .map_err(|source| WorkspaceResolutionErrorV1::WorkspaceRootPathInvalid { source })?;
        if bytes != b"/" {
            bytes.push(b'/');
        }
        bytes.extend_from_slice(b".podway/config.yaml");
        let path = LosslessPathV1::from_raw_bytes(
            bytes.clone(),
            DiagnosticPathDisplayV1::new("worktree workspace configuration")
                .expect("fixed configuration diagnostic is valid"),
        )
        .map_err(|source| WorkspaceResolutionErrorV1::WorkspaceRootPathInvalid { source })?;
        #[cfg(unix)]
        if fs::symlink_metadata(PathBuf::from(OsString::from_vec(bytes)))
            .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
        {
            return Ok(());
        }
        let source = NativeGitResolverV1::new()
            .read_bounded_local_file(
                worktree,
                &path,
                podway_config::MAX_WORKSPACE_CONFIG_BYTES_V1,
            )
            .map_err(|_| WorkspaceResolutionErrorV1::WorkspaceConfigInvalid)?;
        if source.canonical_path() != &path {
            return Err(WorkspaceResolutionErrorV1::WorkspaceConfigInvalid);
        }
        podway_config::parse_workspace_config_v1(source.bytes())
            .map_err(|_| WorkspaceResolutionErrorV1::WorkspaceConfigInvalid)?;
        Ok(())
    }

    pub fn manifest(&self) -> &podway_core::Sha256Digest {
        &self.manifest
    }

    /// Revalidate a Git-only maintenance candidate after native lock acquisition.
    pub fn revalidate_candidate(
        &self,
        worktree: &ValidatedWorktreeV1,
    ) -> Result<(), WorkspaceResolutionErrorV1> {
        let selector = WorktreeSelectorV1::new(
            podway_git::WORKTREE_SELECTOR_VERSION_V1,
            None,
            worktree.roots().worktree_root().clone(),
        )
        .map_err(|source| WorkspaceResolutionErrorV1::Selector { source })?;
        let actual = self.git_resolver.resolve(selector).map_err(|source| {
            WorkspaceResolutionErrorV1::Git {
                observation: WorkspaceGitObservationV1::CandidateRevalidation,
                source,
            }
        })?;
        require_candidate_identity(&actual)?;
        if worktree.identity().common_directory_fingerprint()
            != actual.identity().common_directory_fingerprint()
            || worktree.identity().worktree_administration_fingerprint()
                != actual.identity().worktree_administration_fingerprint()
        {
            return Err(WorkspaceResolutionErrorV1::RuntimeDatabasePathChangedDuringResolution);
        }
        require_same_transient_objects(
            worktree,
            &actual,
            WorkspaceGitObservationV1::CandidateRevalidation,
        )?;
        require_stable_database_path(&database_path_from_worktree(worktree)?, &actual)?;
        Ok(())
    }

    pub fn resolve_existing(
        &self,
        selector: WorktreeSelectorV1,
        expected_workspace_id: Option<&WorkspaceId>,
        deadline: std::time::Instant,
    ) -> Result<(ResolvedWorkspaceV1, DirectStoreV1), DirectWorkspaceResolutionErrorV1> {
        let (resolved, store, _) =
            self.resolve_existing_with_binding(selector, expected_workspace_id, deadline)?;
        Ok((resolved, store))
    }

    pub fn resolve_existing_with_binding(
        &self,
        selector: WorktreeSelectorV1,
        expected_workspace_id: Option<&WorkspaceId>,
        deadline: std::time::Instant,
    ) -> Result<
        (ResolvedWorkspaceV1, DirectStoreV1, WorkspaceBindingV1),
        DirectWorkspaceResolutionErrorV1,
    > {
        let preliminary = self.resolve_preliminary(&selector).map_err(|mut error| {
            // Preliminary discovery must not trust a supplied UUID. On failure,
            // retain the native deletion classification for a previously bound
            // selector without treating that selector as Store authority.
            if selector.durable_identity().is_some()
                && matches!(
                    self.git_resolver.resolve(selector.clone()),
                    Err(GitResolveErrorV1::WorktreeDeleted)
                )
            {
                error = WorkspaceResolutionErrorV1::Git {
                    observation: WorkspaceGitObservationV1::Preliminary,
                    source: GitResolveErrorV1::WorktreeDeleted,
                };
            }
            error
        })?;
        require_candidate_identity(&preliminary)?;
        let layout = DirectWorkspaceLayoutV1::new(store_root_from_worktree(&preliminary)?)?;
        if let Err(error) = self.validate_workspace_config(&preliminary) {
            // Removal can invalidate a config read while its marker still owns admission.
            // Classify without opening SQLite; unmarked invalid config remains rejected.
            match classify_direct_store_v1(&layout) {
                Ok(DirectStoreClassificationV1::Newer { generation }) => {
                    return Err(DirectStoreErrorV1::UnsupportedGeneration { generation }.into());
                }
                Ok(DirectStoreClassificationV1::Maintenance) => {
                    return Err(DirectStoreErrorV1::Maintenance.into());
                }
                _ => {}
            }
            return Err(error.into());
        }
        let database_path = database_path_from_worktree(&preliminary)?;
        let store = DirectStoreV1::new(
            layout,
            self.manifest.clone(),
            preliminary
                .identity()
                .common_directory_fingerprint()
                .clone(),
            preliminary
                .identity()
                .worktree_administration_fingerprint()
                .clone(),
        );
        let store = self.observed_store(store);
        let binding = store.read(deadline, |snapshot| Ok(snapshot.binding().clone()))?;
        for expected in expected_workspace_id.into_iter().chain(
            selector
                .durable_identity()
                .map(|identity| identity.workspace_id()),
        ) {
            if expected != binding.identity().workspace_uuid() {
                return Err(WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch {
                    expected: expected.clone(),
                    actual: binding.identity().workspace_uuid().clone(),
                }
                .into());
            }
        }
        let revalidated = self.revalidate_worktree(&preliminary, &binding, &selector)?;
        let path = require_stable_database_path(&database_path, &revalidated)?;
        Ok((ResolvedWorkspaceV1::new(revalidated, path)?, store, binding))
    }

    /// The caller supplies its transaction's validated binding. No nested Store open occurs.
    pub fn revalidate(
        &self,
        resolved: &ResolvedWorkspaceV1,
        binding: &WorkspaceBindingV1,
    ) -> Result<(), WorkspaceResolutionErrorV1> {
        if resolved.store_identity() != binding.identity() {
            return Err(
                WorkspaceResolutionErrorV1::RevalidatedStoreIdentityMismatch {
                    expected: Box::new(resolved.store_identity().clone()),
                    actual: Box::new(binding.identity().clone()),
                },
            );
        }
        let worktree = resolved.worktree();
        let selector = WorktreeSelectorV1::new(
            podway_git::WORKTREE_SELECTOR_VERSION_V1,
            Some(worktree.identity().clone()),
            worktree.roots().worktree_root().clone(),
        )
        .map_err(|source| WorkspaceResolutionErrorV1::Selector { source })?;
        let revalidated = self.revalidate_worktree(worktree, binding, &selector)?;
        require_stable_database_path(resolved.database_path(), &revalidated)?;
        self.validate_workspace_config(&revalidated)?;
        Ok(())
    }

    fn revalidate_worktree(
        &self,
        preliminary: &ValidatedWorktreeV1,
        binding: &WorkspaceBindingV1,
        selector: &WorktreeSelectorV1,
    ) -> Result<ValidatedWorktreeV1, WorkspaceResolutionErrorV1> {
        require_matching_git_fingerprints(preliminary, binding)?;
        let selector = WorktreeSelectorV1::new(
            selector.version(),
            Some(bound_git_identity(preliminary, binding)?),
            selector.path().clone(),
        )
        .map_err(|source| WorkspaceResolutionErrorV1::Selector { source })?;
        let revalidated = self.git_resolver.resolve(selector).map_err(|source| {
            WorkspaceResolutionErrorV1::Git {
                observation: WorkspaceGitObservationV1::BoundRevalidation,
                source,
            }
        })?;
        require_revalidated_identity(
            &revalidated,
            WorkspaceIdentityStateV1::Bound,
            binding.identity(),
        )?;
        require_same_transient_objects(
            preliminary,
            &revalidated,
            WorkspaceGitObservationV1::BoundRevalidation,
        )?;
        Ok(revalidated)
    }
}

fn preliminary_selector(
    selector: &WorktreeSelectorV1,
) -> Result<WorktreeSelectorV1, WorkspaceResolutionErrorV1> {
    WorktreeSelectorV1::new(selector.version(), None, selector.path().clone())
        .map_err(|source| WorkspaceResolutionErrorV1::Selector { source })
}

fn require_candidate_identity(
    worktree: &ValidatedWorktreeV1,
) -> Result<(), WorkspaceResolutionErrorV1> {
    if worktree.identity().state() != &WorkspaceIdentityStateV1::Candidate {
        return Err(
            WorkspaceResolutionErrorV1::PreliminaryIdentityWasNotCandidate {
                state: worktree.identity().state().clone(),
            },
        );
    }
    Ok(())
}

fn require_matching_git_fingerprints(
    preliminary: &ValidatedWorktreeV1,
    binding: &WorkspaceBindingV1,
) -> Result<(), WorkspaceResolutionErrorV1> {
    let stored = binding.identity();
    if preliminary.identity().common_directory_fingerprint() != stored.common_dir_identity()
        || preliminary.identity().worktree_administration_fingerprint()
            != stored.worktree_admin_identity()
    {
        return Err(WorkspaceResolutionErrorV1::GitStoreFingerprintMismatch {
            stored: stored.clone(),
            observed_common_directory_fingerprint: preliminary
                .identity()
                .common_directory_fingerprint()
                .clone(),
            observed_worktree_administration_fingerprint: preliminary
                .identity()
                .worktree_administration_fingerprint()
                .clone(),
        });
    }
    Ok(())
}

fn bound_git_identity(
    preliminary: &ValidatedWorktreeV1,
    binding: &WorkspaceBindingV1,
) -> Result<GitWorktreeIdentityV1, WorkspaceResolutionErrorV1> {
    let prior_root = LosslessPathV1::from_raw_bytes(
        binding.last_validated_root().unix_bytes(),
        DiagnosticPathDisplayV1::new(STORED_ROOT_DIAGNOSTIC_V1)
            .expect("fixed stored-root diagnostic is valid"),
    )
    .map_err(|source| WorkspaceResolutionErrorV1::StoredRootPathInvalid { source })?;
    let root_directory_fingerprint = preliminary
        .identity()
        .root_directory_fingerprint()
        .expect("validated worktrees always include a root directory fingerprint")
        .clone();

    Ok(GitWorktreeIdentityV1::new_with_root_directory_fingerprint(
        binding.identity().workspace_uuid().clone(),
        preliminary
            .identity()
            .common_directory_fingerprint()
            .clone(),
        preliminary
            .identity()
            .worktree_administration_fingerprint()
            .clone(),
        root_directory_fingerprint,
        prior_root,
    ))
}

fn require_revalidated_identity(
    revalidated: &ValidatedWorktreeV1,
    expected_state: WorkspaceIdentityStateV1,
    expected_store_identity: &StoreWorktreeIdentityV1,
) -> Result<(), WorkspaceResolutionErrorV1> {
    let actual_state = revalidated.identity().state();
    if actual_state != &expected_state {
        return Err(
            WorkspaceResolutionErrorV1::RevalidatedIdentityStateMismatch {
                expected: expected_state,
                actual: actual_state.clone(),
            },
        );
    }

    let actual_store_identity = store_identity_from_git(revalidated.identity());
    if &actual_store_identity != expected_store_identity {
        return Err(
            WorkspaceResolutionErrorV1::RevalidatedStoreIdentityMismatch {
                expected: Box::new(expected_store_identity.clone()),
                actual: Box::new(actual_store_identity),
            },
        );
    }
    Ok(())
}

fn require_same_transient_objects(
    preliminary: &ValidatedWorktreeV1,
    revalidated: &ValidatedWorktreeV1,
    observation: WorkspaceGitObservationV1,
) -> Result<(), WorkspaceResolutionErrorV1> {
    if preliminary.same_transient_object_evidence(revalidated) {
        return Ok(());
    }
    Err(WorkspaceResolutionErrorV1::Git {
        observation,
        source: GitResolveErrorV1::Invariant {
            problem: GitInvariantViolationV1::MetadataChangedDuringResolution,
        },
    })
}

fn store_identity_from_git(identity: &GitWorktreeIdentityV1) -> StoreWorktreeIdentityV1 {
    StoreWorktreeIdentityV1::new(
        identity.common_directory_fingerprint().clone(),
        identity.workspace_id().clone(),
        identity.worktree_administration_fingerprint().clone(),
    )
}

fn store_root_from_worktree(
    worktree: &ValidatedWorktreeV1,
) -> Result<ValidatedWorkspaceRootV1, WorkspaceResolutionErrorV1> {
    let bytes = worktree
        .roots()
        .worktree_root()
        .decode_path_bytes()
        .map_err(|source| WorkspaceResolutionErrorV1::WorkspaceRootPathInvalid { source })?;
    validated_store_root_from_unix_bytes(bytes)
        .map_err(|source| WorkspaceResolutionErrorV1::WorkspaceRootConversion { source })
}

fn database_path_from_worktree(
    worktree: &ValidatedWorktreeV1,
) -> Result<PathBuf, WorkspaceResolutionErrorV1> {
    let runtime_bytes = worktree
        .containment()
        .runtime_directory()
        .decode_path_bytes()
        .map_err(|source| WorkspaceResolutionErrorV1::RuntimeDirectoryPathInvalid { source })?;
    database_path_from_unix_bytes(runtime_bytes)
}
fn require_stable_database_path(
    preliminary_database_path: &Path,
    revalidated: &ValidatedWorktreeV1,
) -> Result<PathBuf, WorkspaceResolutionErrorV1> {
    let revalidated_database_path = database_path_from_worktree(revalidated)?;
    if preliminary_database_path != revalidated_database_path {
        return Err(WorkspaceResolutionErrorV1::RuntimeDatabasePathChangedDuringResolution);
    }
    Ok(revalidated_database_path)
}

#[cfg(unix)]
fn validated_store_root_from_unix_bytes(
    bytes: Vec<u8>,
) -> Result<ValidatedWorkspaceRootV1, StoreValueErrorV1> {
    ValidatedWorkspaceRootV1::from_unix_bytes(bytes)
}

#[cfg(not(unix))]
fn validated_store_root_from_unix_bytes(
    _bytes: Vec<u8>,
) -> Result<ValidatedWorkspaceRootV1, StoreValueErrorV1> {
    Err(StoreValueErrorV1::UnsupportedWorkspaceRootPlatform)
}

#[cfg(unix)]
fn database_path_from_unix_bytes(bytes: Vec<u8>) -> Result<PathBuf, WorkspaceResolutionErrorV1> {
    let mut database_path = PathBuf::from(OsString::from_vec(bytes));
    database_path.push(STATE_DATABASE_FILE_NAME_V1);
    Ok(database_path)
}

#[cfg(not(unix))]
fn database_path_from_unix_bytes(_bytes: Vec<u8>) -> Result<PathBuf, WorkspaceResolutionErrorV1> {
    Err(WorkspaceResolutionErrorV1::RuntimeDirectoryPathUnsupportedPlatform)
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        RemovalObjectIdentityV1, RemovalPathComponentV1, RemovalTraversalBudgetV1,
        inspect_empty_directories_v1, remove_directory_contents_retaining_v1,
    };

    static RUNTIME_TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn runtime_directory() -> (std::path::PathBuf, fs::File) {
        use std::os::unix::fs::PermissionsExt;
        let sequence = RUNTIME_TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "podway-removal-unit-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let directory = fs::File::open(&path).unwrap();
        (path, directory)
    }

    #[test]
    fn nonempty_removal_residual_is_detected_without_partial_deletion() {
        use std::os::unix::fs::MetadataExt;

        let (path, runtime) = runtime_directory();
        let first = path.join("first-empty");
        let second = path.join("second-nonempty");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(second.join("retained"), "ambiguous").unwrap();
        let metadata = runtime.metadata().unwrap();
        let empty = inspect_empty_directories_v1(
            &runtime,
            nix::unistd::getuid().as_raw(),
            metadata.dev(),
            0,
            &mut RemovalTraversalBudgetV1::new(),
        )
        .unwrap();
        assert!(!empty);
        assert!(
            first.is_dir(),
            "inspection must not delete earlier empty siblings"
        );
        assert!(second.join("retained").is_file());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn renamed_removal_subdirectory_is_rejected_before_deleting_outside_podway() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let sequence = RUNTIME_TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root_path = std::env::temp_dir().join(format!(
            "podway-removal-ancestry-unit-{}-{sequence}",
            std::process::id()
        ));
        let podway_path = root_path.join(".podway");
        let child_path = podway_path.join("child");
        fs::create_dir_all(&child_path).unwrap();
        fs::set_permissions(&podway_path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&child_path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(child_path.join("retained"), b"outside boundary").unwrap();

        let root = fs::File::open(&root_path).unwrap();
        let podway = fs::File::open(&podway_path).unwrap();
        let child = fs::File::open(&child_path).unwrap();
        let podway_identity = RemovalObjectIdentityV1::from_metadata(&podway.metadata().unwrap());
        let child_identity = RemovalObjectIdentityV1::from_metadata(&child.metadata().unwrap());
        let moved_path = root_path.join("moved-outside-podway");
        fs::rename(&child_path, &moved_path).unwrap();

        let result = remove_directory_contents_retaining_v1(
            &root,
            &child,
            podway_identity,
            nix::unistd::getuid().as_raw(),
            podway.metadata().unwrap().dev(),
            &[RemovalPathComponentV1 {
                name: "child".into(),
                identity: child_identity,
            }],
            &mut RemovalTraversalBudgetV1::new(),
            &[],
        );
        assert!(result.is_err());
        assert!(moved_path.join("retained").is_file());
        fs::remove_dir_all(root_path).unwrap();
    }
}
