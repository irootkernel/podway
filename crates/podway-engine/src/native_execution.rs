//! Native production adapters for host-independent execution.
//!
//! These adapters implement native boundaries without daemon hosting. Workspace identity is always
//! re-established through the Store/Git two-pass resolver; local artifact bytes remain outside
//! durable state and are read only through the descriptor-anchored Git resolver.

use std::time::{SystemTime, UNIX_EPOCH};

use podway_config::MAX_PROCEDURE_DOCUMENT_BYTES;
use podway_core::{
    ArtifactLocationKindV1, ArtifactValueV1, DomainError, ItemId, ProcedureSnapshotId, UnixMillis,
};
use podway_git::{
    DiagnosticPathDisplayV1, GitInvariantViolationV1, GitReadOperationV1,
    GitRepresentationProblemV1, GitResolverContractV1, GitResolverErrorV1, LocalArtifactStampV1,
    LosslessPathV1, NativeGitResolverV1, ValidatedWorktreeV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_store::WorkspaceBindingV1;

use crate::{
    execution::{
        ArtifactVerifierV1, EmbeddedPresetProcedureProviderV1, ExecutionBoundaryErrorV1,
        ExecutionClockV1, LocalArtifactVerificationV2, ProcedureProviderV1,
    },
    workspace::ResolvedWorkspaceV1,
};

/// Version of the embedded, extension-only media-type mapping.
pub const EMBEDDED_MEDIA_TYPE_MAPPING_VERSION_V1: u8 = 1;

const ARTIFACT_PATH_DISPLAY_V1: &str = "worktree-local artifact";
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

/// Transaction-safe Procedure reads and artifact verification from a qualified direct workspace.
/// The owning snapshot or transaction supplies the binding; this adapter never opens SQLite.
#[derive(Clone)]
pub struct NativeDirectWorkspaceV1 {
    resolved: ResolvedWorkspaceV1,
    hash_observer: Option<NativeArtifactHashObserverV1>,
}

pub(crate) type NativeArtifactHashObserverV1 =
    std::sync::Arc<dyn Fn(&LosslessPathV1, u64) + Send + Sync>;

impl std::fmt::Debug for NativeDirectWorkspaceV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeDirectWorkspaceV1")
            .field("resolved", &self.resolved)
            .finish_non_exhaustive()
    }
}

/// A request-local digest and opaque descriptor stamp, never persisted or serialized.
#[derive(Clone, Debug)]
pub struct PreparedLocalArtifactV1 {
    artifact: ArtifactValueV1,
    stamp: LocalArtifactStampV1,
}

impl PreparedLocalArtifactV1 {
    pub fn artifact(&self) -> &ArtifactValueV1 {
        &self.artifact
    }
}

#[derive(Debug)]
pub(crate) struct PreparedRequiredArtifactV2 {
    item_id: ItemId,
    item_revision: podway_core::Revision,
    attempt_id: podway_core::AttemptId,
    artifact: PreparedLocalArtifactV1,
}

impl NativeDirectWorkspaceV1 {
    pub fn new(resolved: ResolvedWorkspaceV1) -> Self {
        Self {
            resolved,
            hash_observer: None,
        }
    }

    pub(crate) fn with_hash_observer(
        mut self,
        observer: Option<NativeArtifactHashObserverV1>,
    ) -> Self {
        self.hash_observer = observer;
        self
    }

    /// Streams the digest before writer admission and retains only metadata and a stamp.
    pub fn prepare_local_artifact_v1(
        &self,
        binding: &WorkspaceBindingV1,
        path: &str,
        media_type: Option<&str>,
    ) -> Result<PreparedLocalArtifactV1, ExecutionBoundaryErrorV1> {
        self.prepare_artifact(
            binding,
            path,
            media_type.unwrap_or_else(|| embedded_media_type_v1(path)),
            false,
        )
    }

