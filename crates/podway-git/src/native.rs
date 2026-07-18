//! Descriptor-anchored native Unix filesystem parsing for the bounded Git resolver.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, Metadata};
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(target_os = "macos")]
use std::os::macos::fs::MetadataExt as MacMetadataExt;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::{Arc, Barrier, Mutex};

use rustix::fs::{self as rustix_fs, AtFlags, FileType, FlockOperation, Mode, OFlags, RenameFlags};
use sha2::{Digest, Sha256};

use crate::{
    DiagnosticPathDisplayV1, GitInvariantViolationV1, GitReadOperationV1,
    GitRepresentationProblemV1, GitResolverErrorV1, LosslessPathV1,
    MAX_SELECTOR_COMPONENT_BYTES_V1, WorktreeKindV1,
};
use podway_core::Sha256Digest;

const MAX_GIT_METADATA_BYTES: usize = 16 * 1024;
const MAX_LAYOUT_IGNORE_BYTES: usize = 1024 * 1024;
const MAX_LAYOUT_TEMP_ATTEMPTS: usize = 32;
const MAX_LAYOUT_CONFIG_BYTES: usize = 64 * 1024;
static LAYOUT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
// Retained candidate descriptors are capped well below the selector byte bound.
const MAX_DISCOVERY_CANDIDATES: usize = MAX_SELECTOR_COMPONENT_BYTES_V1 / 64;
#[cfg(test)]
static DISCOVERY_MARKER_CREATION_TARGET: Mutex<Option<PathBuf>> = Mutex::new(None);
#[cfg(all(test, target_os = "linux"))]
static REGULAR_FILE_FIFO_REPLACEMENT_TARGET: Mutex<Option<PathBuf>> = Mutex::new(None);
#[cfg(test)]
static ARTIFACT_HASH_REPLACEMENT_BARRIER: Mutex<Option<Arc<Barrier>>> = Mutex::new(None);
#[cfg(test)]
static WORKSPACE_LAYOUT_ROOT_REPLACEMENT_BARRIER: Mutex<Option<(PathBuf, Arc<Barrier>)>> =
    Mutex::new(None);
