//! Native production adapters for daemon-owned execution.
//!
//! This module keeps host concerns at the daemon boundary.  Workspace identity is always
//! re-established through the Store/Git two-pass resolver; local artifact bytes remain outside
//! durable state and are read only through the descriptor-anchored Git resolver.

use std::time::{SystemTime, UNIX_EPOCH};

use podway_config::MAX_PROCEDURE_DOCUMENT_BYTES;
use podway_core::{
    ArtifactLocationKindV1, ArtifactValueV1, AttemptId, BlockerId, DomainError, ItemId, JobId,
    ProcedureSnapshotId, SessionId, UnixMillis,
};
use podway_git::{
    Base64UrlPathBytesV1, DiagnosticPathDisplayV1, GitInvariantViolationV1, GitReadOperationV1,
    GitRepresentationProblemV1, GitResolverErrorV1, LosslessPathV1, NativeGitResolverV1,
    WORKTREE_SELECTOR_VERSION_V1, WorktreeSelectorV1,
};
use podway_protocol::WorktreeSelectorWireV1;
use podway_store::WorkspaceBindingV1;
use uuid::Uuid;

use crate::{
    execution::{
        ArtifactVerifierV1, EmbeddedPresetProcedureProviderV1, ExecutionBoundaryErrorV1,
        ExecutionClockV1, ExecutionIdSourceV1, LocalArtifactVerificationV2, ProcedureProviderV1,
        WorkspaceRevalidatorV1,
    },
    workspace::{
        ResolvedWorkspaceV1, WorkspaceBindingInspectorV1, WorkspaceResolutionErrorV1,
        WorkspaceResolverV1,
    },
};

/// Version of the embedded, extension-only media-type mapping.
pub const EMBEDDED_MEDIA_TYPE_MAPPING_VERSION_V1: u8 = 1;

const ARTIFACT_PATH_DISPLAY_V1: &str = "worktree-local artifact";
const BOUND_ROOT_DISPLAY_V1: &str = "store-bound workspace root";
const DEFAULT_ARTIFACT_MEDIA_TYPE_V1: &str = "application/octet-stream";

/// The v1 deterministic fallback table. It intentionally never reads filesystem metadata or
/// artifact bytes: supplied media types always take precedence.
const EMBEDDED_MEDIA_TYPES_V1: &[(&str, &str)] = &[
    ("csv", "text/csv"),
    ("gif", "image/gif"),
    ("gz", "application/gzip"),
    ("htm", "text/html"),
    ("html", "text/html"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("json", "application/json"),
    ("md", "text/markdown"),
    ("pdf", "application/pdf"),
    ("png", "image/png"),
    ("rs", "text/plain"),
    ("svg", "image/svg+xml"),
    ("tar", "application/x-tar"),
    ("text", "text/plain"),
    ("toml", "application/toml"),
    ("txt", "text/plain"),
    ("webp", "image/webp"),
    ("xml", "application/xml"),
    ("yaml", "application/yaml"),
    ("yml", "application/yaml"),
    ("zip", "application/zip"),
];

/// Determines a v1 media type from the final filename extension only.
///
/// The lookup is deliberately case-sensitive and does not use platform MIME databases, extended
/// attributes, file commands, or byte sniffing. Unknown extensions use the fixed fallback.
pub fn embedded_media_type_v1(path: &str) -> &'static str {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let Some((_, extension)) = file_name.rsplit_once('.') else {
        return DEFAULT_ARTIFACT_MEDIA_TYPE_V1;
    };
    EMBEDDED_MEDIA_TYPES_V1
        .iter()
        .find_map(|(candidate, media_type)| (*candidate == extension).then_some(*media_type))
        .unwrap_or(DEFAULT_ARTIFACT_MEDIA_TYPE_V1)
}

/// Cryptographically random UUID-v4 generator for all execution-domain IDs.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeExecutionIdSourceV1;