    /// Reobserves metadata through the safe descriptor chain, without artifact-byte reads.
    pub fn verify_prepared_local_artifact_v1(
        &self,
        binding: &WorkspaceBindingV1,
        prepared: &PreparedLocalArtifactV1,
    ) -> Result<(), ExecutionBoundaryErrorV1> {
        let worktree = self.revalidate_worktree(binding)?;
        let candidate = artifact_path_from_root_v1(
            self.resolved.workspace_root(),
            prepared.artifact.location(),
        )?;
        let current = NativeGitResolverV1::new()
            .observe_local_artifact_stamp_v1(&worktree, &candidate)
            .map_err(artifact_revalidation_boundary_error_v1)?;
        if current != prepared.stamp {
            return Err(ExecutionBoundaryErrorV1::domain(
                DomainError::ArtifactChanged,
            ));
        }
        Ok(())
    }

    pub(crate) fn prepare_required_artifacts_v2(
        &self,
        binding: &WorkspaceBindingV1,
        state: &podway_store::GraphSessionStateV2,
    ) -> Result<Vec<PreparedRequiredArtifactV2>, crate::direct_execution::DirectExecutionErrorV1>
    {
        use crate::direct_execution::DirectExecutionErrorV1 as E;
        let required =
            crate::execution::required_local_artifacts_v2(state).map_err(E::Preparation)?;
        required
            .into_iter()
            .map(|(item_id, recorded)| {
                let artifact = self
                    .prepare_artifact(binding, recorded.location(), recorded.media_type(), true)
                    .map_err(E::Boundary)?;
                if artifact.artifact != recorded {
                    return Err(E::Boundary(ExecutionBoundaryErrorV1::domain(
                        DomainError::ArtifactChanged,
                    )));
                }
                Ok(PreparedRequiredArtifactV2 {
                    item_revision: active_item_revision(state, &item_id).map_err(E::Boundary)?,
                    attempt_id: state
                        .trace()
                        .active_attempt()
                        .expect("required artifact has an active attempt")
                        .attempt_id()
                        .clone(),
                    item_id,
                    artifact,
                })
            })
            .collect()
    }

    pub(crate) fn verify_required_artifacts_v2(
        &self,
        binding: &WorkspaceBindingV1,
        state: &podway_store::GraphSessionStateV2,
        prepared: &[PreparedRequiredArtifactV2],
    ) -> Result<(), crate::direct_execution::DirectExecutionErrorV1> {
        use crate::direct_execution::DirectExecutionErrorV1 as E;
        let changed = || {
            E::Boundary(ExecutionBoundaryErrorV1::domain(
                DomainError::ArtifactChanged,
            ))
        };
        let current =
            crate::execution::required_local_artifacts_v2(state).map_err(E::Preparation)?;
        if current.len() != prepared.len() {
            return Err(changed());
        }
        for ((item, artifact), prior) in current.iter().zip(prepared) {
            if item != &prior.item_id
                || artifact != prior.artifact.artifact()
                || active_item_revision(state, item).map_err(E::Boundary)? != prior.item_revision
                || state
                    .trace()
                    .active_attempt()
                    .is_none_or(|active| active.attempt_id() != &prior.attempt_id)
            {
                return Err(changed());
            }
            self.verify_prepared_local_artifact_v1(binding, &prior.artifact)
                .map_err(E::Boundary)?;
        }
        Ok(())
    }

    fn revalidate_worktree(
        &self,
        binding: &WorkspaceBindingV1,
    ) -> Result<ValidatedWorktreeV1, ExecutionBoundaryErrorV1> {
        if binding.schema_version() != podway_store::direct_store::DIRECT_SCHEMA_GENERATION_V1
            || binding.identity() != self.resolved.store_identity()
        {
            return Err(domain_invalid_state_v1(
                "direct native binding does not match the qualified Store",
            ));
        }
        let prior = self.resolved.worktree();
        let selector = WorktreeSelectorV1::new(
            WORKTREE_SELECTOR_VERSION_V1,
            Some(prior.identity().clone()),
            prior.roots().worktree_root().clone(),
        )
        .map_err(|_| domain_invalid_state_v1("direct native selector is invalid"))?;
        let current = NativeGitResolverV1::new()
            .resolve(selector)
            .map_err(git_resolution_boundary_error_v1)?;
        if current.identity() != prior.identity() || !prior.same_transient_object_evidence(&current)
        {
            return Err(domain_invalid_state_v1(
                "direct native worktree changed during execution",
            ));
        }
        Ok(current)
    }