#[cfg(test)]
static WORKSPACE_LAYOUT_CONFIG_REPLACEMENT_BARRIER: Mutex<Option<(PathBuf, Arc<Barrier>)>> =
    Mutex::new(None);

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
enum WorkspaceIgnoreMutation {
    Rewrite(Vec<u8>),
    Chmod(u32),
    Replace(Vec<u8>),
}
#[cfg(test)]
static WORKSPACE_IGNORE_MUTATION: Mutex<Option<(PathBuf, WorkspaceIgnoreMutation)>> =
    Mutex::new(None);
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceIgnoreCleanupFailure {
    QuarantineSync,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceIgnoreRenameFailure {
    Exchange,
}
#[cfg(test)]
static WORKSPACE_IGNORE_CLEANUP_FAILURE: Mutex<Option<WorkspaceIgnoreCleanupFailure>> =
    Mutex::new(None);
#[cfg(test)]
static WORKSPACE_IGNORE_RENAME_FAILURE: Mutex<Option<WorkspaceIgnoreRenameFailure>> =
    Mutex::new(None);
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceIgnorePreExchangeFailure {
    CurrentStat,
    TemporarySnapshot,
}
#[cfg(test)]
static WORKSPACE_IGNORE_PRE_EXCHANGE_FAILURE: Mutex<Option<WorkspaceIgnorePreExchangeFailure>> =
    Mutex::new(None);
#[cfg(test)]
static WORKSPACE_IGNORE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
fn create_discovery_marker_for_test() {
    let target = DISCOVERY_MARKER_CREATION_TARGET
        .lock()
        .expect("discovery test hook lock")
        .take();
    if let Some(target) = target {
        fs::create_dir(&target).expect("create nearer Git marker");
    }
}
#[cfg(test)]
fn mutate_workspace_ignore_after_precheck_for_test(path: &Path) {
    let mutation = {
        let mut configured = WORKSPACE_IGNORE_MUTATION
            .lock()
            .expect("workspace ignore test hook lock");
        if configured
            .as_ref()
            .is_some_and(|(target, _)| target == path)
        {
            configured.take()
        } else {
            None
        }
    };
    if let Some((_, mutation)) = mutation {
        match mutation {
            WorkspaceIgnoreMutation::Rewrite(contents) => {
                fs::write(path, contents).expect("rewrite workspace ignore after precheck");
            }
            WorkspaceIgnoreMutation::Chmod(mode) => {
                fs::set_permissions(path, fs::Permissions::from_mode(mode))
                    .expect("chmod workspace ignore after precheck");
            }
            WorkspaceIgnoreMutation::Replace(contents) => {
                fs::remove_file(path).expect("remove staged workspace ignore");
                fs::write(path, contents).expect("replace staged workspace ignore");
            }
        }
    }
}
#[cfg(test)]
fn take_workspace_ignore_cleanup_failure(failure: WorkspaceIgnoreCleanupFailure) -> bool {
    let mut configured = WORKSPACE_IGNORE_CLEANUP_FAILURE
        .lock()
        .expect("workspace ignore cleanup test hook lock");
    if *configured == Some(failure) {
        *configured = None;
        true
    } else {
        false
    }
}
#[cfg(test)]
fn take_workspace_ignore_rename_failure(failure: WorkspaceIgnoreRenameFailure) -> bool {
    let mut configured = WORKSPACE_IGNORE_RENAME_FAILURE
        .lock()
        .expect("workspace ignore test hook lock");
    if *configured == Some(failure) {
        *configured = None;
        true
    } else {
        false
    }
}
#[cfg(test)]
fn take_workspace_ignore_pre_exchange_failure(failure: WorkspaceIgnorePreExchangeFailure) -> bool {
    let mut configured = WORKSPACE_IGNORE_PRE_EXCHANGE_FAILURE
        .lock()
        .expect("workspace ignore test hook lock");
    if *configured == Some(failure) {
        *configured = None;
        true
    } else {
        false
    }
}

#[cfg(test)]
fn injected_workspace_ignore_cleanup_failure() -> GitResolverErrorV1 {
    GitResolverErrorV1::Io {
        operation: GitReadOperationV1::InitializeWorkspaceLayout,
    }
}

#[cfg(all(test, target_os = "linux"))]
fn replace_regular_file_with_fifo_for_test(path: &Path) {
    let target = REGULAR_FILE_FIFO_REPLACEMENT_TARGET
        .lock()
        .expect("regular-file test hook lock")
        .take();
    if target.as_deref() == Some(path) {
        fs::remove_file(path).expect("remove staged regular file");
        rustix_fs::mkfifoat(rustix_fs::CWD, path, Mode::RUSR | Mode::WUSR)
            .expect("replace regular file with FIFO");
    }
}

#[cfg(test)]
pub(crate) fn install_artifact_hash_replacement_hook_for_test(barrier: Arc<Barrier>) {
    *ARTIFACT_HASH_REPLACEMENT_BARRIER
        .lock()
        .expect("artifact hash test hook lock") = Some(barrier);
}

#[cfg(test)]
pub(crate) fn synchronize_artifact_hash_replacement_for_test() {
    let barrier = ARTIFACT_HASH_REPLACEMENT_BARRIER
        .lock()
        .expect("artifact hash test hook lock")
        .take();
    if let Some(barrier) = barrier {
        barrier.wait();
        barrier.wait();
    }
}
#[cfg(test)]
pub(crate) fn install_workspace_layout_root_replacement_hook_for_test(
    root: PathBuf,
    barrier: Arc<Barrier>,
) {
    *WORKSPACE_LAYOUT_ROOT_REPLACEMENT_BARRIER
        .lock()
        .expect("workspace layout root replacement test hook lock") = Some((root, barrier));
}

#[cfg(test)]
fn synchronize_workspace_layout_root_replacement_for_test(root: &Path) {
    let barrier = {
        let mut hook = WORKSPACE_LAYOUT_ROOT_REPLACEMENT_BARRIER
            .lock()
            .expect("workspace layout root replacement test hook lock");
        if hook.as_ref().is_some_and(|(target, _)| target == root) {
            hook.take().map(|(_, barrier)| barrier)
        } else {
            None
        }
    };
    if let Some(barrier) = barrier {
        barrier.wait();
        barrier.wait();
    }
}
#[cfg(test)]
pub(crate) fn install_workspace_layout_config_replacement_hook_for_test(
    config: PathBuf,
    barrier: Arc<Barrier>,
) {
    *WORKSPACE_LAYOUT_CONFIG_REPLACEMENT_BARRIER
        .lock()
        .expect("workspace layout config replacement test hook lock") = Some((config, barrier));
}

#[cfg(test)]
fn synchronize_workspace_layout_config_replacement_for_test(config: &Path) {
    let barrier = {
        let mut hook = WORKSPACE_LAYOUT_CONFIG_REPLACEMENT_BARRIER
            .lock()
            .expect("workspace layout config replacement test hook lock");
        if hook.as_ref().is_some_and(|(target, _)| target == config) {
            hook.take().map(|(_, barrier)| barrier)
        } else {
            None
        }
    };
    if let Some(barrier) = barrier {
        barrier.wait();
        barrier.wait();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StableFileType {
    Directory,
    Regular,
}

impl StableFileType {
    fn tag(self) -> &'static [u8] {
        match self {
            Self::Directory => b"directory",
            Self::Regular => b"regular",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CreationEvidence {
    #[cfg(target_os = "macos")]
    Darwin {
        birth_seconds: i64,
        birth_nanoseconds: i64,
    },
    #[cfg(target_os = "linux")]
    Linux {
        device_major: u32,
        device_minor: u32,
        birth_seconds: i64,
        birth_nanoseconds: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ObjectIdentity {
    device: u64,
    inode: u64,
    file_type: StableFileType,
    creation: CreationEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileSnapshot {
    identity: ObjectIdentity,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl FileSnapshot {
    fn from_open_file(
        file: &File,
        path: &Path,
        operation: GitReadOperationV1,
    ) -> Result<Self, GitResolverErrorV1> {
        let metadata = file
            .metadata()
            .map_err(|error| map_io(path, operation.clone(), error))?;
        Ok(Self {
            identity: object_identity(file, &metadata, path, operation)?,
            size: metadata.size(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        })
    }

    fn same_identity(self, other: Self) -> bool {
        self.identity == other.identity
    }

    pub(crate) fn size(self) -> u64 {
        self.size
    }
}

pub(crate) struct OpenedDirectory {
    file: File,
    path: PathBuf,
    snapshot: FileSnapshot,
}

impl OpenedDirectory {
    fn from_file(
        file: File,
        path: PathBuf,
        operation: GitReadOperationV1,
    ) -> Result<Self, GitResolverErrorV1> {
        let metadata = file
            .metadata()
            .map_err(|error| map_io(&path, operation.clone(), error))?;
        if !metadata.is_dir() {
            return Err(GitResolverErrorV1::Representation {
                problem: GitRepresentationProblemV1::UnsupportedRepositoryLayout,
            });
        }
        let snapshot = FileSnapshot::from_open_file(&file, &path, operation)?;
        if snapshot.identity.file_type != StableFileType::Directory {
            return Err(GitResolverErrorV1::Representation {
                problem: GitRepresentationProblemV1::UnsupportedRepositoryLayout,
            });
        }
        Ok(Self {
            file,
            path,
            snapshot,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn verify(&self, invariant: GitInvariantViolationV1) -> Result<(), GitResolverErrorV1> {
        let current = FileSnapshot::from_open_file(
            &self.file,
            &self.path,
            GitReadOperationV1::ReadGitDirectory,
        )?;
        if !current.same_identity(self.snapshot) {
            return Err(metadata_changed(invariant));
        }
        let reopened = match open_absolute_directory(
            &self.path,
            GitReadOperationV1::ReadGitDirectory,
            GitRepresentationProblemV1::UnsupportedRepositoryLayout,
        ) {
            Ok(directory) => directory,
            Err(error) => return Err(map_revalidation_error(error, invariant.clone())),
        };
        if !reopened.snapshot.same_identity(self.snapshot) {
            return Err(metadata_changed(invariant));
        }
        Ok(())
    }
}

struct OpenedRegularFile {
    file: File,
    path: PathBuf,
    snapshot: FileSnapshot,
}

impl OpenedRegularFile {
    fn from_file(
        file: File,
        path: PathBuf,
        operation: GitReadOperationV1,
    ) -> Result<Self, GitResolverErrorV1> {
        let metadata = file
            .metadata()
            .map_err(|error| map_io(&path, operation.clone(), error))?;
        if !metadata.is_file() {
            return Err(GitResolverErrorV1::Representation {
                problem: GitRepresentationProblemV1::NonRegularArtifact,
            });
        }
        let snapshot = FileSnapshot::from_open_file(&file, &path, operation)?;
        if snapshot.identity.file_type != StableFileType::Regular {
            return Err(GitResolverErrorV1::Representation {
                problem: GitRepresentationProblemV1::NonRegularArtifact,
            });
        }
        Ok(Self {
            file,
            path,
            snapshot,
        })
    }

    fn verify(&self, invariant: GitInvariantViolationV1) -> Result<(), GitResolverErrorV1> {
        let current =
            FileSnapshot::from_open_file(&self.file, &self.path, GitReadOperationV1::ReadGitFile)?;
        if current != self.snapshot {
            return Err(metadata_changed(invariant));
        }
        let reopened = match open_absolute_regular_file(
            &self.path,
            GitReadOperationV1::ReadGitFile,
            GitRepresentationProblemV1::MalformedGitFile,
        ) {
            Ok(file) => file,
            Err(error) => return Err(map_revalidation_error(error, invariant.clone())),
        };
        if reopened.snapshot != self.snapshot {
            return Err(metadata_changed(invariant));
        }
        Ok(())
    }
}

enum GitMarker {
    Directory(OpenedDirectory),
    File(OpenedRegularFile),
}

impl GitMarker {
    fn verify(&self, invariant: GitInvariantViolationV1) -> Result<(), GitResolverErrorV1> {
        match self {
            Self::Directory(directory) => directory.verify(invariant),
            Self::File(file) => file.verify(invariant),
        }
    }
}

struct DiscoveryCandidate {
    directory: OpenedDirectory,
}

impl DiscoveryCandidate {
    fn validate(&self, invariant: GitInvariantViolationV1) -> Result<(), GitResolverErrorV1> {
        self.directory.verify(invariant.clone())?;
        let marker = entry_stat_if_exists(
            &self.directory,
            OsStr::new(".git"),
            GitReadOperationV1::DiscoverRepository,
        )?;
        if marker.is_some() || is_bare_repository_signature(&self.directory)? {
            return Err(metadata_changed(invariant));
        }
        Ok(())
    }
}

pub(crate) struct DiscoveredLayout {
    pub(crate) worktree_root: OpenedDirectory,
    git_marker: GitMarker,
    pub(crate) common_directory_root: OpenedDirectory,
    pub(crate) worktree_administration_root: OpenedDirectory,
    discovery_candidates: Vec<DiscoveryCandidate>,
    supporting_directories: Vec<OpenedDirectory>,
    metadata_records: Vec<OpenedRegularFile>,
    pub(crate) kind: WorktreeKindV1,
}

impl DiscoveredLayout {
    pub(crate) fn validate_resolution(&self) -> Result<(), GitResolverErrorV1> {
        self.validate(GitInvariantViolationV1::MetadataChangedDuringResolution)
    }

    pub(crate) fn validate_artifact_snapshot(&self) -> Result<(), GitResolverErrorV1> {
        self.validate(GitInvariantViolationV1::MetadataChangedDuringArtifactHash)
    }

    fn validate(&self, invariant: GitInvariantViolationV1) -> Result<(), GitResolverErrorV1> {
        self.worktree_root.verify(invariant.clone())?;
        self.git_marker.verify(invariant.clone())?;
        self.common_directory_root.verify(invariant.clone())?;
        self.worktree_administration_root
            .verify(invariant.clone())?;
        for candidate in &self.discovery_candidates {
            candidate.validate(invariant.clone())?;
        }
        for directory in &self.supporting_directories {
            directory.verify(invariant.clone())?;
        }
        for record in &self.metadata_records {
            record.verify(invariant.clone())?;
        }
        Ok(())
    }
}

pub(crate) struct ContainmentSnapshot {
    podway: Option<OpenedDirectory>,
    runtime: Option<OpenedDirectory>,
}

impl ContainmentSnapshot {
    pub(crate) fn podway_path(&self, root: &Path) -> PathBuf {
        self.podway
            .as_ref()
            .map(|directory| directory.path.clone())
            .unwrap_or_else(|| root.join(".podway"))
    }

    pub(crate) fn runtime_path(&self, root: &Path) -> PathBuf {
        self.runtime
            .as_ref()
            .map(|directory| directory.path.clone())
            .unwrap_or_else(|| self.podway_path(root).join("runtime"))
    }

    pub(crate) fn validate(
        &self,
        root: &OpenedDirectory,
        invariant: GitInvariantViolationV1,
    ) -> Result<(), GitResolverErrorV1> {
        match &self.podway {
            Some(directory) => directory.verify(invariant.clone())?,
            None => ensure_child_missing(root, OsStr::new(".podway"), invariant.clone())?,
        }
        match (&self.podway, &self.runtime) {
            (Some(_), Some(directory)) => directory.verify(invariant)?,
            (Some(podway), None) => {
                ensure_child_missing(podway, OsStr::new("runtime"), invariant)?;
            }
            (None, None) => {}
            (None, Some(_)) => return Err(metadata_changed(invariant)),
        }
        Ok(())
    }
}

pub(crate) struct OpenedArtifact {
    parent: OpenedDirectory,
    file: OpenedRegularFile,
}

impl OpenedArtifact {
    pub(crate) fn path(&self) -> &Path {
        &self.file.path
    }

    pub(crate) fn expected_size(&self) -> u64 {
        self.file.snapshot.size()
    }

    pub(crate) fn read(&mut self, buffer: &mut [u8]) -> Result<usize, GitResolverErrorV1> {
        self.file.file.read(buffer).map_err(|error| {
            map_io(
                &self.file.path,
                GitReadOperationV1::ReadLocalArtifact,
                error,
            )
        })
    }

    pub(crate) fn validate(&self) -> Result<(), GitResolverErrorV1> {
        self.file
            .verify(GitInvariantViolationV1::MetadataChangedDuringArtifactHash)?;
        self.parent
            .verify(GitInvariantViolationV1::MetadataChangedDuringArtifactHash)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceLayoutNativeReport {
    pub(crate) podway_created: bool,
    pub(crate) procedures_created: bool,
    pub(crate) runtime_created: bool,
    pub(crate) ignore_created: bool,
    pub(crate) runtime_ignore_rule_created: bool,
    pub(crate) config_created: Option<bool>,
}

pub(crate) struct WorkspaceLayoutSnapshot {
    root: OpenedDirectory,
    podway: OpenedDirectory,
    procedures: OpenedDirectory,
    runtime: OpenedDirectory,
    ignore: OpenedRegularFile,
    config: Option<OpenedRegularFile>,
    report: WorkspaceLayoutNativeReport,
}

impl WorkspaceLayoutSnapshot {
    pub(crate) fn report(&self) -> WorkspaceLayoutNativeReport {
        self.report
    }

    pub(crate) fn validate(&self) -> Result<(), GitResolverErrorV1> {
        self.root
            .verify(GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout)?;
        self.podway
            .verify(GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout)?;
        if let Some(config) = &self.config {
            config.verify(GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout)?;
        }
        self.procedures
            .verify(GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout)?;
        self.runtime
            .verify(GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout)?;
        ensure_runtime_is_private(&self.runtime)?;
        self.ignore
            .verify(GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout)
    }
}

pub(crate) fn open_workspace_layout_root(
    path: &LosslessPathV1,
) -> Result<OpenedDirectory, GitResolverErrorV1> {
    let path = decode_lossless_path(path)?;
    open_absolute_directory(
        &path,
        GitReadOperationV1::InitializeWorkspaceLayout,
        GitRepresentationProblemV1::WorkspaceLayoutComponentNotDirectory,
    )
}

pub(crate) fn validate_workspace_config_bytes(
    default_config_bytes: &[u8],
) -> Result<(), GitResolverErrorV1> {
    if default_config_bytes.is_empty() || default_config_bytes.len() > MAX_LAYOUT_CONFIG_BYTES {
        return Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::WorkspaceLayoutIgnoreFileTooLarge,
        });
    }
    Ok(())
}

pub(crate) fn initialize_workspace_layout(
    root: OpenedDirectory,
) -> Result<WorkspaceLayoutSnapshot, GitResolverErrorV1> {
    initialize_workspace_layout_inner(root, None)
}

pub(crate) fn initialize_workspace_layout_with_config(
    root: OpenedDirectory,
    default_config_bytes: &[u8],
) -> Result<WorkspaceLayoutSnapshot, GitResolverErrorV1> {
    validate_workspace_config_bytes(default_config_bytes)?;
    initialize_workspace_layout_inner(root, Some(default_config_bytes))
}

fn initialize_workspace_layout_inner(
    root: OpenedDirectory,
    default_config_bytes: Option<&[u8]>,
) -> Result<WorkspaceLayoutSnapshot, GitResolverErrorV1> {
    root.verify(GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout)?;
    #[cfg(test)]
    synchronize_workspace_layout_root_replacement_for_test(root.path());
    preflight_workspace_layout(&root, default_config_bytes.is_some())?;
    let (podway, podway_created) =
        ensure_workspace_directory(&root, OsStr::new(".podway"), Mode::from_raw_mode(0o755))?;
    let (procedures, procedures_created) = ensure_workspace_directory(
        &podway,
        OsStr::new("procedures"),
        Mode::from_raw_mode(0o755),
    )?;
    let (runtime, runtime_created) =
        ensure_workspace_directory(&podway, OsStr::new("runtime"), Mode::from_raw_mode(0o700))?;
    if runtime_created {
        set_new_runtime_private(&runtime)?;
    } else {
        ensure_runtime_is_private(&runtime)?;
    }

    rustix_fs::flock(&podway.file, FlockOperation::LockExclusive).map_err(|error| {
        map_rustix_io(
            &podway.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    let (config, config_created) = match default_config_bytes {
        Some(default_config_bytes) => {
            let (config, created) = ensure_workspace_config(&podway, default_config_bytes)?;
            (Some(config), Some(created))
        }
        None => (None, None),
    };
    let (ignore, ignore_created, runtime_ignore_rule_created) =
        ensure_workspace_ignore_rule(&podway)?;

    let snapshot = WorkspaceLayoutSnapshot {
        root,
        podway,
        config,
        procedures,
        runtime,
        ignore,
        report: WorkspaceLayoutNativeReport {
            podway_created,
            procedures_created,
            runtime_created,
            ignore_created,
            runtime_ignore_rule_created,
            config_created,
        },
    };
    #[cfg(test)]
    if let Some(config) = &snapshot.config {
        synchronize_workspace_layout_config_replacement_for_test(&config.path);
    }
    snapshot.validate()?;
    Ok(snapshot)
}

fn preflight_workspace_layout(
    root: &OpenedDirectory,
    check_config: bool,
) -> Result<(), GitResolverErrorV1> {
    let podway_path = root.path.join(".podway");
    let Some(stat) = entry_stat_if_exists(
        root,
        OsStr::new(".podway"),
        GitReadOperationV1::InitializeWorkspaceLayout,
    )?
    else {
        return Ok(());
    };
    if file_type(&stat) == FileType::Symlink {
        return Err(GitResolverErrorV1::SymlinkEscape {
            path: lossless_path(&podway_path)?,
        });
    }
    if file_type(&stat) != FileType::Directory {
        return Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::WorkspaceLayoutComponentNotDirectory,
        });
    }
    let podway = open_directory_at(
        root,
        OsStr::new(".podway"),
        GitReadOperationV1::InitializeWorkspaceLayout,
        GitRepresentationProblemV1::WorkspaceLayoutComponentNotDirectory,
    )?;
    preflight_workspace_directory(&podway, OsStr::new("procedures"))?;
    preflight_workspace_directory(&podway, OsStr::new("runtime"))?;
    if check_config {
        preflight_workspace_config(&podway)?;
    }

    let ignore_path = podway.path.join(".gitignore");
    match entry_stat_if_exists(
        &podway,
        OsStr::new(".gitignore"),
        GitReadOperationV1::InitializeWorkspaceLayout,
    )? {
        None => Ok(()),
        Some(stat) if file_type(&stat) == FileType::RegularFile => Ok(()),
        Some(stat) if file_type(&stat) == FileType::Symlink => {
            Err(GitResolverErrorV1::SymlinkEscape {
                path: lossless_path(&ignore_path)?,
            })
        }
        Some(_) => Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
        }),
    }
}
fn preflight_workspace_config(podway: &OpenedDirectory) -> Result<(), GitResolverErrorV1> {
    let path = podway.path.join("config.yaml");
    match entry_stat_if_exists(
        podway,
        OsStr::new("config.yaml"),
        GitReadOperationV1::InitializeWorkspaceLayout,
    )? {
        None => Ok(()),
        Some(stat) if file_type(&stat) == FileType::RegularFile => Ok(()),
        Some(stat) if file_type(&stat) == FileType::Symlink => {
            Err(GitResolverErrorV1::SymlinkEscape {
                path: lossless_path(&path)?,
            })
        }
        Some(_) => Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
        }),
    }
}

fn preflight_workspace_directory(
    parent: &OpenedDirectory,
    name: &OsStr,
) -> Result<(), GitResolverErrorV1> {
    let path = parent.path.join(name);
    match entry_stat_if_exists(parent, name, GitReadOperationV1::InitializeWorkspaceLayout)? {
        None => Ok(()),
        Some(stat) if file_type(&stat) == FileType::Symlink => {
            Err(GitResolverErrorV1::SymlinkEscape {
                path: lossless_path(&path)?,
            })
        }
        Some(stat) if file_type(&stat) == FileType::Directory => {
            open_directory_at(
                parent,
                name,
                GitReadOperationV1::InitializeWorkspaceLayout,
                GitRepresentationProblemV1::WorkspaceLayoutComponentNotDirectory,
            )?;
            Ok(())
        }
        Some(_) => Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::WorkspaceLayoutComponentNotDirectory,
        }),
    }
}
fn ensure_workspace_directory(
    parent: &OpenedDirectory,
    name: &OsStr,
    mode: Mode,
) -> Result<(OpenedDirectory, bool), GitResolverErrorV1> {
    let path = parent.path.join(name);
    for _ in 0..MAX_LAYOUT_TEMP_ATTEMPTS {
        let created = match entry_stat_if_exists(
            parent,
            name,
            GitReadOperationV1::InitializeWorkspaceLayout,
        )? {
            Some(stat) if file_type(&stat) == FileType::Symlink => {
                return Err(GitResolverErrorV1::SymlinkEscape {
                    path: lossless_path(&path)?,
                });
            }
            Some(stat) if file_type(&stat) == FileType::Directory => false,
            Some(_) => {
                return Err(GitResolverErrorV1::Representation {
                    problem: GitRepresentationProblemV1::WorkspaceLayoutComponentNotDirectory,
                });
            }
            None => match rustix_fs::mkdirat(&parent.file, name, mode) {
                Ok(()) => {
                    rustix_fs::fsync(&parent.file).map_err(|error| {
                        map_rustix_io(
                            &parent.path,
                            GitReadOperationV1::InitializeWorkspaceLayout,
                            error,
                        )
                    })?;
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(map_rustix_io(
                        &path,
                        GitReadOperationV1::InitializeWorkspaceLayout,
                        error,
                    ));
                }
            },
        };
        let directory = open_directory_at(
            parent,
            name,
            GitReadOperationV1::InitializeWorkspaceLayout,
            GitRepresentationProblemV1::WorkspaceLayoutComponentNotDirectory,
        )?;
        return Ok((directory, created));
    }
    Err(metadata_changed(
        GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
    ))
}
fn ensure_workspace_config(
    podway: &OpenedDirectory,
    default_config_bytes: &[u8],
) -> Result<(OpenedRegularFile, bool), GitResolverErrorV1> {
    for _ in 0..MAX_LAYOUT_TEMP_ATTEMPTS {
        let name = OsStr::new("config.yaml");
        let path = podway.path.join(name);
        match entry_stat_if_exists(podway, name, GitReadOperationV1::InitializeWorkspaceLayout)? {
            Some(stat) if file_type(&stat) == FileType::Symlink => {
                return Err(GitResolverErrorV1::SymlinkEscape {
                    path: lossless_path(&path)?,
                });
            }
            Some(stat) if file_type(&stat) == FileType::RegularFile => {
                let config = open_regular_file_at(
                    podway,
                    name,
                    GitReadOperationV1::InitializeWorkspaceLayout,
                    GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
                )?;
                if !stat_matches_snapshot(&stat, config.snapshot) {
                    return Err(metadata_changed(
                        GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
                    ));
                }
                return Ok((config, false));
            }
            Some(_) => {
                return Err(GitResolverErrorV1::Representation {
                    problem: GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
                });
            }
            None => {}
        }

        let descriptor = match rustix_fs::openat(
            &podway.file,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(descriptor) => descriptor,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(map_rustix_io(
                    &path,
                    GitReadOperationV1::InitializeWorkspaceLayout,
                    error,
                ));
            }
        };
        let mut file = File::from(descriptor);
        let created = FileSnapshot::from_open_file(
            &file,
            &path,
            GitReadOperationV1::InitializeWorkspaceLayout,
        )?;
        if let Err(error) = file.write_all(default_config_bytes) {
            let primary = map_io(&path, GitReadOperationV1::InitializeWorkspaceLayout, error);
            drop(file);
            return Err(with_workspace_layout_cleanup(
                primary,
                remove_new_workspace_config_if_matches(podway, name, created),
            ));
        }
        if let Err(error) = file.sync_all() {
            let primary = map_io(&path, GitReadOperationV1::InitializeWorkspaceLayout, error);
            drop(file);
            return Err(with_workspace_layout_cleanup(
                primary,
                remove_new_workspace_config_if_matches(podway, name, created),
            ));
        }
        let current =
            match entry_stat_if_exists(podway, name, GitReadOperationV1::InitializeWorkspaceLayout)
            {
                Ok(current) => current,
                Err(error) => {
                    drop(file);
                    return Err(with_workspace_layout_cleanup(
                        error,
                        remove_new_workspace_config_if_matches(podway, name, created),
                    ));
                }
            };
        if !matches!(current, Some(stat) if stat_matches_snapshot(&stat, created)) {
            drop(file);
            return Err(with_workspace_layout_cleanup(
                metadata_changed(GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout),
                remove_new_workspace_config_if_matches(podway, name, created),
            ));
        }
        drop(file);
        if let Err(error) = rustix_fs::fsync(&podway.file) {
            let primary = map_rustix_io(
                &podway.path,
                GitReadOperationV1::InitializeWorkspaceLayout,
                error,
            );
            return Err(with_workspace_layout_cleanup(
                primary,
                remove_new_workspace_config_if_matches(podway, name, created),
            ));
        }
        let config = match open_regular_file_at(
            podway,
            name,
            GitReadOperationV1::InitializeWorkspaceLayout,
            GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
        ) {
            Ok(config) => config,
            Err(error) => {
                return Err(with_workspace_layout_cleanup(
                    error,
                    remove_new_workspace_config_if_matches(podway, name, created),
                ));
            }
        };
        if !config.snapshot.same_identity(created) {
            return Err(with_workspace_layout_cleanup(
                metadata_changed(GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout),
                remove_new_workspace_config_if_matches(podway, name, created),
            ));
        }
        return Ok((config, true));
    }

    Err(metadata_changed(
        GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
    ))
}

fn remove_new_workspace_config_if_matches(
    podway: &OpenedDirectory,
    name: &OsStr,
    expected: FileSnapshot,
) -> Result<(), GitResolverErrorV1> {
    let Some(stat) =
        entry_stat_if_exists(podway, name, GitReadOperationV1::InitializeWorkspaceLayout)?
    else {
        return Ok(());
    };
    if !stat_matches_snapshot(&stat, expected) {
        return Ok(());
    }
    let config = open_regular_file_at(
        podway,
        name,
        GitReadOperationV1::InitializeWorkspaceLayout,
        GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
    )?;
    if !config.snapshot.same_identity(expected) {
        return Ok(());
    }
    rustix_fs::unlinkat(&podway.file, name, AtFlags::empty()).map_err(|error| {
        map_path_race_or_io(
            &podway.path.join(name),
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    rustix_fs::fsync(&podway.file).map_err(|error| {
        map_rustix_io(
            &podway.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })
}

fn set_new_runtime_private(runtime: &OpenedDirectory) -> Result<(), GitResolverErrorV1> {
    rustix_fs::fchmod(&runtime.file, Mode::RUSR | Mode::WUSR | Mode::XUSR).map_err(|error| {
        map_rustix_io(
            &runtime.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    rustix_fs::fsync(&runtime.file).map_err(|error| {
        map_rustix_io(
            &runtime.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    let metadata = runtime.file.metadata().map_err(|error| {
        map_io(
            &runtime.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    if metadata.mode() & 0o777 != 0o700 {
        return Err(metadata_changed(
            GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
        ));
    }
    Ok(())
}
fn ensure_runtime_is_private(runtime: &OpenedDirectory) -> Result<(), GitResolverErrorV1> {
    let metadata = runtime.file.metadata().map_err(|error| {
        map_io(
            &runtime.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    let current = metadata.mode() & 0o777;
    if current != 0o700 {
        rustix_fs::fchmod(&runtime.file, Mode::RUSR | Mode::WUSR | Mode::XUSR).map_err(
            |error| {
                map_rustix_io(
                    &runtime.path,
                    GitReadOperationV1::InitializeWorkspaceLayout,
                    error,
                )
            },
        )?;
    }
    rustix_fs::fsync(&runtime.file).map_err(|error| {
        map_rustix_io(
            &runtime.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    let current = runtime.file.metadata().map_err(|error| {
        map_io(
            &runtime.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    if current.mode() & 0o777 != 0o700 {
        return Err(metadata_changed(
            GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
        ));
    }
    Ok(())
}

fn ensure_workspace_ignore_rule(
    podway: &OpenedDirectory,
) -> Result<(OpenedRegularFile, bool, bool), GitResolverErrorV1> {
    for _ in 0..MAX_LAYOUT_TEMP_ATTEMPTS {
        let path = podway.path.join(".gitignore");
        let entry = entry_stat_if_exists(
            podway,
            OsStr::new(".gitignore"),
            GitReadOperationV1::InitializeWorkspaceLayout,
        )?;
        let Some(stat) = entry else {
            let mut staged = create_workspace_ignore_temp(podway, b"runtime/\n", None)?;
            if let Err(error) = staged.verify_pending_name(podway) {
                return Err(staged.abort_pre_exchange(podway, error));
            }
            match rustix_fs::renameat_with(
                &podway.file,
                staged.name(),
                &podway.file,
                ".gitignore",
                RenameFlags::NOREPLACE,
            ) {
                Ok(()) => {
                    staged.release();
                    if let Err(error) = rustix_fs::fsync(&podway.file) {
                        return Err(map_rustix_io(
                            &path,
                            GitReadOperationV1::InitializeWorkspaceLayout,
                            error,
                        ));
                    }
                    let ignore = open_regular_file_at(
                        podway,
                        OsStr::new(".gitignore"),
                        GitReadOperationV1::InitializeWorkspaceLayout,
                        GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
                    )?;
                    return Ok((ignore, true, true));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    staged.cleanup_pending(podway)?;
                    continue;
                }
                Err(error) => {
                    let primary =
                        map_rustix_io(&path, GitReadOperationV1::InitializeWorkspaceLayout, error);
                    return Err(staged.abort_pre_exchange(podway, primary));
                }
            }
        };

        if file_type(&stat) == FileType::Symlink {
            return Err(GitResolverErrorV1::SymlinkEscape {
                path: lossless_path(&path)?,
            });
        }
        if file_type(&stat) != FileType::RegularFile {
            return Err(GitResolverErrorV1::Representation {
                problem: GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
            });
        }

        let mut ignore = open_regular_file_at(
            podway,
            OsStr::new(".gitignore"),
            GitReadOperationV1::InitializeWorkspaceLayout,
            GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
        )?;
        let contents = read_workspace_ignore(&mut ignore)?;
        let Some(updated) = normalized_workspace_ignore(&contents)? else {
            return Ok((ignore, false, false));
        };
        let expected = ignore.snapshot;
        let expected_mode = workspace_ignore_mode(&ignore)?;
        let after_mode = FileSnapshot::from_open_file(
            &ignore.file,
            &ignore.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
        )?;
        if after_mode != expected {
            return Err(metadata_changed(
                GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
            ));
        }
        let mut staged = create_workspace_ignore_temp(podway, &updated, Some(expected_mode))?;
        let replacement = replace_workspace_ignore(
            podway,
            &mut staged,
            expected,
            expected_mode,
            &contents,
            &updated,
        );
        drop(staged);
        match replacement {
            Ok(()) => {
                let ignore = open_regular_file_at(
                    podway,
                    OsStr::new(".gitignore"),
                    GitReadOperationV1::InitializeWorkspaceLayout,
                    GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
                )?;
                return Ok((ignore, false, true));
            }
            Err(error) => return Err(error),
        }
    }

    Err(metadata_changed(
        GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
    ))
}

fn read_workspace_ignore(ignore: &mut OpenedRegularFile) -> Result<Vec<u8>, GitResolverErrorV1> {
    ignore.file.seek(SeekFrom::Start(0)).map_err(|error| {
        map_io(
            &ignore.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    let mut bytes = Vec::with_capacity(
        usize::try_from(ignore.snapshot.size().min(MAX_LAYOUT_IGNORE_BYTES as u64))
            .expect("bounded layout ignore size fits usize"),
    );
    Read::by_ref(&mut ignore.file)
        .take((MAX_LAYOUT_IGNORE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            map_io(
                &ignore.path,
                GitReadOperationV1::InitializeWorkspaceLayout,
                error,
            )
        })?;
    if bytes.len() > MAX_LAYOUT_IGNORE_BYTES {
        return Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::WorkspaceLayoutIgnoreFileTooLarge,
        });
    }
    let after = FileSnapshot::from_open_file(
        &ignore.file,
        &ignore.path,
        GitReadOperationV1::InitializeWorkspaceLayout,
    )?;
    if after != ignore.snapshot {
        return Err(metadata_changed(
            GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
        ));
    }
    Ok(bytes)
}

fn normalized_workspace_ignore(contents: &[u8]) -> Result<Option<Vec<u8>>, GitResolverErrorV1> {
    let lines: Vec<&[u8]> = contents.split_inclusive(|byte| *byte == b'\n').collect();
    let final_owned_rule = lines
        .last()
        .is_some_and(|line| is_runtime_ignore_rule(workspace_ignore_rule_line(line)));

    let mut normalized = Vec::with_capacity(contents.len() + b"runtime/\n".len());
    for (index, line) in lines.iter().enumerate() {
        if is_runtime_ignore_rule(workspace_ignore_rule_line(line))
            && !(final_owned_rule && index + 1 == lines.len())
        {
            continue;
        }
        normalized.extend_from_slice(line);
    }

    if !final_owned_rule {
        let newline: &[u8] = if normalized.ends_with(b"\r\n") {
            b"\r\n"
        } else {
            b"\n"
        };
        if normalized.is_empty() {
            normalized.extend_from_slice(b"runtime/\n");
        } else if normalized.ends_with(b"\n") {
            normalized.extend_from_slice(b"runtime/");
            normalized.extend_from_slice(newline);
        } else {
            normalized.extend_from_slice(newline);
            normalized.extend_from_slice(b"runtime/");
        }
    }

    if normalized.len() > MAX_LAYOUT_IGNORE_BYTES {
        return Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::WorkspaceLayoutIgnoreFileTooLarge,
        });
    }

    Ok((normalized != contents).then_some(normalized))
}

fn workspace_ignore_rule_line(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn is_runtime_ignore_rule(line: &[u8]) -> bool {
    line == b"runtime/"
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceIgnoreStageState {
    Pending,
    Exchanged,
    Released,
}

struct StagedWorkspaceIgnore {
    file: File,
    name: OsString,
    snapshot: FileSnapshot,
    state: WorkspaceIgnoreStageState,
}

impl StagedWorkspaceIgnore {
    fn name(&self) -> &OsStr {
        &self.name
    }

    fn verify_pending_name(&self, podway: &OpenedDirectory) -> Result<(), GitResolverErrorV1> {
        if self.state != WorkspaceIgnoreStageState::Pending {
            return Err(metadata_changed(
                GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
            ));
        }
        let staged = open_regular_file_at(
            podway,
            self.name(),
            GitReadOperationV1::InitializeWorkspaceLayout,
            GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
        )?;
        if !staged.snapshot.same_identity(self.snapshot) {
            return Err(metadata_changed(
                GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
            ));
        }
        Ok(())
    }

    fn cleanup_pending(&mut self, podway: &OpenedDirectory) -> Result<(), GitResolverErrorV1> {
        if self.state != WorkspaceIgnoreStageState::Pending {
            return Ok(());
        }
        self.state = WorkspaceIgnoreStageState::Released;
        let _ = remove_workspace_ignore_temp_if_matches(podway, self.name(), self.snapshot)?;
        Ok(())
    }

    fn abort_pre_exchange(
        &mut self,
        podway: &OpenedDirectory,
        primary: GitResolverErrorV1,
    ) -> GitResolverErrorV1 {
        with_workspace_layout_cleanup(primary, self.cleanup_pending(podway))
    }

    fn mark_exchanged(&mut self) {
        debug_assert_eq!(self.state, WorkspaceIgnoreStageState::Pending);
        self.state = WorkspaceIgnoreStageState::Exchanged;
    }

    fn release(&mut self) {
        self.state = WorkspaceIgnoreStageState::Released;
    }
}

fn create_workspace_ignore_temp(
    podway: &OpenedDirectory,
    contents: &[u8],
    final_mode: Option<Mode>,
) -> Result<StagedWorkspaceIgnore, GitResolverErrorV1> {
    for _ in 0..MAX_LAYOUT_TEMP_ATTEMPTS {
        let sequence = LAYOUT_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = OsString::from(format!(
            ".podway-ignore-{}-{sequence}.tmp",
            std::process::id()
        ));
        let path = podway.path.join(&name);
        let descriptor = match rustix_fs::openat(
            &podway.file,
            &name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(descriptor) => descriptor,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(map_rustix_io(
                    &path,
                    GitReadOperationV1::InitializeWorkspaceLayout,
                    error,
                ));
            }
        };
        let file = File::from(descriptor);
        let snapshot = match FileSnapshot::from_open_file(
            &file,
            &path,
            GitReadOperationV1::InitializeWorkspaceLayout,
        ) {
            Ok(snapshot) => snapshot,
            Err(primary) => {
                drop(file);
                return Err(with_workspace_layout_cleanup(
                    primary,
                    discard_new_workspace_ignore_temp(podway, &name),
                ));
            }
        };
        let mut staged = StagedWorkspaceIgnore {
            file,
            name,
            snapshot,
            state: WorkspaceIgnoreStageState::Pending,
        };
        if let Err(error) = staged.file.write_all(contents) {
            let primary = map_io(&path, GitReadOperationV1::InitializeWorkspaceLayout, error);
            return Err(staged.abort_pre_exchange(podway, primary));
        }
        let chmod_result = final_mode.map(|mode| rustix_fs::fchmod(&staged.file, mode));
        if let Some(Err(error)) = chmod_result {
            let primary =
                map_rustix_io(&path, GitReadOperationV1::InitializeWorkspaceLayout, error);
            return Err(staged.abort_pre_exchange(podway, primary));
        }
        if let Err(error) = staged.file.sync_all() {
            let primary = map_io(&path, GitReadOperationV1::InitializeWorkspaceLayout, error);
            return Err(staged.abort_pre_exchange(podway, primary));
        }
        return Ok(staged);
    }

    Err(metadata_changed(
        GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
    ))
}

fn workspace_ignore_mode(ignore: &OpenedRegularFile) -> Result<Mode, GitResolverErrorV1> {
    let metadata = ignore.file.metadata().map_err(|error| {
        map_io(
            &ignore.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    Ok(Mode::from_raw_mode(
        (metadata.mode() & 0o777)
            .try_into()
            .expect("permission mask fits the platform mode type"),
    ))
}

fn exchange_workspace_ignore_entries(
    podway: &OpenedDirectory,
    temporary_name: &OsStr,
    ignore_name: &OsStr,
) -> Result<(), rustix::io::Errno> {
    #[cfg(test)]
    if take_workspace_ignore_rename_failure(WorkspaceIgnoreRenameFailure::Exchange) {
        return Err(rustix::io::Errno::NOENT);
    }

    rustix_fs::renameat_with(
        &podway.file,
        temporary_name,
        &podway.file,
        ignore_name,
        RenameFlags::EXCHANGE,
    )
}

fn replace_workspace_ignore(
    podway: &OpenedDirectory,
    staged: &mut StagedWorkspaceIgnore,
    expected: FileSnapshot,
    expected_mode: Mode,
    expected_contents: &[u8],
    replacement_contents: &[u8],
) -> Result<(), GitResolverErrorV1> {
    #[cfg(test)]
    if take_workspace_ignore_pre_exchange_failure(WorkspaceIgnorePreExchangeFailure::CurrentStat) {
        return Err(staged.abort_pre_exchange(podway, injected_workspace_ignore_cleanup_failure()));
    }

    let current = match entry_stat_if_exists(
        podway,
        OsStr::new(".gitignore"),
        GitReadOperationV1::InitializeWorkspaceLayout,
    ) {
        Ok(current) => current,
        Err(error) => return Err(staged.abort_pre_exchange(podway, error)),
    };
    if !matches!(current, Some(stat) if stat_matches_snapshot(&stat, expected)) {
        let primary =
            metadata_changed(GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout);
        return Err(staged.abort_pre_exchange(podway, primary));
    }

    let temporary_path = podway.path.join(staged.name());
    #[cfg(test)]
    if take_workspace_ignore_pre_exchange_failure(
        WorkspaceIgnorePreExchangeFailure::TemporarySnapshot,
    ) {
        return Err(staged.abort_pre_exchange(podway, injected_workspace_ignore_cleanup_failure()));
    }
    let desired = match FileSnapshot::from_open_file(
        &staged.file,
        &temporary_path,
        GitReadOperationV1::InitializeWorkspaceLayout,
    ) {
        Ok(desired) => desired,
        Err(error) => return Err(staged.abort_pre_exchange(podway, error)),
    };

    #[cfg(test)]
    mutate_workspace_ignore_after_precheck_for_test(&temporary_path);

    if let Err(error) = staged.verify_pending_name(podway) {
        return Err(staged.abort_pre_exchange(podway, error));
    }

    #[cfg(test)]
    mutate_workspace_ignore_after_precheck_for_test(&podway.path.join(".gitignore"));

    if let Err(error) =
        exchange_workspace_ignore_entries(podway, staged.name(), OsStr::new(".gitignore"))
    {
        let primary = map_path_race_or_io(
            &podway.path.join(".gitignore"),
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        );
        return Err(staged.abort_pre_exchange(podway, primary));
    }
    staged.mark_exchanged();

    if let Err(error) = verify_workspace_ignore_exchange(
        podway,
        staged.name(),
        expected,
        expected_mode,
        expected_contents,
        desired,
        replacement_contents,
    ) {
        let primary = map_revalidation_error(
            error,
            GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
        );
        return Err(quarantine_workspace_ignore_after_conflict(
            podway, staged, primary,
        ));
    }

    staged.release();
    match remove_workspace_ignore_temp_if_matches(podway, staged.name(), expected) {
        Ok(true) => Ok(()),
        Ok(false) => Err(quarantine_workspace_ignore_after_conflict(
            podway,
            staged,
            metadata_changed(GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout),
        )),
        Err(error) => Err(error),
    }
}

fn verify_workspace_ignore_exchange(
    podway: &OpenedDirectory,
    temporary_name: &OsStr,
    expected: FileSnapshot,
    expected_mode: Mode,
    expected_contents: &[u8],
    desired: FileSnapshot,
    replacement_contents: &[u8],
) -> Result<(), GitResolverErrorV1> {
    let mut displaced = open_regular_file_at(
        podway,
        temporary_name,
        GitReadOperationV1::InitializeWorkspaceLayout,
        GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
    )?;
    let displaced_contents = read_workspace_ignore(&mut displaced)?;
    let displaced_mode = workspace_ignore_mode(&displaced)?;
    if !displaced.snapshot.same_identity(expected)
        || displaced_contents != expected_contents
        || displaced_mode != expected_mode
    {
        return Err(metadata_changed(
            GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
        ));
    }

    let mut installed = open_regular_file_at(
        podway,
        OsStr::new(".gitignore"),
        GitReadOperationV1::InitializeWorkspaceLayout,
        GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
    )?;
    let installed_contents = read_workspace_ignore(&mut installed)?;
    let installed_mode = workspace_ignore_mode(&installed)?;
    if !installed.snapshot.same_identity(desired)
        || installed_contents != replacement_contents
        || installed_mode != expected_mode
    {
        return Err(metadata_changed(
            GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
        ));
    }
    Ok(())
}

fn quarantine_workspace_ignore_after_conflict(
    podway: &OpenedDirectory,
    staged: &mut StagedWorkspaceIgnore,
    primary: GitResolverErrorV1,
) -> GitResolverErrorV1 {
    staged.release();
    with_workspace_layout_cleanup(primary, sync_workspace_ignore_quarantine(podway))
}

fn sync_workspace_ignore_quarantine(podway: &OpenedDirectory) -> Result<(), GitResolverErrorV1> {
    #[cfg(test)]
    if take_workspace_ignore_cleanup_failure(WorkspaceIgnoreCleanupFailure::QuarantineSync) {
        return Err(injected_workspace_ignore_cleanup_failure());
    }

    rustix_fs::fsync(&podway.file).map_err(|error| {
        map_rustix_io(
            &podway.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })
}

fn discard_new_workspace_ignore_temp(
    podway: &OpenedDirectory,
    temporary_name: &OsStr,
) -> Result<(), GitResolverErrorV1> {
    let temporary_path = podway.path.join(temporary_name);
    rustix_fs::unlinkat(&podway.file, temporary_name, AtFlags::empty()).map_err(|error| {
        map_path_race_or_io(
            &temporary_path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    rustix_fs::fsync(&podway.file).map_err(|error| {
        map_rustix_io(
            &podway.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })
}

fn remove_workspace_ignore_temp_if_matches(
    podway: &OpenedDirectory,
    temporary_name: &OsStr,
    expected: FileSnapshot,
) -> Result<bool, GitResolverErrorV1> {
    let Some(stat) = entry_stat_if_exists(
        podway,
        temporary_name,
        GitReadOperationV1::InitializeWorkspaceLayout,
    )?
    else {
        return Ok(false);
    };
    if !stat_matches_snapshot(&stat, expected) {
        return Ok(false);
    }
    let temporary = open_regular_file_at(
        podway,
        temporary_name,
        GitReadOperationV1::InitializeWorkspaceLayout,
        GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
    )?;
    if !temporary.snapshot.same_identity(expected) {
        return Ok(false);
    }
    let temporary_path = podway.path.join(temporary_name);
    rustix_fs::unlinkat(&podway.file, temporary_name, AtFlags::empty()).map_err(|error| {
        map_path_race_or_io(
            &temporary_path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    rustix_fs::fsync(&podway.file).map_err(|error| {
        map_rustix_io(
            &podway.path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            error,
        )
    })?;
    Ok(true)
}

fn with_workspace_layout_cleanup(
    primary: GitResolverErrorV1,
    cleanup: Result<(), GitResolverErrorV1>,
) -> GitResolverErrorV1 {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup) => GitResolverErrorV1::WorkspaceLayoutCleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        },
    }
}

pub(crate) fn decode_lossless_path(path: &LosslessPathV1) -> Result<PathBuf, GitResolverErrorV1> {
    let bytes = path
        .decode_path_bytes()
        .map_err(GitResolverErrorV1::Selector)?;
    let native = PathBuf::from(OsString::from_vec(bytes));
    if !native.is_absolute() {
        return Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::InvalidPathEncoding,
        });
    }
    Ok(native)
}

pub(crate) fn lossless_path(path: &Path) -> Result<LosslessPathV1, GitResolverErrorV1> {
    let bytes = path.as_os_str().as_bytes();
    let display = path.as_os_str().to_string_lossy().into_owned();
    let display = DiagnosticPathDisplayV1::new(display).map_err(GitResolverErrorV1::Selector)?;
    LosslessPathV1::from_raw_bytes(bytes, display).map_err(GitResolverErrorV1::Selector)
}

pub(crate) fn is_strictly_contained(root: &Path, child: &Path) -> bool {
    child != root && child.starts_with(root)
}

pub(crate) fn discover_worktree(start: PathBuf) -> Result<DiscoveredLayout, GitResolverErrorV1> {
    let mut root = open_absolute_directory(
        &start,
        GitReadOperationV1::CanonicalizePath,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?;
    let mut discovery_candidates = Vec::new();
    loop {
        let marker_name = OsStr::new(".git");
        let marker_path = root.path.join(marker_name);
        match entry_stat_if_exists(&root, marker_name, GitReadOperationV1::DiscoverRepository)? {
            None => {
                if is_bare_repository_signature(&root)? {
                    return Err(GitResolverErrorV1::BareRepository);
                }
            }
            Some(stat) if file_type(&stat) == FileType::Symlink => {
                return Err(GitResolverErrorV1::SymlinkEscape {
                    path: lossless_path(&marker_path)?,
                });
            }
            Some(stat) if file_type(&stat) == FileType::Directory => {
                let git_directory = open_directory_at(
                    &root,
                    marker_name,
                    GitReadOperationV1::ReadGitDirectory,
                    GitRepresentationProblemV1::MalformedGitDirectory,
                )?;
                return discover_main_worktree(root, git_directory, discovery_candidates);
            }
            Some(stat) if file_type(&stat) == FileType::RegularFile => {
                let mut marker = open_regular_file_at(
                    &root,
                    marker_name,
                    GitReadOperationV1::ReadGitFile,
                    GitRepresentationProblemV1::MalformedGitFile,
                )?;
                let marker_bytes = read_bounded_regular(
                    &mut marker,
                    GitReadOperationV1::ReadGitFile,
                    GitRepresentationProblemV1::MalformedGitFile,
                )?;
                return discover_linked_worktree(root, marker, marker_bytes, discovery_candidates);
            }
            Some(_) => {
                return Err(GitResolverErrorV1::Representation {
                    problem: GitRepresentationProblemV1::MalformedGitDirectory,
                });
            }
        }

        let Some(parent) = root.path().parent().map(Path::to_path_buf) else {
            return Err(GitResolverErrorV1::NonGitRepository);
        };
        if discovery_candidates.len() == MAX_DISCOVERY_CANDIDATES {
            return Err(GitResolverErrorV1::Representation {
                problem: GitRepresentationProblemV1::UnsupportedRepositoryLayout,
            });
        }
        discovery_candidates.push(DiscoveryCandidate { directory: root });
        root = open_absolute_directory(
            &parent,
            GitReadOperationV1::DiscoverRepository,
            GitRepresentationProblemV1::UnsupportedRepositoryLayout,
        )?;
    }
}

fn discover_main_worktree(
    root: OpenedDirectory,
    git_directory: OpenedDirectory,
    discovery_candidates: Vec<DiscoveryCandidate>,
) -> Result<DiscoveredLayout, GitResolverErrorV1> {
    let mut head = open_regular_file_at(
        &git_directory,
        OsStr::new("HEAD"),
        GitReadOperationV1::ReadWorktreeMetadata,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?;
    validate_head_record(&read_bounded_regular(
        &mut head,
        GitReadOperationV1::ReadWorktreeMetadata,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?)?;
    let objects = open_directory_at(
        &git_directory,
        OsStr::new("objects"),
        GitReadOperationV1::ReadGitDirectory,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?;
    let refs = open_directory_at(
        &git_directory,
        OsStr::new("refs"),
        GitReadOperationV1::ReadGitDirectory,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?;
    let marker_path = git_directory.path.clone();
    let common = open_absolute_directory(
        &marker_path,
        GitReadOperationV1::ReadGitDirectory,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?;
    let administration = open_absolute_directory(
        &marker_path,
        GitReadOperationV1::ReadGitDirectory,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?;

    #[cfg(test)]
    create_discovery_marker_for_test();

    Ok(DiscoveredLayout {
        worktree_root: root,
        git_marker: GitMarker::Directory(git_directory),
        common_directory_root: common,
        worktree_administration_root: administration,
        discovery_candidates,
        supporting_directories: vec![objects, refs],
        metadata_records: vec![head],
        kind: WorktreeKindV1::Main,
    })
}

fn discover_linked_worktree(
    root: OpenedDirectory,
    marker: OpenedRegularFile,
    marker_bytes: Vec<u8>,
    discovery_candidates: Vec<DiscoveryCandidate>,
) -> Result<DiscoveredLayout, GitResolverErrorV1> {
    let marker_path = marker.path.clone();
    let administration_reference = resolve_metadata_path(
        root.path(),
        parse_gitdir_record(&marker_bytes, GitRepresentationProblemV1::MalformedGitFile)?,
        GitRepresentationProblemV1::MalformedGitFile,
    )?;
    let administration = open_absolute_directory(
        &administration_reference,
        GitReadOperationV1::ValidateWorktreeAdministration,
        GitRepresentationProblemV1::MalformedGitFile,
    )?;

    let mut common_record = open_regular_file_at(
        &administration,
        OsStr::new("commondir"),
        GitReadOperationV1::ReadGitFile,
        GitRepresentationProblemV1::MalformedCommonDirectory,
    )?;
    let common_record_bytes = read_bounded_regular(
        &mut common_record,
        GitReadOperationV1::ReadGitFile,
        GitRepresentationProblemV1::MalformedCommonDirectory,
    )?;
    let common_reference = resolve_metadata_path(
        administration.path(),
        parse_plain_path_record(
            &common_record_bytes,
            GitRepresentationProblemV1::MalformedCommonDirectory,
        )?,
        GitRepresentationProblemV1::MalformedCommonDirectory,
    )?;
    let common = open_absolute_directory(
        &common_reference,
        GitReadOperationV1::ValidateCommonDirectory,
        GitRepresentationProblemV1::MalformedCommonDirectory,
    )?;

    if !is_supported_worktree_administration(common.path(), administration.path()) {
        return Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::UnsupportedAdministrationRelationship,
        });
    }
    let worktrees = open_directory_at(
        &common,
        OsStr::new("worktrees"),
        GitReadOperationV1::ValidateWorktreeAdministration,
        GitRepresentationProblemV1::UnsupportedAdministrationRelationship,
    )?;

    let mut backlink_record = open_regular_file_at(
        &administration,
        OsStr::new("gitdir"),
        GitReadOperationV1::ReadGitFile,
        GitRepresentationProblemV1::MalformedLinkedBacklink,
    )?;
    let backlink_record_bytes = read_bounded_regular(
        &mut backlink_record,
        GitReadOperationV1::ReadGitFile,
        GitRepresentationProblemV1::MalformedLinkedBacklink,
    )?;
    let backlink_reference = resolve_absolute_metadata_path(
        parse_plain_path_record(
            &backlink_record_bytes,
            GitRepresentationProblemV1::MalformedLinkedBacklink,
        )?,
        GitRepresentationProblemV1::MalformedLinkedBacklink,
    )?;
    if backlink_reference != marker_path {
        return Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::MalformedLinkedBacklink,
        });
    }
    let backlink_marker = open_absolute_regular_file(
        &backlink_reference,
        GitReadOperationV1::ValidateWorktreeAdministration,
        GitRepresentationProblemV1::MalformedLinkedBacklink,
    )?;
    if !backlink_marker.snapshot.same_identity(marker.snapshot) {
        return Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::MalformedLinkedBacklink,
        });
    }

    let mut administration_head = open_regular_file_at(
        &administration,
        OsStr::new("HEAD"),
        GitReadOperationV1::ReadWorktreeMetadata,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?;
    validate_head_record(&read_bounded_regular(
        &mut administration_head,
        GitReadOperationV1::ReadWorktreeMetadata,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?)?;
    let mut common_head = open_regular_file_at(
        &common,
        OsStr::new("HEAD"),
        GitReadOperationV1::ReadWorktreeMetadata,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?;
    validate_head_record(&read_bounded_regular(
        &mut common_head,
        GitReadOperationV1::ReadWorktreeMetadata,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?)?;
    let objects = open_directory_at(
        &common,
        OsStr::new("objects"),
        GitReadOperationV1::ReadGitDirectory,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?;
    let refs = open_directory_at(
        &common,
        OsStr::new("refs"),
        GitReadOperationV1::ReadGitDirectory,
        GitRepresentationProblemV1::UnsupportedRepositoryLayout,
    )?;

    #[cfg(test)]
    create_discovery_marker_for_test();
    Ok(DiscoveredLayout {
        worktree_root: root,
        git_marker: GitMarker::File(marker),
        common_directory_root: common,
        worktree_administration_root: administration,
        discovery_candidates,
        supporting_directories: vec![worktrees, objects, refs],
        metadata_records: vec![
            common_record,
            backlink_record,
            administration_head,
            common_head,
        ],
        kind: WorktreeKindV1::Linked,
    })
}

fn is_supported_worktree_administration(common: &Path, administration: &Path) -> bool {
    let Ok(relative) = administration.strip_prefix(common) else {
        return false;
    };
    let mut components = relative.components();
    matches!(
        (components.next(), components.next(), components.next()),
        (
            Some(Component::Normal(worktrees)),
            Some(Component::Normal(name)),
            None,
        ) if worktrees == OsStr::new("worktrees") && !name.as_bytes().is_empty()
    )
}

fn is_bare_repository_signature(root: &OpenedDirectory) -> Result<bool, GitResolverErrorV1> {
    let head = entry_stat_if_exists(
        root,
        OsStr::new("HEAD"),
        GitReadOperationV1::DiscoverRepository,
    )?;
    let objects = entry_stat_if_exists(
        root,
        OsStr::new("objects"),
        GitReadOperationV1::DiscoverRepository,
    )?;
    let refs = entry_stat_if_exists(
        root,
        OsStr::new("refs"),
        GitReadOperationV1::DiscoverRepository,
    )?;
    Ok(
        matches!(head, Some(stat) if file_type(&stat) == FileType::RegularFile)
            && matches!(objects, Some(stat) if file_type(&stat) == FileType::Directory)
            && matches!(refs, Some(stat) if file_type(&stat) == FileType::Directory),
    )
}

pub(crate) fn fingerprint_directory(
    directory: &OpenedDirectory,
    role: &'static [u8],
    kind: &WorktreeKindV1,
) -> Result<Sha256Digest, GitResolverErrorV1> {
    fingerprint_directory_with_invariant(
        directory,
        role,
        kind,
        GitInvariantViolationV1::MetadataChangedDuringResolution,
    )
}

pub(crate) fn validate_workspace_layout_root(
    root: &OpenedDirectory,
    expected_fingerprint: &Sha256Digest,
    kind: &WorktreeKindV1,
) -> Result<(), GitResolverErrorV1> {
    let actual = fingerprint_directory_with_invariant(
        root,
        b"worktree-root",
        kind,
        GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
    )?;
    if &actual != expected_fingerprint {
        return Err(metadata_changed(
            GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
        ));
    }
    Ok(())
}

fn fingerprint_directory_with_invariant(
    directory: &OpenedDirectory,
    role: &'static [u8],
    kind: &WorktreeKindV1,
    invariant: GitInvariantViolationV1,
) -> Result<Sha256Digest, GitResolverErrorV1> {
    directory.verify(invariant)?;
    let identity = directory.snapshot.identity;
    let mut hasher = Sha256::new();
    hasher.update(b"podway-git-directory-fingerprint\0");
    hasher.update(2_u16.to_be_bytes());
    write_frame(&mut hasher, role);
    write_frame(&mut hasher, layout_kind_tag(kind));
    write_frame(&mut hasher, platform_tag());
    write_frame(&mut hasher, &identity.device.to_be_bytes());
    write_frame(&mut hasher, &identity.inode.to_be_bytes());
    write_frame(&mut hasher, identity.file_type.tag());
    match identity.creation {
        #[cfg(target_os = "macos")]
        CreationEvidence::Darwin {
            birth_seconds,
            birth_nanoseconds,
        } => {
            write_frame(&mut hasher, b"darwin-birthtime");
            write_frame(&mut hasher, &birth_seconds.to_be_bytes());
            write_frame(&mut hasher, &birth_nanoseconds.to_be_bytes());
        }
        #[cfg(target_os = "linux")]
        CreationEvidence::Linux {
            device_major,
            device_minor,
            birth_seconds,
            birth_nanoseconds,
        } => {
            write_frame(&mut hasher, b"linux-statx");
            write_frame(&mut hasher, &device_major.to_be_bytes());
            write_frame(&mut hasher, &device_minor.to_be_bytes());
            write_frame(&mut hasher, &birth_seconds.to_be_bytes());
            write_frame(&mut hasher, &birth_nanoseconds.to_be_bytes());
        }
    }
    digest_from_hasher(hasher)
}

pub(crate) fn inspect_containment(
    root: &OpenedDirectory,
) -> Result<ContainmentSnapshot, GitResolverErrorV1> {
    let podway = match entry_stat_if_exists(
        root,
        OsStr::new(".podway"),
        GitReadOperationV1::InspectRuntimeDirectory,
    )? {
        None => None,
        Some(stat) if file_type(&stat) == FileType::Symlink => {
            return Err(GitResolverErrorV1::SymlinkEscape {
                path: lossless_path(&root.path.join(".podway"))?,
            });
        }
        Some(stat) if file_type(&stat) == FileType::Directory => Some(open_directory_at(
            root,
            OsStr::new(".podway"),
            GitReadOperationV1::InspectRuntimeDirectory,
            GitRepresentationProblemV1::UnsupportedRepositoryLayout,
        )?),
        Some(_) => {
            return Err(GitResolverErrorV1::Representation {
                problem: GitRepresentationProblemV1::UnsupportedRepositoryLayout,
            });
        }
    };
    let runtime = match &podway {
        None => None,
        Some(podway) => match entry_stat_if_exists(
            podway,
            OsStr::new("runtime"),
            GitReadOperationV1::InspectRuntimeDirectory,
        )? {
            None => None,
            Some(stat) if file_type(&stat) == FileType::Symlink => {
                return Err(GitResolverErrorV1::Invariant {
                    problem: GitInvariantViolationV1::RuntimeDirectoryIsSymlink,
                });
            }
            Some(stat) if file_type(&stat) == FileType::Directory => Some(open_directory_at(
                podway,
                OsStr::new("runtime"),
                GitReadOperationV1::InspectRuntimeDirectory,
                GitRepresentationProblemV1::UnsupportedRepositoryLayout,
            )?),
            Some(_) => {
                return Err(GitResolverErrorV1::Representation {
                    problem: GitRepresentationProblemV1::UnsupportedRepositoryLayout,
                });
            }
        },
    };
    Ok(ContainmentSnapshot { podway, runtime })
}

pub(crate) fn open_artifact_beneath(
    root: &OpenedDirectory,
    artifact: &Path,
) -> Result<OpenedArtifact, GitResolverErrorV1> {
    let relative = artifact
        .strip_prefix(root.path())
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .ok_or(GitResolverErrorV1::Invariant {
            problem: GitInvariantViolationV1::MetadataChangedDuringArtifactHash,
        })?;
    let components: Vec<&OsStr> = relative
        .components()
        .map(|component| match component {
            Component::Normal(component) => Ok(component),
            _ => Err(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringArtifactHash,
            }),
        })
        .collect::<Result<_, _>>()?;
    let (last, parents) = components
        .split_last()
        .ok_or(GitResolverErrorV1::Invariant {
            problem: GitInvariantViolationV1::MetadataChangedDuringArtifactHash,
        })?;
    let mut parent = duplicate_directory(root, GitReadOperationV1::OpenLocalArtifact)?;
    for component in parents {
        parent = open_directory_at(
            &parent,
            component,
            GitReadOperationV1::OpenLocalArtifact,
            GitRepresentationProblemV1::NonRegularArtifact,
        )?;
    }
    let file = open_regular_file_at(
        &parent,
        last,
        GitReadOperationV1::OpenLocalArtifact,
        GitRepresentationProblemV1::NonRegularArtifact,
    )?;
    Ok(OpenedArtifact { parent, file })
}

fn open_absolute_directory(
    path: &Path,
    operation: GitReadOperationV1,
    problem: GitRepresentationProblemV1,
) -> Result<OpenedDirectory, GitResolverErrorV1> {
    if !path.is_absolute() {
        return Err(GitResolverErrorV1::Representation { problem });
    }
    let root_file = rustix_fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| map_rustix_io(Path::new("/"), operation.clone(), error))?;
    let mut current =
        OpenedDirectory::from_file(File::from(root_file), PathBuf::from("/"), operation.clone())?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                current = open_directory_at(&current, name, operation.clone(), problem.clone())?;
            }
            Component::CurDir | Component::ParentDir => {
                return Err(GitResolverErrorV1::Representation { problem });
            }
            _ => return Err(GitResolverErrorV1::Representation { problem }),
        }
    }
    Ok(current)
}

fn open_absolute_regular_file(
    path: &Path,
    operation: GitReadOperationV1,
    problem: GitRepresentationProblemV1,
) -> Result<OpenedRegularFile, GitResolverErrorV1> {
    let parent_path = path
        .parent()
        .ok_or_else(|| GitResolverErrorV1::Representation {
            problem: problem.clone(),
        })?;
    let name = path
        .file_name()
        .ok_or_else(|| GitResolverErrorV1::Representation {
            problem: problem.clone(),
        })?;
    let parent = open_absolute_directory(parent_path, operation.clone(), problem.clone())?;
    open_regular_file_at(&parent, name, operation, problem)
}

fn open_directory_at(
    parent: &OpenedDirectory,
    name: &OsStr,
    operation: GitReadOperationV1,
    problem: GitRepresentationProblemV1,
) -> Result<OpenedDirectory, GitResolverErrorV1> {
    let path = parent.path.join(name);
    let invariant = invariant_for_operation(&operation);
    let before = entry_stat_if_exists(parent, name, operation.clone())?.ok_or_else(|| {
        if operation == GitReadOperationV1::CanonicalizePath {
            GitResolverErrorV1::Io {
                operation: GitReadOperationV1::CanonicalizePath,
            }
        } else {
            GitResolverErrorV1::Representation {
                problem: problem.clone(),
            }
        }
    })?;
    match file_type(&before) {
        FileType::Symlink => {
            return Err(GitResolverErrorV1::SymlinkEscape {
                path: lossless_path(&path)?,
            });
        }
        FileType::Directory => {}
        _ => return Err(GitResolverErrorV1::Representation { problem }),
    }
    let file = match rustix_fs::openat(
        &parent.file,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => File::from(file),
        Err(error) => return Err(map_path_race_or_io(&path, operation, error)),
    };
    let opened = OpenedDirectory::from_file(file, path, operation.clone())?;
    let after = entry_stat_if_exists(parent, name, operation.clone())?;
    if !matches!(after, Some(stat) if stat_matches_snapshot(&stat, opened.snapshot)) {
        return Err(metadata_changed(invariant));
    }
    Ok(opened)
}

fn open_regular_file_at(
    parent: &OpenedDirectory,
    name: &OsStr,
    operation: GitReadOperationV1,
    problem: GitRepresentationProblemV1,
) -> Result<OpenedRegularFile, GitResolverErrorV1> {
    let path = parent.path.join(name);
    let invariant = invariant_for_operation(&operation);
    let before = entry_stat_if_exists(parent, name, operation.clone())?.ok_or_else(|| {
        GitResolverErrorV1::Representation {
            problem: problem.clone(),
        }
    })?;
    match file_type(&before) {
        FileType::Symlink => {
            return Err(GitResolverErrorV1::SymlinkEscape {
                path: lossless_path(&path)?,
            });
        }
        FileType::RegularFile => {}
        _ => return Err(GitResolverErrorV1::Representation { problem }),
    }
    #[cfg(all(test, target_os = "linux"))]
    replace_regular_file_with_fifo_for_test(&path);
    let file = match rustix_fs::openat(
        &parent.file,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => File::from(file),
        Err(error) => return Err(map_path_race_or_io(&path, operation, error)),
    };
    let opened = OpenedRegularFile::from_file(file, path, operation.clone())?;
    let after = entry_stat_if_exists(parent, name, operation.clone())?;
    if !matches!(after, Some(stat) if stat_matches_snapshot(&stat, opened.snapshot)) {
        return Err(metadata_changed(invariant));
    }
    Ok(opened)
}

fn duplicate_directory(
    directory: &OpenedDirectory,
    operation: GitReadOperationV1,
) -> Result<OpenedDirectory, GitResolverErrorV1> {
    let file = rustix_fs::openat(
        &directory.file,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| map_rustix_io(directory.path(), operation.clone(), error))?;
    let duplicate =
        OpenedDirectory::from_file(File::from(file), directory.path.clone(), operation)?;
    if !duplicate.snapshot.same_identity(directory.snapshot) {
        return Err(metadata_changed(
            GitInvariantViolationV1::MetadataChangedDuringArtifactHash,
        ));
    }
    Ok(duplicate)
}

fn entry_stat_if_exists(
    parent: &OpenedDirectory,
    name: &OsStr,
    operation: GitReadOperationV1,
) -> Result<Option<rustix_fs::Stat>, GitResolverErrorV1> {
    match rustix_fs::statat(&parent.file, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(stat)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(map_rustix_io(&parent.path.join(name), operation, error)),
    }
}

fn ensure_child_missing(
    parent: &OpenedDirectory,
    name: &OsStr,
    invariant: GitInvariantViolationV1,
) -> Result<(), GitResolverErrorV1> {
    match entry_stat_if_exists(parent, name, GitReadOperationV1::InspectRuntimeDirectory)? {
        None => Ok(()),
        Some(_) => Err(metadata_changed(invariant)),
    }
}

fn file_type(stat: &rustix_fs::Stat) -> FileType {
    FileType::from_raw_mode(stat.st_mode)
}

fn stat_matches_snapshot(stat: &rustix_fs::Stat, snapshot: FileSnapshot) -> bool {
    let expected_type = match snapshot.identity.file_type {
        StableFileType::Directory => FileType::Directory,
        StableFileType::Regular => FileType::RegularFile,
    };
    file_type(stat) == expected_type
        && stat.st_dev as u64 == snapshot.identity.device
        && stat.st_ino == snapshot.identity.inode
}

fn read_bounded_regular(
    file: &mut OpenedRegularFile,
    operation: GitReadOperationV1,
    problem: GitRepresentationProblemV1,
) -> Result<Vec<u8>, GitResolverErrorV1> {
    file.file
        .seek(SeekFrom::Start(0))
        .map_err(|error| map_io(&file.path, operation.clone(), error))?;
    let mut bytes = Vec::with_capacity(metadata_capacity(file.snapshot.size()));
    Read::by_ref(&mut file.file)
        .take((MAX_GIT_METADATA_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| map_io(&file.path, operation.clone(), error))?;
    let after = FileSnapshot::from_open_file(&file.file, &file.path, operation)?;
    if after != file.snapshot {
        return Err(metadata_changed(
            GitInvariantViolationV1::MetadataChangedDuringResolution,
        ));
    }
    if bytes.len() > MAX_GIT_METADATA_BYTES {
        return Err(GitResolverErrorV1::Representation { problem });
    }
    Ok(bytes)
}

fn parse_gitdir_record(
    bytes: &[u8],
    problem: GitRepresentationProblemV1,
) -> Result<&[u8], GitResolverErrorV1> {
    let Some(line) = single_bounded_line(bytes) else {
        return Err(GitResolverErrorV1::Representation { problem });
    };
    let Some(path) = line.strip_prefix(b"gitdir: ") else {
        return Err(GitResolverErrorV1::Representation { problem });
    };
    if path.is_empty() || path[0] == b' ' || path.contains(&0) {
        return Err(GitResolverErrorV1::Representation { problem });
    }
    Ok(path)
}

fn parse_plain_path_record(
    bytes: &[u8],
    problem: GitRepresentationProblemV1,
) -> Result<&[u8], GitResolverErrorV1> {
    let Some(path) = single_bounded_line(bytes) else {
        return Err(GitResolverErrorV1::Representation { problem });
    };
    if path.is_empty() || path[0] == b' ' || path.contains(&0) {
        return Err(GitResolverErrorV1::Representation { problem });
    }
    Ok(path)
}

fn validate_head_record(bytes: &[u8]) -> Result<(), GitResolverErrorV1> {
    let Some(line) = single_bounded_line(bytes) else {
        return Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::UnsupportedRepositoryLayout,
        });
    };
    let valid_symbolic = line
        .strip_prefix(b"ref: refs/")
        .filter(|reference| !reference.is_empty())
        .is_some_and(valid_ref_suffix);
    let valid_detached = matches!(line.len(), 40 | 64)
        && line
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f' | b'A'..=b'F'));
    if valid_symbolic || valid_detached {
        Ok(())
    } else {
        Err(GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::UnsupportedRepositoryLayout,
        })
    }
}

fn valid_ref_suffix(reference: &[u8]) -> bool {
    reference.split(|byte| *byte == b'/').all(|component| {
        !component.is_empty()
            && component != b"."
            && component != b".."
            && !component.iter().any(|byte| {
                byte.is_ascii_control()
                    || byte.is_ascii_whitespace()
                    || matches!(byte, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
            })
    })
}

fn single_bounded_line(bytes: &[u8]) -> Option<&[u8]> {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    (!bytes.is_empty() && !bytes.contains(&b'\n') && !bytes.contains(&b'\r')).then_some(bytes)
}

fn resolve_metadata_path(
    base: &Path,
    bytes: &[u8],
    problem: GitRepresentationProblemV1,
) -> Result<PathBuf, GitResolverErrorV1> {
    if bytes.contains(&0) {
        return Err(GitResolverErrorV1::Representation { problem });
    }
    let reference = PathBuf::from(OsString::from_vec(bytes.to_vec()));
    let combined = if reference.is_absolute() {
        reference
    } else {
        base.join(reference)
    };
    normalize_absolute_path(&combined, problem)
}

fn resolve_absolute_metadata_path(
    bytes: &[u8],
    problem: GitRepresentationProblemV1,
) -> Result<PathBuf, GitResolverErrorV1> {
    if bytes.starts_with(b"//") || bytes.contains(&0) {
        return Err(GitResolverErrorV1::Representation { problem });
    }
    let reference = PathBuf::from(OsString::from_vec(bytes.to_vec()));
    if !reference.is_absolute() {
        return Err(GitResolverErrorV1::Representation { problem });
    }
    normalize_absolute_path(&reference, problem)
}

fn normalize_absolute_path(
    path: &Path,
    problem: GitRepresentationProblemV1,
) -> Result<PathBuf, GitResolverErrorV1> {
    if !path.is_absolute() {
        return Err(GitResolverErrorV1::Representation { problem });
    }
    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => normalized.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => return Err(GitResolverErrorV1::Representation { problem }),
        }
    }
    Ok(normalized)
}

fn object_identity(
    _file: &File,
    metadata: &Metadata,
    _path: &Path,
    _operation: GitReadOperationV1,
) -> Result<ObjectIdentity, GitResolverErrorV1> {
    let file_type = if metadata.is_dir() {
        StableFileType::Directory
    } else if metadata.is_file() {
        StableFileType::Regular
    } else {
        return Err(GitResolverErrorV1::Invariant {
            problem: GitInvariantViolationV1::UnsupportedFilesystemIdentity,
        });
    };

    #[cfg(target_os = "macos")]
    {
        let birth_seconds = MacMetadataExt::st_birthtime(metadata);
        let birth_nanoseconds = MacMetadataExt::st_birthtime_nsec(metadata);
        if birth_seconds == 0 && birth_nanoseconds == 0 {
            return Err(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::UnsupportedFilesystemIdentity,
            });
        }
        Ok(ObjectIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
            file_type,
            creation: CreationEvidence::Darwin {
                birth_seconds,
                birth_nanoseconds,
            },
        })
    }

    #[cfg(target_os = "linux")]
    {
        let requested = rustix_fs::StatxFlags::BASIC_STATS | rustix_fs::StatxFlags::BTIME;
        let stat = rustix_fs::statx(_file, "", AtFlags::EMPTY_PATH, requested)
            .map_err(|error| map_statx_identity_error(_path, _operation, error))?;
        let required =
            rustix_fs::StatxFlags::TYPE | rustix_fs::StatxFlags::INO | rustix_fs::StatxFlags::BTIME;
        let expected_type = match file_type {
            StableFileType::Directory => FileType::Directory,
            StableFileType::Regular => FileType::RegularFile,
        };
        if !stat.stx_mask.contains(required)
            || stat.stx_ino != metadata.ino()
            || FileType::from_raw_mode(stat.stx_mode.into()) != expected_type
        {
            return Err(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::UnsupportedFilesystemIdentity,
            });
        }
        Ok(ObjectIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
            file_type,
            creation: CreationEvidence::Linux {
                device_major: stat.stx_dev_major,
                device_minor: stat.stx_dev_minor,
                birth_seconds: stat.stx_btime.tv_sec,
                birth_nanoseconds: stat.stx_btime.tv_nsec,
            },
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (_file, metadata, file_type);
        Err(GitResolverErrorV1::Invariant {
            problem: GitInvariantViolationV1::UnsupportedFilesystemIdentity,
        })
    }
}

#[cfg(target_os = "linux")]
fn map_statx_identity_error(
    path: &Path,
    operation: GitReadOperationV1,
    error: rustix::io::Errno,
) -> GitResolverErrorV1 {
    if matches!(
        error,
        rustix::io::Errno::NOSYS | rustix::io::Errno::NOTSUP | rustix::io::Errno::INVAL
    ) {
        GitResolverErrorV1::Invariant {
            problem: GitInvariantViolationV1::UnsupportedFilesystemIdentity,
        }
    } else {
        map_path_race_or_io(path, operation, error)
    }
}

fn metadata_capacity(length: u64) -> usize {
    length.min(MAX_GIT_METADATA_BYTES as u64) as usize
}

fn metadata_changed(problem: GitInvariantViolationV1) -> GitResolverErrorV1 {
    GitResolverErrorV1::Invariant { problem }
}

fn write_frame(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u32).to_be_bytes());
    hasher.update(value);
}

fn digest_from_hasher(hasher: Sha256) -> Result<Sha256Digest, GitResolverErrorV1> {
    let bytes = hasher.finalize();
    let mut rendered = String::with_capacity("sha256:".len() + bytes.len() * 2);
    rendered.push_str("sha256:");
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(rendered, "{byte:02x}");
    }
    Sha256Digest::new(rendered).map_err(|_| GitResolverErrorV1::Invariant {
        problem: GitInvariantViolationV1::UnsupportedFilesystemIdentity,
    })
}

fn layout_kind_tag(kind: &WorktreeKindV1) -> &'static [u8] {
    match kind {
        WorktreeKindV1::Main => b"main",
        WorktreeKindV1::Linked => b"linked",
    }
}

#[cfg(target_os = "macos")]
fn platform_tag() -> &'static [u8] {
    b"darwin"
}

#[cfg(target_os = "linux")]
fn platform_tag() -> &'static [u8] {
    b"linux"
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_tag() -> &'static [u8] {
    b"unix-other"
}

pub(crate) fn path_is_missing(
    path: &Path,
    operation: GitReadOperationV1,
) -> Result<bool, GitResolverErrorV1> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(map_io(path, operation, error)),
    }
}
fn map_rustix_io(
    path: &Path,
    operation: GitReadOperationV1,
    error: rustix::io::Errno,
) -> GitResolverErrorV1 {
    map_io(path, operation, error.into())
}
fn invariant_for_operation(operation: &GitReadOperationV1) -> GitInvariantViolationV1 {
    match operation {
        GitReadOperationV1::OpenLocalArtifact | GitReadOperationV1::ReadLocalArtifact => {
            GitInvariantViolationV1::MetadataChangedDuringArtifactHash
        }
        GitReadOperationV1::InitializeWorkspaceLayout => {
            GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout
        }
        GitReadOperationV1::DiscoverRepository
        | GitReadOperationV1::ReadGitFile
        | GitReadOperationV1::ReadGitDirectory
        | GitReadOperationV1::CanonicalizePath
        | GitReadOperationV1::ReadWorktreeMetadata
        | GitReadOperationV1::ValidateCommonDirectory
        | GitReadOperationV1::ValidateWorktreeAdministration
        | GitReadOperationV1::InspectRuntimeDirectory => {
            GitInvariantViolationV1::MetadataChangedDuringResolution
        }
    }
}

fn is_concrete_path_race_errno(error: rustix::io::Errno) -> bool {
    matches!(
        error,
        rustix::io::Errno::NOENT
            | rustix::io::Errno::NOTDIR
            | rustix::io::Errno::ISDIR
            | rustix::io::Errno::LOOP
            | rustix::io::Errno::STALE
    )
}

fn map_path_race_or_io(
    path: &Path,
    operation: GitReadOperationV1,
    error: rustix::io::Errno,
) -> GitResolverErrorV1 {
    if is_concrete_path_race_errno(error) {
        metadata_changed(invariant_for_operation(&operation))
    } else {
        map_rustix_io(path, operation, error)
    }
}

fn map_revalidation_error(
    error: GitResolverErrorV1,
    invariant: GitInvariantViolationV1,
) -> GitResolverErrorV1 {
    match error {
        GitResolverErrorV1::Invariant {
            problem:
                GitInvariantViolationV1::MetadataChangedDuringResolution
                | GitInvariantViolationV1::MetadataChangedDuringArtifactHash
                | GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
        }
        | GitResolverErrorV1::Representation { .. }
        | GitResolverErrorV1::SymlinkEscape { .. } => metadata_changed(invariant),
        error => error,
    }
}

fn map_io(path: &Path, operation: GitReadOperationV1, error: std::io::Error) -> GitResolverErrorV1 {
    if let (std::io::ErrorKind::PermissionDenied, Ok(path)) = (error.kind(), lossless_path(path)) {
        return GitResolverErrorV1::PermissionDenied { path };
    }
    GitResolverErrorV1::Io { operation }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_main(root: &Path) {
        let administration = root.join(".git");
        fs::create_dir_all(administration.join("objects")).expect("objects directory");
        fs::create_dir_all(administration.join("refs")).expect("refs directory");
        fs::write(administration.join("HEAD"), b"ref: refs/heads/main\n").expect("HEAD record");
    }
    fn stage_workspace_ignore_replacement(
        podway_path: &Path,
    ) -> (
        OpenedDirectory,
        PathBuf,
        StagedWorkspaceIgnore,
        OsString,
        FileSnapshot,
        Mode,
        Vec<u8>,
        Vec<u8>,
    ) {
        fs::create_dir(podway_path).expect("podway directory");
        let podway_path = fs::canonicalize(podway_path).expect("canonical podway directory");
        let ignore_path = podway_path.join(".gitignore");
        fs::write(&ignore_path, b"keep\n").expect("initial ignore");

        let podway = open_absolute_directory(
            &podway_path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            GitRepresentationProblemV1::WorkspaceLayoutComponentNotDirectory,
        )
        .expect("opened podway directory");
        let mut ignore = open_regular_file_at(
            &podway,
            OsStr::new(".gitignore"),
            GitReadOperationV1::InitializeWorkspaceLayout,
            GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
        )
        .expect("opened ignore");
        let expected_contents = read_workspace_ignore(&mut ignore).expect("read initial ignore");
        let expected = ignore.snapshot;
        let expected_mode = workspace_ignore_mode(&ignore).expect("ignore mode");
        let updated = normalized_workspace_ignore(&expected_contents)
            .expect("normalization result")
            .expect("normalization required");
        let replacement = create_workspace_ignore_temp(&podway, &updated, Some(expected_mode))
            .expect("staged replacement");
        let replacement_name = replacement.name().to_os_string();
        (
            podway,
            ignore_path,
            replacement,
            replacement_name,
            expected,
            expected_mode,
            expected_contents,
            updated,
        )
    }

    #[test]
    fn nearer_candidate_created_after_discovery_is_metadata_change() {
        let temporary = tempdir().expect("temporary directory");
        let outer = temporary.path().join("outer");
        let child = outer.join("child");
        create_main(&outer);
        fs::create_dir_all(&child).expect("candidate directory");
        let child = fs::canonicalize(&child).expect("canonical candidate directory");

        *DISCOVERY_MARKER_CREATION_TARGET
            .lock()
            .expect("discovery test hook lock") = Some(child.join(".git"));
        let layout = discover_worktree(child).expect("outer worktree discovery");
        assert!(matches!(
            layout.validate_resolution(),
            Err(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringResolution
            })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn regular_file_replaced_with_fifo_before_open_is_rejected_without_reading() {
        let temporary = tempdir().expect("temporary directory");
        let record = temporary.path().join("record");
        fs::write(&record, b"regular").expect("regular record");
        let parent = open_absolute_directory(
            temporary.path(),
            GitReadOperationV1::ReadGitFile,
            GitRepresentationProblemV1::MalformedGitFile,
        )
        .expect("opened parent directory");

        *REGULAR_FILE_FIFO_REPLACEMENT_TARGET
            .lock()
            .expect("regular-file test hook lock") = Some(record);
        assert!(matches!(
            open_regular_file_at(
                &parent,
                OsStr::new("record"),
                GitReadOperationV1::ReadGitFile,
                GitRepresentationProblemV1::MalformedGitFile,
            ),
            Err(GitResolverErrorV1::Representation {
                problem: GitRepresentationProblemV1::NonRegularArtifact
            })
        ));
    }
    #[test]
    fn forward_exchange_quarantines_same_inode_rewrite_without_promoting_it() {
        let _test_guard = WORKSPACE_IGNORE_TEST_LOCK
            .lock()
            .expect("workspace ignore test lock");
        let temporary = tempdir().expect("temporary directory");
        let (
            podway,
            ignore_path,
            mut staged,
            temporary_name,
            expected,
            expected_mode,
            expected_contents,
            updated,
        ) = stage_workspace_ignore_replacement(&temporary.path().join(".podway"));

        *WORKSPACE_IGNORE_MUTATION
            .lock()
            .expect("workspace ignore test hook lock") = Some((
            ignore_path.clone(),
            WorkspaceIgnoreMutation::Rewrite(b"race\n".to_vec()),
        ));
        let result = replace_workspace_ignore(
            &podway,
            &mut staged,
            expected,
            expected_mode,
            &expected_contents,
            &updated,
        );
        drop(staged);

        assert!(matches!(
            result,
            Err(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout
            })
        ));
        assert_eq!(
            fs::read(&ignore_path).expect("normalized replacement"),
            updated
        );
        assert_eq!(
            fs::read(podway.path.join(&temporary_name)).expect("quarantined concurrent bytes"),
            b"race\n"
        );
    }

    #[test]
    fn forward_exchange_quarantines_concurrent_mode_without_overwriting_it() {
        let _test_guard = WORKSPACE_IGNORE_TEST_LOCK
            .lock()
            .expect("workspace ignore test lock");
        let temporary = tempdir().expect("temporary directory");
        let podway_path = temporary.path().join(".podway");
        fs::create_dir(&podway_path).expect("podway directory");
        let podway_path = fs::canonicalize(podway_path).expect("canonical podway directory");
        let ignore_path = podway_path.join(".gitignore");
        fs::write(&ignore_path, b"keep\n").expect("initial ignore");
        fs::set_permissions(&ignore_path, fs::Permissions::from_mode(0o640))
            .expect("initial ignore mode");

        let podway = open_absolute_directory(
            &podway_path,
            GitReadOperationV1::InitializeWorkspaceLayout,
            GitRepresentationProblemV1::WorkspaceLayoutComponentNotDirectory,
        )
        .expect("opened podway directory");
        let mut ignore = open_regular_file_at(
            &podway,
            OsStr::new(".gitignore"),
            GitReadOperationV1::InitializeWorkspaceLayout,
            GitRepresentationProblemV1::WorkspaceLayoutIgnoreNotRegular,
        )
        .expect("opened ignore");
        let expected_contents = read_workspace_ignore(&mut ignore).expect("read initial ignore");
        let expected = ignore.snapshot;
        let expected_mode = workspace_ignore_mode(&ignore).expect("ignore mode");
        let updated = normalized_workspace_ignore(&expected_contents)
            .expect("normalization result")
            .expect("normalization required");
        let mut staged = create_workspace_ignore_temp(&podway, &updated, Some(expected_mode))
            .expect("staged replacement");
        let temporary_name = staged.name().to_os_string();

        *WORKSPACE_IGNORE_MUTATION
            .lock()
            .expect("workspace ignore test hook lock") =
            Some((ignore_path.clone(), WorkspaceIgnoreMutation::Chmod(0o600)));
        let result = replace_workspace_ignore(
            &podway,
            &mut staged,
            expected,
            expected_mode,
            &expected_contents,
            &updated,
        );
        drop(staged);

        assert!(matches!(
            result,
            Err(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout
            })
        ));
        assert_eq!(
            fs::read(&ignore_path).expect("normalized replacement"),
            updated
        );
        assert_eq!(
            fs::metadata(&ignore_path)
                .expect("normalized replacement mode")
                .mode()
                & 0o777,
            0o640
        );
        let quarantined = podway.path.join(&temporary_name);
        assert_eq!(
            fs::read(&quarantined).expect("quarantined bytes"),
            b"keep\n"
        );
        assert_eq!(
            fs::metadata(&quarantined)
                .expect("quarantined concurrent mode")
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn exchange_rename_race_maps_to_layout_metadata_change() {
        let _test_guard = WORKSPACE_IGNORE_TEST_LOCK
            .lock()
            .expect("workspace ignore test lock");
        let temporary = tempdir().expect("temporary directory");
        let (
            podway,
            ignore_path,
            mut replacement,
            _replacement_name,
            expected,
            expected_mode,
            expected_contents,
            updated,
        ) = stage_workspace_ignore_replacement(&temporary.path().join(".podway"));

        *WORKSPACE_IGNORE_RENAME_FAILURE
            .lock()
            .expect("workspace ignore test hook lock") =
            Some(WorkspaceIgnoreRenameFailure::Exchange);
        let exchange = replace_workspace_ignore(
            &podway,
            &mut replacement,
            expected,
            expected_mode,
            &expected_contents,
            &updated,
        )
        .expect_err("injected exchange race");
        drop(replacement);
        assert!(matches!(
            exchange,
            GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout
            }
        ));
        assert_eq!(fs::read(&ignore_path).expect("preserved ignore"), b"keep\n");
    }

    #[test]
    fn descriptor_open_error_mapping_uses_active_operation_and_preserves_resource_io() {
        let path = Path::new("/workspace/entry");

        assert!(matches!(
            map_path_race_or_io(
                path,
                GitReadOperationV1::OpenLocalArtifact,
                rustix::io::Errno::NOENT,
            ),
            GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringArtifactHash
            }
        ));
        assert!(matches!(
            map_path_race_or_io(
                path,
                GitReadOperationV1::InitializeWorkspaceLayout,
                rustix::io::Errno::NOTDIR,
            ),
            GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout
            }
        ));
        assert!(matches!(
            map_path_race_or_io(
                path,
                GitReadOperationV1::OpenLocalArtifact,
                rustix::io::Errno::MFILE,
            ),
            GitResolverErrorV1::Io {
                operation: GitReadOperationV1::OpenLocalArtifact
            }
        ));
        assert!(matches!(
            map_path_race_or_io(
                path,
                GitReadOperationV1::InitializeWorkspaceLayout,
                rustix::io::Errno::MFILE,
            ),
            GitResolverErrorV1::Io {
                operation: GitReadOperationV1::InitializeWorkspaceLayout
            }
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn statx_identity_error_mapping_preserves_capability_and_operational_failures() {
        let path = Path::new("/workspace/entry");

        assert!(matches!(
            map_statx_identity_error(
                path,
                GitReadOperationV1::ReadGitFile,
                rustix::io::Errno::ACCES,
            ),
            GitResolverErrorV1::PermissionDenied { .. }
        ));
        assert!(matches!(
            map_statx_identity_error(
                path,
                GitReadOperationV1::ReadGitFile,
                rustix::io::Errno::NOENT,
            ),
            GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringResolution
            }
        ));
        assert!(matches!(
            map_statx_identity_error(
                path,
                GitReadOperationV1::ReadGitFile,
                rustix::io::Errno::AGAIN,
            ),
            GitResolverErrorV1::Io {
                operation: GitReadOperationV1::ReadGitFile
            }
        ));
        assert!(matches!(
            map_statx_identity_error(
                path,
                GitReadOperationV1::ReadGitFile,
                rustix::io::Errno::NOSYS,
            ),
            GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::UnsupportedFilesystemIdentity
            }
        ));
    }
    #[test]
    fn quarantine_sync_failure_preserves_primary_and_quarantined_bytes() {
        let _test_guard = WORKSPACE_IGNORE_TEST_LOCK
            .lock()
            .expect("workspace ignore test lock");
        let temporary = tempdir().expect("temporary directory");
        let (
            podway,
            ignore_path,
            mut staged,
            temporary_name,
            expected,
            expected_mode,
            expected_contents,
            updated,
        ) = stage_workspace_ignore_replacement(&temporary.path().join(".podway"));

        *WORKSPACE_IGNORE_MUTATION
            .lock()
            .expect("workspace ignore test hook lock") = Some((
            ignore_path.clone(),
            WorkspaceIgnoreMutation::Rewrite(b"race\n".to_vec()),
        ));
        *WORKSPACE_IGNORE_CLEANUP_FAILURE
            .lock()
            .expect("workspace ignore cleanup test hook lock") =
            Some(WorkspaceIgnoreCleanupFailure::QuarantineSync);
        let result = replace_workspace_ignore(
            &podway,
            &mut staged,
            expected,
            expected_mode,
            &expected_contents,
            &updated,
        );
        drop(staged);

        assert!(matches!(
            result,
            Err(GitResolverErrorV1::WorkspaceLayoutCleanup {
                primary,
                cleanup,
            }) if matches!(
                primary.as_ref(),
                GitResolverErrorV1::Invariant {
                    problem: GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout
                }
            ) && matches!(
                cleanup.as_ref(),
                GitResolverErrorV1::Io {
                    operation: GitReadOperationV1::InitializeWorkspaceLayout
                }
            )
        ));
        assert_eq!(
            fs::read(&ignore_path).expect("normalized replacement"),
            updated
        );
        assert_eq!(
            fs::read(podway.path.join(&temporary_name)).expect("quarantined bytes"),
            b"race\n"
        );
    }

    #[test]
    fn temporary_name_substitution_never_promotes_unverified_bytes() {
        let _test_guard = WORKSPACE_IGNORE_TEST_LOCK
            .lock()
            .expect("workspace ignore test lock");
        let temporary = tempdir().expect("temporary directory");
        let (
            podway,
            ignore_path,
            mut staged,
            temporary_name,
            expected,
            expected_mode,
            expected_contents,
            updated,
        ) = stage_workspace_ignore_replacement(&temporary.path().join(".podway"));

        *WORKSPACE_IGNORE_MUTATION
            .lock()
            .expect("workspace ignore test hook lock") = Some((
            podway.path.join(&temporary_name),
            WorkspaceIgnoreMutation::Replace(b"attacker bytes\n".to_vec()),
        ));
        let result = replace_workspace_ignore(
            &podway,
            &mut staged,
            expected,
            expected_mode,
            &expected_contents,
            &updated,
        );
        assert!(matches!(
            result,
            Err(GitResolverErrorV1::Invariant {
                problem: GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout
            })
        ));
        drop(staged);

        assert_eq!(fs::read(&ignore_path).expect("original ignore"), b"keep\n");
        assert_eq!(
            fs::read(podway.path.join(&temporary_name)).expect("quarantined attacker bytes"),
            b"attacker bytes\n"
        );
    }

    #[test]
    fn pre_exchange_failures_cleanup_staged_entries() {
        let _test_guard = WORKSPACE_IGNORE_TEST_LOCK
            .lock()
            .expect("workspace ignore test lock");
        for failure in [
            WorkspaceIgnorePreExchangeFailure::CurrentStat,
            WorkspaceIgnorePreExchangeFailure::TemporarySnapshot,
        ] {
            let temporary = tempdir().expect("temporary directory");
            let (
                podway,
                ignore_path,
                mut staged,
                temporary_name,
                expected,
                expected_mode,
                expected_contents,
                updated,
            ) = stage_workspace_ignore_replacement(&temporary.path().join(".podway"));
            *WORKSPACE_IGNORE_PRE_EXCHANGE_FAILURE
                .lock()
                .expect("workspace ignore test hook lock") = Some(failure);
            let result = replace_workspace_ignore(
                &podway,
                &mut staged,
                expected,
                expected_mode,
                &expected_contents,
                &updated,
            );
            assert!(matches!(
                result,
                Err(GitResolverErrorV1::Io {
                    operation: GitReadOperationV1::InitializeWorkspaceLayout
                })
            ));
            drop(staged);
            assert_eq!(fs::read(&ignore_path).expect("original ignore"), b"keep\n");
            assert!(!podway.path.join(&temporary_name).exists());
        }
    }
}
