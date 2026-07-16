//! Daemon-authoritative, read-only workspace resolution.
//!
//! Git first establishes a validated worktree snapshot. The Store binding is then inspected
//! without mutation, and Git is resolved again with the durable binding before a caller may open
//! or update SQLite. Paths remain diagnostic and routing data; the scheduler key is derived only
//! from the store-validated UUID and Git administration fingerprints.

use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

use podway_core::WorkspaceId;
use podway_git::{
    DiagnosticPathDisplayV1, DurableWorktreeIdentityV1 as GitWorktreeIdentityV1, GitResolveErrorV1,
    GitResolverContractV1, LosslessPathV1, SelectorValidationErrorV1, ValidatedWorktreeV1,
    WorkspaceIdentityStateV1, WorktreeMoveMetadataV1, WorktreeSelectorV1,
};
use podway_store::{
    DurableWorktreeIdentityV1 as StoreWorktreeIdentityV1, SqliteStoreOptionsV1, SqliteStoreV1,
    StoreErrorV1, StoreValueErrorV1, ValidatedWorkspaceRootV1, WorkspaceBindingV1,
};

use crate::scheduler::WorkspaceSchedulerKeyV1;

const STATE_DATABASE_FILE_NAME_V1: &str = "state.sqlite3";
const STORED_ROOT_DIAGNOSTIC_V1: &str = "store-validated workspace root";

/// Read-only access to the durable binding for one exact SQLite database path.
///
/// Implementations must neither create a database nor recover or update one. Keeping this
/// capability separate from `StoreContractV1` prevents resolution from acquiring Store mutation
/// authority before both Git observations agree.
pub trait WorkspaceBindingInspectorV1: Send + Sync {
    fn inspect_workspace_binding(
        &self,
        database_path: &Path,
    ) -> Result<Option<WorkspaceBindingV1>, WorkspaceBindingInspectionErrorV1>;
}

/// SQLite-backed read-only workspace binding inspector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteWorkspaceBindingInspectorV1 {
    options: SqliteStoreOptionsV1,
}

impl SqliteWorkspaceBindingInspectorV1 {
    pub fn new(options: SqliteStoreOptionsV1) -> Self {
        Self { options }
    }

    pub fn options(&self) -> &SqliteStoreOptionsV1 {
        &self.options
    }
}

impl WorkspaceBindingInspectorV1 for SqliteWorkspaceBindingInspectorV1 {
    fn inspect_workspace_binding(
        &self,
        database_path: &Path,
    ) -> Result<Option<WorkspaceBindingV1>, WorkspaceBindingInspectionErrorV1> {
        if let Some(parent) = database_path.parent()
            && matches!(
                fs::symlink_metadata(parent),
                Err(error) if error.kind() == io::ErrorKind::NotFound
            )
        {
            return Ok(None);
        }
        SqliteStoreV1::inspect_workspace_binding(database_path, &self.options)
            .map_err(WorkspaceBindingInspectionErrorV1::Store)
    }
}

/// Failures from the read-only Store binding inspection boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceBindingInspectionErrorV1 {
    Store(StoreErrorV1),
}

impl fmt::Display for WorkspaceBindingInspectionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("workspace binding inspection failed"),
        }
    }
}

impl Error for WorkspaceBindingInspectionErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(source) => Some(source),
        }
    }
}

/// The Git observation that failed during two-pass workspace resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceGitObservationV1 {
    Preliminary,
    BoundRevalidation,
    CandidateRevalidation,
}

/// Exact failures that prevent a workspace from receiving daemon authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceResolutionErrorV1 {
    Selector {
        source: SelectorValidationErrorV1,
    },
    Git {
        observation: WorkspaceGitObservationV1,
        source: GitResolveErrorV1,
    },
    BindingInspection {
        source: WorkspaceBindingInspectionErrorV1,
    },
    ExistingBindingMissing,
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
            Self::BindingInspection { .. } => {
                formatter.write_str("workspace binding inspection failed")
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
            Self::BindingInspection { source } => Some(source),
            Self::WorkspaceRootConversion { source } => Some(source),
            Self::Selector { .. }
            | Self::Git { .. }
            | Self::StoredRootPathInvalid { .. }
            | Self::WorkspaceRootPathInvalid { .. }
            | Self::RuntimeDirectoryPathInvalid { .. }
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
/// None of these fields uses display text or a filesystem path as identity. `database_path` is
/// only the exact local SQLite location that the Store must open after resolution succeeds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedWorkspaceV1 {
    worktree: ValidatedWorktreeV1,
    store_identity: StoreWorktreeIdentityV1,
    workspace_root: ValidatedWorkspaceRootV1,
    database_path: PathBuf,
    scheduler_key: WorkspaceSchedulerKeyV1,
    move_metadata: WorktreeMoveMetadataV1,
}