    fn hash(
        &self,
        binding: &WorkspaceBindingV1,
        path: &str,
        media_type: &str,
        revalidation: bool,
    ) -> Result<ArtifactValueV1, ExecutionBoundaryErrorV1> {
        self.prepare_artifact(binding, path, media_type, revalidation)
            .map(|prepared| prepared.artifact)
    }

    fn prepare_artifact(
        &self,
        binding: &WorkspaceBindingV1,
        path: &str,
        media_type: &str,
        revalidation: bool,
    ) -> Result<PreparedLocalArtifactV1, ExecutionBoundaryErrorV1> {
        let worktree = self.revalidate_worktree(binding)?;
        let candidate =
            artifact_path_from_root_v1(self.resolved.workspace_root(), path).map_err(|_| {
                ExecutionBoundaryErrorV1::LocalArtifact(
                    crate::execution::LocalArtifactFailureV1::PathOutsideWorktree,
                )
            })?;
        let resolver = NativeGitResolverV1::new();
        let hashed = match &self.hash_observer {
            Some(observer) => {
                resolver.hash_local_artifact_with_observer_v1(&worktree, &candidate, &|bytes| {
                    observer(&candidate, bytes)
                })
            }
            None => resolver.hash_local_artifact(&worktree, &candidate),
        }
        .map_err(|error| {
            if revalidation {
                artifact_revalidation_boundary_error_v1(error)
            } else {
                direct_artifact_boundary_error_v1(error, &candidate)
            }
        })?;
        if hashed.canonical_path() != &candidate {
            return Err(domain_invalid_state_v1(
                "artifact resolver returned another canonical path",
            ));
        }
        let artifact = ArtifactValueV1::local_path(
            path,
            hashed.digest().clone(),
            hashed.byte_length(),
            media_type,
        )
        .map_err(ExecutionBoundaryErrorV1::domain)?;
        Ok(PreparedLocalArtifactV1 {
            artifact,
            stamp: hashed.stamp(),
        })
    }
}

fn active_item_revision(
    state: &podway_store::GraphSessionStateV2,
    item: &ItemId,
) -> Result<podway_core::Revision, ExecutionBoundaryErrorV1> {
    state
        .trace()
        .active_attempt()
        .and_then(|active| {
            state
                .workflow_memory()
                .attempts()
                .iter()
                .find(|memory| memory.attempt_id() == active.attempt_id())
        })
        .and_then(|memory| {
            memory
                .item_slots()
                .iter()
                .find(|slot| slot.item_id() == item)
        })
        .map(|slot| slot.revision())
        .ok_or_else(|| domain_invalid_state_v1("required artifact has no active item slot"))
}

fn direct_artifact_boundary_error_v1(
    error: GitResolverErrorV1,
    candidate: &LosslessPathV1,
) -> ExecutionBoundaryErrorV1 {
    use crate::execution::LocalArtifactFailureV1 as E;
    let failure = match error {
        GitResolverErrorV1::Invariant {
            problem: GitInvariantViolationV1::MetadataChangedDuringArtifactHash,
        } => return ExecutionBoundaryErrorV1::domain(DomainError::ArtifactChanged),
        GitResolverErrorV1::PathEscape { .. } | GitResolverErrorV1::SymlinkEscape { .. } => {
            E::PathOutsideWorktree
        }
        GitResolverErrorV1::Io {
            operation: GitReadOperationV1::OpenLocalArtifact,
        }
        | GitResolverErrorV1::Representation {
            problem: GitRepresentationProblemV1::NonRegularArtifact,
        } => {
            // This observation only refines a failed descriptor-safe open. It is never
            // used to authorize a hash or to bypass Git's no-follow validation.
            use std::os::unix::ffi::OsStringExt;
            let path = candidate
                .decode_path_bytes()
                .ok()
                .map(|bytes| std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes)));
            if path.is_some_and(|path| {
                std::fs::symlink_metadata(path)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            }) {
                E::NotFound
            } else {
                E::Unreadable
            }
        }
        GitResolverErrorV1::Representation { .. } | GitResolverErrorV1::Selector { .. } => {
            E::PathUnsafe
        }
        _ => E::Unreadable,
    };
    ExecutionBoundaryErrorV1::LocalArtifact(failure)
}