impl ExecutionIdSourceV1 for NativeExecutionIdSourceV1 {
    fn next_job_id(&self) -> JobId {
        JobId::new(next_uuid_v4_v1()).expect("UUID v4 must satisfy the domain UUID contract")
    }

    fn next_session_id(&self) -> SessionId {
        SessionId::new(next_uuid_v4_v1()).expect("UUID v4 must satisfy the domain UUID contract")
    }

    fn next_attempt_id(&self) -> AttemptId {
        AttemptId::new(next_uuid_v4_v1()).expect("UUID v4 must satisfy the domain UUID contract")
    }

    fn next_blocker_id(&self) -> BlockerId {
        BlockerId::new(next_uuid_v4_v1()).expect("UUID v4 must satisfy the domain UUID contract")
    }

    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
        ProcedureSnapshotId::new(next_uuid_v4_v1())
            .expect("UUID v4 must satisfy the domain UUID contract")
    }
}

fn next_uuid_v4_v1() -> String {
    Uuid::new_v4().hyphenated().to_string()
}

/// Wall-clock UTC millisecond source for execution timestamps.
///
/// `UnixMillis` cannot represent dates before the Unix epoch and must never silently wrap a
/// wider platform duration. Both conditions deliberately fail fast instead of corrupting durable
/// timestamps.
#[derive(Clone, Copy, Debug, Default)]
pub struct WallUtcExecutionClockV1;

impl ExecutionClockV1 for WallUtcExecutionClockV1 {
    fn now(&self) -> UnixMillis {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("wall clock is before the Unix epoch");
        let milliseconds = u64::try_from(elapsed.as_millis())
            .expect("wall clock milliseconds exceed UnixMillis capacity");
        UnixMillis::new(milliseconds)
    }
}

/// Store/Git two-pass workspace revalidation for admitted execution.
///
/// The selector path remains an observation hint. `WorkspaceResolverV1::resolve_existing` reads
/// the Store binding, verifies its UUID and Git fingerprints, and then re-resolves the bound Git
/// identity before a binding is returned.
pub struct NativeWorkspaceRevalidatorV1<I> {
    resolver: WorkspaceResolverV1<NativeGitResolverV1, I>,
}

impl<I> NativeWorkspaceRevalidatorV1<I>
where
    I: WorkspaceBindingInspectorV1,
{
    pub fn new(binding_inspector: I) -> Self {
        Self {
            resolver: WorkspaceResolverV1::new(NativeGitResolverV1::new(), binding_inspector),
        }
    }

    pub fn resolver(&self) -> &WorkspaceResolverV1<NativeGitResolverV1, I> {
        &self.resolver
    }

    fn resolve_bound_workspace(
        &self,
        binding: &WorkspaceBindingV1,
    ) -> Result<WorkspaceBindingV1, ExecutionBoundaryErrorV1> {
        let selector = worktree_selector_from_store_root_v1(binding)?;
        let resolved = self
            .resolver
            .resolve_existing(selector, Some(binding.identity().workspace_uuid()))
            .map_err(workspace_resolution_boundary_error_v1)?;
        let revalidated = WorkspaceBindingV1::new(
            resolved.store_identity().clone(),
            resolved.workspace_root().clone(),
        );
        if &revalidated != binding {
            return Err(domain_invalid_state_v1(
                "manager workspace binding does not match Store/Git evidence",
            ));
        }
        Ok(revalidated)
    }
}