impl ResolvedWorkspaceV1 {
    fn new(
        worktree: ValidatedWorktreeV1,
        database_path: PathBuf,
    ) -> Result<Self, WorkspaceResolutionErrorV1> {
        let store_identity = store_identity_from_git(worktree.identity());
        let workspace_root = store_root_from_worktree(&worktree)?;
        let scheduler_key = WorkspaceSchedulerKeyV1::from_durable_identity(&store_identity);
        let move_metadata = worktree.move_metadata().clone();
        Ok(Self {
            worktree,
            store_identity,
            workspace_root,
            database_path,
            scheduler_key,
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

    pub fn scheduler_key(&self) -> &WorkspaceSchedulerKeyV1 {
        &self.scheduler_key
    }

    pub fn move_metadata(&self) -> &WorktreeMoveMetadataV1 {
        &self.move_metadata
    }
}

/// Resolves a selector by observing Git, then SQLite binding state, then Git again.
///
/// `R` and `I` are injected so the orchestration can be tested with deterministic Git race and
/// Store-inspection fixtures. Neither dependency receives mutation authority here.
pub struct WorkspaceResolverV1<R, I> {
    git_resolver: R,
    binding_inspector: I,
}

impl<R, I> WorkspaceResolverV1<R, I>
where
    R: GitResolverContractV1,
    I: WorkspaceBindingInspectorV1,
{
    pub fn new(git_resolver: R, binding_inspector: I) -> Self {
        Self {
            git_resolver,
            binding_inspector,
        }
    }

    pub fn git_resolver(&self) -> &R {
        &self.git_resolver
    }

    pub fn binding_inspector(&self) -> &I {
        &self.binding_inspector
    }

    /// Resolves an already-bound workspace. A missing database is always an error; this method
    /// never promotes the preliminary Git candidate into a durable UUID.
    pub fn resolve_existing(
        &self,
        selector: WorktreeSelectorV1,
        expected_workspace_id: Option<&WorkspaceId>,
    ) -> Result<ResolvedWorkspaceV1, WorkspaceResolutionErrorV1> {
        let preliminary_selector = preliminary_selector(&selector)?;
        let preliminary = self
            .git_resolver
            .resolve(preliminary_selector)
            .map_err(|source| WorkspaceResolutionErrorV1::Git {
                observation: WorkspaceGitObservationV1::Preliminary,
                source,
            })?;
        require_candidate_identity(&preliminary)?;

        let database_path = database_path_from_worktree(&preliminary)?;
        let binding = self
            .binding_inspector
            .inspect_workspace_binding(&database_path)
            .map_err(|source| WorkspaceResolutionErrorV1::BindingInspection { source })?
            .ok_or(WorkspaceResolutionErrorV1::ExistingBindingMissing)?;
        let stored_identity = binding.identity();

        if let Some(expected) = expected_workspace_id
            && expected != stored_identity.workspace_uuid()
        {
            return Err(WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch {
                expected: expected.clone(),
                actual: stored_identity.workspace_uuid().clone(),
            });
        }

        require_matching_git_fingerprints(&preliminary, stored_identity)?;
        let bound_identity = bound_git_identity(&preliminary, &binding)?;
        let bound_selector = WorktreeSelectorV1::new(
            selector.version(),
            Some(bound_identity),
            selector.path().clone(),
        )
        .map_err(|source| WorkspaceResolutionErrorV1::Selector { source })?;
        let revalidated = self
            .git_resolver
            .resolve(bound_selector)
            .map_err(|source| WorkspaceResolutionErrorV1::Git {
                observation: WorkspaceGitObservationV1::BoundRevalidation,
                source,
            })?;

        require_revalidated_identity(
            &revalidated,
            WorkspaceIdentityStateV1::Bound,
            stored_identity,
        )?;
        let revalidated_database_path = require_stable_database_path(&database_path, &revalidated)?;
        ResolvedWorkspaceV1::new(revalidated, revalidated_database_path)
    }

    /// Resolves a workspace that has no durable database binding yet.
    ///
    /// The returned validated worktree retains Git's `Candidate` identity. The accompanying Store
    /// identity is suitable only for the caller's atomic Store bind/open operation; this method
    /// itself creates and updates no SQLite state.
    pub fn resolve_bootstrap(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<ResolvedWorkspaceV1, WorkspaceResolutionErrorV1> {
        let preliminary_selector = preliminary_selector(&selector)?;
        let preliminary = self
            .git_resolver
            .resolve(preliminary_selector)
            .map_err(|source| WorkspaceResolutionErrorV1::Git {
                observation: WorkspaceGitObservationV1::Preliminary,
                source,
            })?;
        require_candidate_identity(&preliminary)?;

        let database_path = database_path_from_worktree(&preliminary)?;
        if self
            .binding_inspector
            .inspect_workspace_binding(&database_path)
            .map_err(|source| WorkspaceResolutionErrorV1::BindingInspection { source })?
            .is_some()
        {
            return Err(WorkspaceResolutionErrorV1::BootstrapBindingAlreadyPresent);
        }

        let candidate_selector = WorktreeSelectorV1::new(
            selector.version(),
            Some(preliminary.identity().clone()),
            selector.path().clone(),
        )
        .map_err(|source| WorkspaceResolutionErrorV1::Selector { source })?;
        let revalidated = self
            .git_resolver
            .resolve(candidate_selector)
            .map_err(|source| WorkspaceResolutionErrorV1::Git {
                observation: WorkspaceGitObservationV1::CandidateRevalidation,
                source,
            })?;
        require_revalidated_identity(
            &revalidated,
            WorkspaceIdentityStateV1::Candidate,
            &store_identity_from_git(preliminary.identity()),
        )?;

        let revalidated_database_path = require_stable_database_path(&database_path, &revalidated)?;
        ResolvedWorkspaceV1::new(revalidated, revalidated_database_path)
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
    stored: &StoreWorktreeIdentityV1,
) -> Result<(), WorkspaceResolutionErrorV1> {
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