impl ArtifactVerifierV1 for NativeDirectWorkspaceV1 {
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
        let current = self.hash(workspace, artifact.location(), artifact.media_type(), true)?;
        if current.location() != artifact.location()
            || current.digest() != artifact.digest()
            || current.size_bytes() != artifact.size_bytes()
        {
            return Err(ExecutionBoundaryErrorV1::domain(
                DomainError::ArtifactChanged,
            ));
        }
        Ok(LocalArtifactVerificationV2 {
            item_id: item_id.clone(),
            location: current.location().into(),
            digest: current.digest().clone(),
            size_bytes: current.size_bytes(),
        })
    }
}

impl ProcedureProviderV1 for NativeDirectWorkspaceV1 {
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
        self.prepare_workspace_procedure_source_v2(workspace, procedure)?
            .into_snapshot(snapshot_id, created_at)
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

impl NativeDirectWorkspaceV1 {
    fn procedure_bytes(
        &self,
        workspace: &WorkspaceBindingV1,
        procedure: &str,
    ) -> Result<Vec<u8>, crate::execution::ProcedureV2SourceAdmissionErrorV1> {
        use crate::execution::ProcedureV2SourceAdmissionErrorV1;
        let worktree = self
            .revalidate_worktree(workspace)
            .map_err(ProcedureV2SourceAdmissionErrorV1::Rejected)?;
        let candidate = artifact_path_from_root_v1(self.resolved.workspace_root(), procedure)
            .map_err(ProcedureV2SourceAdmissionErrorV1::Rejected)?;
        let source = NativeGitResolverV1::new()
            .read_bounded_local_file(&worktree, &candidate, MAX_PROCEDURE_DOCUMENT_BYTES)
            .map_err(git_resolution_boundary_error_v1)
            .map_err(ProcedureV2SourceAdmissionErrorV1::Rejected)?;
        if source.canonical_path() != &candidate {
            return Err(ProcedureV2SourceAdmissionErrorV1::Rejected(
                domain_invalid_state_v1("Procedure resolver returned another canonical path"),
            ));
        }
        Ok(source.bytes().to_vec())
    }

    pub fn prepare_workspace_procedure_source_v2(
        &self,
        workspace: &WorkspaceBindingV1,
        procedure: &str,
    ) -> Result<
        crate::execution::PreparedProcedureSourceV2,
        crate::execution::ProcedureV2SourceAdmissionErrorV1,
    > {
        crate::execution::prepare_workspace_procedure_source_v2(
            procedure,
            &self.procedure_bytes(workspace, procedure)?,
        )
    }

    pub fn revalidate_procedure_source_v2(
        &self,
        workspace: &WorkspaceBindingV1,
        procedure: &str,
        prepared: &crate::execution::PreparedProcedureSourceV2,
    ) -> Result<podway_core::Sha256Digest, crate::execution::ProcedureV2SourceAdmissionErrorV1>
    {
        use sha2::{Digest as _, Sha256};
        let bytes = self.procedure_bytes(workspace, procedure)?;
        let raw_digest =
            podway_core::Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(&bytes)))
                .expect("SHA-256 produces a canonical digest");
        if prepared.source_bytes_digest() == Some(&raw_digest) {
            return Ok(prepared.digest().clone());
        }
        crate::execution::prepare_workspace_procedure_source_v2(procedure, &bytes)
            .map(|source| source.digest().clone())
    }
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