impl<I> WorkspaceRevalidatorV1 for NativeWorkspaceRevalidatorV1<I>
where
    I: WorkspaceBindingInspectorV1,
{
    fn revalidate(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<WorkspaceBindingV1, ExecutionBoundaryErrorV1> {
        let expected_workspace_id = selector.expected_uuid().cloned();
        let selector = worktree_selector_from_wire_v1(selector)?;
        let resolved = self
            .resolver
            .resolve_existing(selector, expected_workspace_id.as_ref())
            .map_err(workspace_resolution_boundary_error_v1)?;
        let binding = WorkspaceBindingV1::new(
            resolved.store_identity().clone(),
            resolved.workspace_root().clone(),
        );
        Ok(binding)
    }

    fn revalidate_binding(
        &self,
        binding: &WorkspaceBindingV1,
    ) -> Result<WorkspaceBindingV1, ExecutionBoundaryErrorV1> {
        self.resolve_bound_workspace(binding)
    }
}

/// Descriptor-safe local artifact hashing over a freshly revalidated Store/Git workspace.
///
/// This adapter has no path-keyed lookup. Every operation first re-establishes the Store binding
/// from the caller's UUID and fingerprints, then hashes through `NativeGitResolverV1`.
pub struct NativeArtifactVerifierV1<I> {
    resolver: WorkspaceResolverV1<NativeGitResolverV1, I>,
    git_resolver: NativeGitResolverV1,
}

/// Descriptor-anchored production Procedure provider over a freshly revalidated Store/Git root.
pub struct NativeProcedureProviderV1<I> {
    resolver: WorkspaceResolverV1<NativeGitResolverV1, I>,
    git_resolver: NativeGitResolverV1,
}

impl<I> NativeProcedureProviderV1<I>
where
    I: WorkspaceBindingInspectorV1,
{
    pub fn new(binding_inspector: I) -> Self {
        Self {
            resolver: WorkspaceResolverV1::new(NativeGitResolverV1::new(), binding_inspector),
            git_resolver: NativeGitResolverV1::new(),
        }
    }

    fn resolve_bound_workspace(
        &self,
        workspace: &WorkspaceBindingV1,
    ) -> Result<ResolvedWorkspaceV1, ExecutionBoundaryErrorV1> {
        let selector = worktree_selector_from_store_root_v1(workspace)?;
        let resolved = self
            .resolver
            .resolve_existing(selector, Some(workspace.identity().workspace_uuid()))
            .map_err(workspace_resolution_boundary_error_v1)?;
        if resolved.store_identity() != workspace.identity() {
            return Err(domain_invalid_state_v1(
                "Procedure workspace identity does not match the Store binding",
            ));
        }
        Ok(resolved)
    }
}

impl<I> ProcedureProviderV1 for NativeProcedureProviderV1<I>
where
    I: WorkspaceBindingInspectorV1 + Send + Sync,
{
    fn load_workspace_procedure_snapshot_v2(
        &self,
        workspace: &WorkspaceBindingV1,
        procedure: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<
        podway_store::ProcedureSnapshotV2,
        crate::execution::ProcedureV2SourceAdmissionErrorV1,
    > {
        use crate::execution::ProcedureV2SourceAdmissionErrorV1;

        let resolved = self
            .resolve_bound_workspace(workspace)
            .map_err(ProcedureV2SourceAdmissionErrorV1::Rejected)?;
        let candidate = artifact_path_from_root_v1(resolved.workspace_root(), procedure)
            .map_err(ProcedureV2SourceAdmissionErrorV1::Rejected)?;
        let source = self
            .git_resolver
            .read_bounded_local_file(
                resolved.worktree(),
                &candidate,
                MAX_PROCEDURE_DOCUMENT_BYTES,
            )
            .map_err(git_resolution_boundary_error_v1)
            .map_err(ProcedureV2SourceAdmissionErrorV1::Rejected)?;
        if source.canonical_path() != &candidate {
            return Err(ProcedureV2SourceAdmissionErrorV1::Rejected(
                domain_invalid_state_v1(
                    "Procedure resolver returned a path outside the requested worktree location",
                ),
            ));
        }
        crate::execution::workspace_procedure_snapshot_from_bytes_v2(
            procedure,
            source.bytes(),
            snapshot_id,
            created_at,
        )
    }

    fn load_preset_snapshot_v2(
        &self,
        preset: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<
        Option<(podway_store::ProcedureSnapshotV2, podway_core::Sha256Digest)>,
        crate::execution::ProcedureV2SourceAdmissionErrorV1,
    > {
        EmbeddedPresetProcedureProviderV1.load_preset_snapshot_v2(preset, snapshot_id, created_at)
    }
}

impl<I> NativeArtifactVerifierV1<I>
where
    I: WorkspaceBindingInspectorV1,
{
    pub fn new(binding_inspector: I) -> Self {
        Self {
            resolver: WorkspaceResolverV1::new(NativeGitResolverV1::new(), binding_inspector),
            git_resolver: NativeGitResolverV1::new(),
        }
    }

    pub fn resolver(&self) -> &WorkspaceResolverV1<NativeGitResolverV1, I> {
        &self.resolver
    }

    fn resolve_bound_workspace(
        &self,
        workspace: &WorkspaceBindingV1,
    ) -> Result<ResolvedWorkspaceV1, ExecutionBoundaryErrorV1> {
        let selector = worktree_selector_from_store_root_v1(workspace)?;
        let resolved = self
            .resolver
            .resolve_existing(selector, Some(workspace.identity().workspace_uuid()))
            .map_err(workspace_resolution_boundary_error_v1)?;
        if resolved.store_identity() != workspace.identity() {
            return Err(domain_invalid_state_v1(
                "artifact workspace identity does not match the Store binding",
            ));
        }
        Ok(resolved)
    }

    fn hash_verified_local_artifact(
        &self,
        workspace: &WorkspaceBindingV1,
        path: &str,
        media_type: &str,
        revalidation: bool,
    ) -> Result<ArtifactValueV1, ExecutionBoundaryErrorV1> {
        let resolved = self.resolve_bound_workspace(workspace)?;
        let candidate = artifact_path_from_root_v1(resolved.workspace_root(), path)?;
        let hashed = self
            .git_resolver
            .hash_local_artifact(resolved.worktree(), &candidate)
            .map_err(|error| {
                if revalidation {
                    artifact_revalidation_boundary_error_v1(error)
                } else {
                    git_resolution_boundary_error_v1(error)
                }
            })?;
        if hashed.canonical_path() != &candidate {
            return Err(domain_invalid_state_v1(
                "artifact resolver returned a path outside the requested worktree location",
            ));
        }
        let artifact = ArtifactValueV1::local_path(
            path,
            hashed.digest().clone(),
            hashed.byte_length(),
            media_type,
        )
        .map_err(ExecutionBoundaryErrorV1::domain)?;
        Ok(artifact)
    }
}

impl<I> ArtifactVerifierV1 for NativeArtifactVerifierV1<I>
where
    I: WorkspaceBindingInspectorV1,
{
    fn hash_local_artifact(
        &self,
        workspace: &WorkspaceBindingV1,
        path: &str,
        requested_media_type: Option<&str>,
    ) -> Result<ArtifactValueV1, ExecutionBoundaryErrorV1> {
        let media_type = requested_media_type.unwrap_or_else(|| embedded_media_type_v1(path));
        self.hash_verified_local_artifact(workspace, path, media_type, false)
    }

    fn revalidate_local_artifact(
        &self,
        workspace: &WorkspaceBindingV1,
        item_id: &ItemId,
        artifact: &ArtifactValueV1,
    ) -> Result<LocalArtifactVerificationV2, ExecutionBoundaryErrorV1> {
        if artifact.location_kind() != ArtifactLocationKindV1::LocalPath {
            return Err(domain_invalid_state_v1(
                "local artifact revalidation requires a local-path artifact",
            ));
        }
        let observed = self.hash_verified_local_artifact(
            workspace,
            artifact.location(),
            artifact.media_type(),
            true,
        )?;
        if observed.location() != artifact.location()
            || observed.digest() != artifact.digest()
            || observed.size_bytes() != artifact.size_bytes()
        {
            return Err(ExecutionBoundaryErrorV1::domain(
                DomainError::ArtifactChanged,
            ));
        }
        Ok(LocalArtifactVerificationV2 {
            item_id: item_id.clone(),
            location: observed.location().to_owned(),
            digest: observed.digest().clone(),
            size_bytes: observed.size_bytes(),
        })
    }
}

fn worktree_selector_from_wire_v1(
    selector: &WorktreeSelectorWireV1,
) -> Result<WorktreeSelectorV1, ExecutionBoundaryErrorV1> {
    if u16::from(selector.version()) != WORKTREE_SELECTOR_VERSION_V1 {
        return Err(domain_invalid_state_v1(
            "workspace selector version is not supported by the Git resolver",
        ));
    }
    let encoded = Base64UrlPathBytesV1::new(selector.path_bytes_base64url())
        .map_err(|_| domain_invalid_state_v1("workspace selector path bytes are invalid"))?;
    let display = DiagnosticPathDisplayV1::new(selector.display())
        .map_err(|_| domain_invalid_state_v1("workspace selector display is invalid"))?;
    let path = LosslessPathV1::from_base64url(encoded, display);
    WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, path)
        .map_err(|_| domain_invalid_state_v1("workspace selector is invalid"))
}

fn worktree_selector_from_store_root_v1(
    workspace: &WorkspaceBindingV1,
) -> Result<WorktreeSelectorV1, ExecutionBoundaryErrorV1> {
    let root = LosslessPathV1::from_raw_bytes(
        workspace.last_validated_root().unix_bytes(),
        DiagnosticPathDisplayV1::new(BOUND_ROOT_DISPLAY_V1)
            .expect("fixed Store-root diagnostic is valid"),
    )
    .map_err(|_| domain_invalid_state_v1("Store workspace root is invalid"))?;
    WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, root)
        .map_err(|_| domain_invalid_state_v1("Store workspace root selector is invalid"))
}

fn artifact_path_from_root_v1(
    root: &podway_store::ValidatedWorkspaceRootV1,
    path: &str,
) -> Result<LosslessPathV1, ExecutionBoundaryErrorV1> {
    validate_relative_artifact_path_v1(path)?;
    let root = root.unix_bytes();
    let separator = if root == b"/" { 0 } else { 1 };
    let capacity = root
        .len()
        .checked_add(separator)
        .and_then(|value| value.checked_add(path.len()))
        .ok_or_else(|| domain_invalid_state_v1("artifact path length overflows"))?;
    let mut absolute = Vec::with_capacity(capacity);
    absolute.extend_from_slice(root);
    if separator == 1 {
        absolute.push(b'/');
    }
    absolute.extend_from_slice(path.as_bytes());
    LosslessPathV1::from_raw_bytes(
        absolute,
        DiagnosticPathDisplayV1::new(ARTIFACT_PATH_DISPLAY_V1)
            .expect("fixed artifact diagnostic is valid"),
    )
    .map_err(|_| domain_invalid_state_v1("artifact path is not a bounded local path"))
}

fn validate_relative_artifact_path_v1(path: &str) -> Result<(), ExecutionBoundaryErrorV1> {
    if path.chars().count() > 4_000 {
        return Err(domain_invalid_state_v1(
            "local artifact path exceeds the v1 scalar limit",
        ));
    }
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains('\0')
        || path.ends_with('/')
        || path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':'
    {
        return Err(domain_invalid_state_v1(
            "local artifact path must be normalized and worktree-relative",
        ));
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(domain_invalid_state_v1(
            "local artifact path must not escape the worktree",
        ));
    }
    Ok(())
}

fn workspace_resolution_boundary_error_v1(
    error: WorkspaceResolutionErrorV1,
) -> ExecutionBoundaryErrorV1 {
    match error {
        WorkspaceResolutionErrorV1::BindingInspection { .. }
        | WorkspaceResolutionErrorV1::RuntimeDatabasePathChangedDuringResolution => {
            ExecutionBoundaryErrorV1::transient("revalidate workspace binding")
        }
        WorkspaceResolutionErrorV1::Git { source, .. } => git_resolution_boundary_error_v1(source),
        WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch { expected, actual } => {
            ExecutionBoundaryErrorV1::workspace_identity_mismatch(expected, actual)
        }
        WorkspaceResolutionErrorV1::Selector { .. }
        | WorkspaceResolutionErrorV1::ExistingBindingMissing
        | WorkspaceResolutionErrorV1::BootstrapBindingAlreadyPresent
        | WorkspaceResolutionErrorV1::GitStoreFingerprintMismatch { .. }
        | WorkspaceResolutionErrorV1::PreliminaryIdentityWasNotCandidate { .. }
        | WorkspaceResolutionErrorV1::RevalidatedIdentityStateMismatch { .. }
        | WorkspaceResolutionErrorV1::RevalidatedStoreIdentityMismatch { .. }
        | WorkspaceResolutionErrorV1::StoredRootPathInvalid { .. }
        | WorkspaceResolutionErrorV1::WorkspaceRootPathInvalid { .. }
        | WorkspaceResolutionErrorV1::RuntimeDirectoryPathInvalid { .. }
        | WorkspaceResolutionErrorV1::RuntimeDirectoryPathUnsupportedPlatform
        | WorkspaceResolutionErrorV1::WorkspaceRootConversion { .. } => {
            domain_invalid_state_v1("workspace is not a matching Store-bound Git worktree")
        }
    }
}

fn git_resolution_boundary_error_v1(error: GitResolverErrorV1) -> ExecutionBoundaryErrorV1 {
    match error {
        GitResolverErrorV1::PermissionDenied { .. }
        | GitResolverErrorV1::Io { .. }
        | GitResolverErrorV1::WorkspaceLayoutCleanup { .. }
        | GitResolverErrorV1::Invariant {
            problem:
                GitInvariantViolationV1::MetadataChangedDuringResolution
                | GitInvariantViolationV1::MetadataChangedDuringArtifactHash
                | GitInvariantViolationV1::MetadataChangedDuringWorkspaceLayout,
        } => ExecutionBoundaryErrorV1::transient("revalidate local Git worktree"),
        GitResolverErrorV1::Selector { .. }
        | GitResolverErrorV1::NonGitRepository
        | GitResolverErrorV1::BareRepository
        | GitResolverErrorV1::PathEscape { .. }
        | GitResolverErrorV1::SymlinkEscape { .. }
        | GitResolverErrorV1::CopiedWorkspaceUuid { .. }
        | GitResolverErrorV1::IdentityMismatch { .. }
        | GitResolverErrorV1::MoveConflict { .. }
        | GitResolverErrorV1::WorktreeDeleted
        | GitResolverErrorV1::Representation { .. }
        | GitResolverErrorV1::Invariant { .. } => domain_invalid_state_v1(
            "workspace or local artifact does not satisfy Git safety checks",
        ),
    }
}

fn artifact_revalidation_boundary_error_v1(error: GitResolverErrorV1) -> ExecutionBoundaryErrorV1 {
    match error {
        GitResolverErrorV1::Io {
            operation: GitReadOperationV1::OpenLocalArtifact,
        }
        | GitResolverErrorV1::SymlinkEscape { .. }
        | GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::NonRegularArtifact,
        }
        | GitResolverErrorV1::Invariant {
            problem: GitInvariantViolationV1::MetadataChangedDuringArtifactHash,
        } => ExecutionBoundaryErrorV1::domain(DomainError::ArtifactChanged),
        error => git_resolution_boundary_error_v1(error),
    }
}

fn domain_invalid_state_v1(reason: &'static str) -> ExecutionBoundaryErrorV1 {
    ExecutionBoundaryErrorV1::domain(DomainError::InvalidState { reason })
}
