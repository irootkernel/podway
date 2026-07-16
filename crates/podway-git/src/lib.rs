#![forbid(unsafe_code)]

//! Read-only Git worktree resolution contract for Podway.
//!
//! The contract carries encoded native path bytes alongside bounded diagnostic
//! display strings. It deliberately exposes no Git mutation operation.

use podway_core::{Sha256Digest, WorkspaceId};

mod layout;
mod native;
mod resolver;

pub use layout::{
    WorkspaceLayoutElementStatusV1, WorkspaceLayoutInitializerV1, WorkspaceLayoutReportV1,
};
pub use resolver::NativeGitResolverV1;

pub const WORKTREE_SELECTOR_VERSION_V1: u16 = 1;
pub const MAX_SELECTOR_COMPONENT_BYTES_V1: usize = 16 * 1024;

/// Resolves one selector into a verified, non-bare Git worktree.
pub trait GitResolverContractV1: Send + Sync {
    fn resolve(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<ValidatedWorktreeV1, GitResolveErrorV1>;
}

/// An unpadded base64url encoding of canonical, absolute local native path bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Base64UrlPathBytesV1(String);

impl Base64UrlPathBytesV1 {
    pub fn from_raw_bytes(value: impl AsRef<[u8]>) -> Result<Self, SelectorValidationErrorV1> {
        let value = value.as_ref();
        validate_path_bytes(value)?;
        let encoded = encode_base64url(value);
        validate_component_size(&encoded, "path_bytes_base64url")?;
        Ok(Self(encoded))
    }

    pub fn new(value: impl Into<String>) -> Result<Self, SelectorValidationErrorV1> {
        let value = value.into();
        validate_component_size(&value, "path_bytes_base64url")?;
        let decoded = decode_base64url(&value)?;
        validate_path_bytes(&decoded)?;
        if encode_base64url(&decoded) != value {
            return Err(SelectorValidationErrorV1::NonCanonicalBase64Url);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn decode(&self) -> Result<Vec<u8>, SelectorValidationErrorV1> {
        decode_base64url(&self.0)
    }
}

/// Bounded UTF-8 path text intended only for local diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticPathDisplayV1(String);

impl DiagnosticPathDisplayV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, SelectorValidationErrorV1> {
        let value = value.into();
        if value.is_empty() {
            return Err(SelectorValidationErrorV1::EmptyDisplay);
        }
        if value.as_bytes().contains(&0) {
            return Err(SelectorValidationErrorV1::NulInDisplay);
        }
        validate_component_size(&value, "display")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One canonical, absolute local native path represented without UTF-8 loss and with a diagnostic display.
/// Equality is based only on native path bytes; diagnostic display text is never identity.
#[derive(Clone, Debug)]
pub struct LosslessPathV1 {
    path_bytes_base64url: Base64UrlPathBytesV1,
    display: DiagnosticPathDisplayV1,
}

impl LosslessPathV1 {
    pub fn from_raw_bytes(
        path_bytes: impl AsRef<[u8]>,
        display: DiagnosticPathDisplayV1,
    ) -> Result<Self, SelectorValidationErrorV1> {
        Ok(Self {
            path_bytes_base64url: Base64UrlPathBytesV1::from_raw_bytes(path_bytes)?,
            display,
        })
    }

    pub fn from_base64url(
        path_bytes_base64url: Base64UrlPathBytesV1,
        display: DiagnosticPathDisplayV1,
    ) -> Self {
        Self {
            path_bytes_base64url,
            display,
        }
    }

    pub fn path_bytes_base64url(&self) -> &Base64UrlPathBytesV1 {
        &self.path_bytes_base64url
    }

    pub fn display(&self) -> &DiagnosticPathDisplayV1 {
        &self.display
    }

    pub fn decode_path_bytes(&self) -> Result<Vec<u8>, SelectorValidationErrorV1> {
        self.path_bytes_base64url.decode()
    }

    fn same_canonical_path(&self, other: &Self) -> bool {
        self.path_bytes_base64url.as_str() == other.path_bytes_base64url.as_str()
    }
}
impl PartialEq for LosslessPathV1 {
    fn eq(&self, other: &Self) -> bool {
        self.path_bytes_base64url == other.path_bytes_base64url
    }
}

impl Eq for LosslessPathV1 {}

/// Git and Podway identity whose binding state is explicit at the Git/store boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableWorktreeIdentityV1 {
    workspace_id: WorkspaceId,
    common_directory_fingerprint: Sha256Digest,
    worktree_administration_fingerprint: Sha256Digest,
    root_directory_fingerprint: Option<Sha256Digest>,
    last_validated_root: LosslessPathV1,
    state: WorkspaceIdentityStateV1,
}

/// Whether a Git identity is a store-bound durable value or only a fresh candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceIdentityStateV1 {
    /// A fresh UUID candidate that must be atomically bound by the store.
    Candidate,
    /// A root-evidenced identity supplied from a durable store binding.
    Bound,
    /// A source-compatible identity that lacks the evidence required for validation.
    UnvalidatedLegacy,
}

impl DurableWorktreeIdentityV1 {
    /// Constructs a source-compatible identity that must be revalidated before use.
    pub fn new(
        workspace_id: WorkspaceId,
        common_directory_fingerprint: Sha256Digest,
        worktree_administration_fingerprint: Sha256Digest,
        last_validated_root: LosslessPathV1,
    ) -> Self {
        Self {
            workspace_id,
            common_directory_fingerprint,
            worktree_administration_fingerprint,
            root_directory_fingerprint: None,
            last_validated_root,
            state: WorkspaceIdentityStateV1::UnvalidatedLegacy,
        }
    }

    /// Constructs a store-bound identity with descriptor-derived root evidence.
    pub fn new_with_root_directory_fingerprint(
        workspace_id: WorkspaceId,
        common_directory_fingerprint: Sha256Digest,
        worktree_administration_fingerprint: Sha256Digest,
        root_directory_fingerprint: Sha256Digest,
        last_validated_root: LosslessPathV1,
    ) -> Self {
        Self {
            workspace_id,
            common_directory_fingerprint,
            worktree_administration_fingerprint,
            root_directory_fingerprint: Some(root_directory_fingerprint),
            last_validated_root,
            state: WorkspaceIdentityStateV1::Bound,
        }
    }

    pub(crate) fn new_candidate_with_root_directory_fingerprint(
        workspace_id: WorkspaceId,
        common_directory_fingerprint: Sha256Digest,
        worktree_administration_fingerprint: Sha256Digest,
        root_directory_fingerprint: Sha256Digest,
        last_validated_root: LosslessPathV1,
    ) -> Self {
        Self {
            workspace_id,
            common_directory_fingerprint,
            worktree_administration_fingerprint,
            root_directory_fingerprint: Some(root_directory_fingerprint),
            last_validated_root,
            state: WorkspaceIdentityStateV1::Candidate,
        }
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.workspace_id
    }

    pub fn common_directory_fingerprint(&self) -> &Sha256Digest {
        &self.common_directory_fingerprint
    }

    pub fn worktree_administration_fingerprint(&self) -> &Sha256Digest {
        &self.worktree_administration_fingerprint
    }

    /// Returns descriptor-derived root evidence when the identity was produced natively.
    pub fn root_directory_fingerprint(&self) -> Option<&Sha256Digest> {
        self.root_directory_fingerprint.as_ref()
    }

    /// Returns whether this identity is a fresh candidate or a store-bound value.
    pub fn state(&self) -> &WorkspaceIdentityStateV1 {
        &self.state
    }

    /// Returns true only after the workspace UUID has been atomically bound by the store.
    pub fn is_store_bound(&self) -> bool {
        self.state == WorkspaceIdentityStateV1::Bound
    }

    pub fn last_validated_root(&self) -> &LosslessPathV1 {
        &self.last_validated_root
    }
}

/// Versioned resolver input with an optional previously durable identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeSelectorV1 {
    version: u16,
    durable_identity: Option<DurableWorktreeIdentityV1>,
    path: LosslessPathV1,
}

impl WorktreeSelectorV1 {
    pub fn new(
        version: u16,
        durable_identity: Option<DurableWorktreeIdentityV1>,
        path: LosslessPathV1,
    ) -> Result<Self, SelectorValidationErrorV1> {
        if version != WORKTREE_SELECTOR_VERSION_V1 {
            return Err(SelectorValidationErrorV1::UnsupportedVersion { found: version });
        }
        Ok(Self {
            version,
            durable_identity,
            path,
        })
    }

    pub fn version(&self) -> u16 {
        self.version
    }

    pub fn durable_identity(&self) -> Option<&DurableWorktreeIdentityV1> {
        self.durable_identity.as_ref()
    }

    pub fn path(&self) -> &LosslessPathV1 {
        &self.path
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorktreeKindV1 {
    Main,
    Linked,
}

/// Canonical native roots discovered from validated Git administrative metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeRootsV1 {
    worktree_root: LosslessPathV1,
    common_directory_root: LosslessPathV1,
    worktree_administration_root: LosslessPathV1,
}

impl WorktreeRootsV1 {
    pub fn new(
        worktree_root: LosslessPathV1,
        common_directory_root: LosslessPathV1,
        worktree_administration_root: LosslessPathV1,
    ) -> Self {
        Self {
            worktree_root,
            common_directory_root,
            worktree_administration_root,
        }
    }

    pub fn worktree_root(&self) -> &LosslessPathV1 {
        &self.worktree_root
    }

    pub fn common_directory_root(&self) -> &LosslessPathV1 {
        &self.common_directory_root
    }

    pub fn worktree_administration_root(&self) -> &LosslessPathV1 {
        &self.worktree_administration_root
    }
}

/// Filesystem boundaries established during worktree validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainmentMetadataV1 {
    workspace_root: LosslessPathV1,
    podway_directory: LosslessPathV1,
    runtime_directory: LosslessPathV1,
}

impl ContainmentMetadataV1 {
    pub fn new(
        workspace_root: LosslessPathV1,
        podway_directory: LosslessPathV1,
        runtime_directory: LosslessPathV1,
    ) -> Self {
        Self {
            workspace_root,
            podway_directory,
            runtime_directory,
        }
    }

    pub fn workspace_root(&self) -> &LosslessPathV1 {
        &self.workspace_root
    }

    pub fn podway_directory(&self) -> &LosslessPathV1 {
        &self.podway_directory
    }

    pub fn runtime_directory(&self) -> &LosslessPathV1 {
        &self.runtime_directory
    }
}

/// Move evidence used to perform a safe registry-root update outside this crate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeMoveMetadataV1 {
    previous_root: Option<LosslessPathV1>,
    current_root: LosslessPathV1,
}

impl WorktreeMoveMetadataV1 {
    pub fn stationary(current_root: LosslessPathV1) -> Self {
        Self {
            previous_root: None,
            current_root,
        }
    }

    pub fn relocated(
        previous_root: LosslessPathV1,
        current_root: LosslessPathV1,
    ) -> Result<Self, ValidatedWorktreeValidationErrorV1> {
        if previous_root.same_canonical_path(&current_root) {
            return Err(ValidatedWorktreeValidationErrorV1::RelocationWithoutRootChange);
        }
        Ok(Self {
            previous_root: Some(previous_root),
            current_root,
        })
    }

    pub fn previous_root(&self) -> Option<&LosslessPathV1> {
        self.previous_root.as_ref()
    }

    pub fn current_root(&self) -> &LosslessPathV1 {
        &self.current_root
    }

    pub fn relocated_from_prior_root(&self) -> bool {
        self.previous_root.is_some()
    }
}

/// Registry repair action that is safe only after identity and UUID validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryRepairActionV1 {
    NotRequired,
    UpdateValidatedRoot { previous_root: LosslessPathV1 },
}

/// The store/registry verification still required for the workspace UUID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceUuidVerificationV1 {
    /// A generated candidate that becomes durable only after store initialization.
    PendingStoreInitialization,
    /// A durable selector matched Git identity but still needs registry checking.
    RegistryCheckRequired,
    /// Retained as legacy input only; constructors quarantine it as registry checking required.
    Unique,
}

/// Metadata the daemon needs to repair registration without guessing identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeRepairMetadataV1 {
    registry_action: RegistryRepairActionV1,
    workspace_uuid_verification: WorkspaceUuidVerificationV1,
}

impl WorktreeRepairMetadataV1 {
    /// Constructs metadata without root repair for a fresh, unpersisted workspace.
    pub fn not_required() -> Self {
        Self::not_required_with_uuid_verification(
            WorkspaceUuidVerificationV1::PendingStoreInitialization,
        )
    }

    /// Constructs no-repair metadata with an explicit store verification state.
    pub fn not_required_with_uuid_verification(
        workspace_uuid_verification: WorkspaceUuidVerificationV1,
    ) -> Self {
        Self {
            registry_action: RegistryRepairActionV1::NotRequired,
            workspace_uuid_verification: quarantine_legacy_uuid_verification(
                workspace_uuid_verification,
            ),
        }
    }

    /// Constructs root-repair metadata for a fresh, unpersisted workspace.
    pub fn update_validated_root(previous_root: LosslessPathV1) -> Self {
        Self::update_validated_root_with_uuid_verification(
            previous_root,
            WorkspaceUuidVerificationV1::PendingStoreInitialization,
        )
    }

    /// Constructs root-repair metadata with an explicit store verification state.
    pub fn update_validated_root_with_uuid_verification(
        previous_root: LosslessPathV1,
        workspace_uuid_verification: WorkspaceUuidVerificationV1,
    ) -> Self {
        Self {
            registry_action: RegistryRepairActionV1::UpdateValidatedRoot { previous_root },
            workspace_uuid_verification: quarantine_legacy_uuid_verification(
                workspace_uuid_verification,
            ),
        }
    }

    pub fn registry_action(&self) -> &RegistryRepairActionV1 {
        &self.registry_action
    }

    pub fn workspace_uuid_verification(&self) -> &WorkspaceUuidVerificationV1 {
        &self.workspace_uuid_verification
    }
}
fn quarantine_legacy_uuid_verification(
    verification: WorkspaceUuidVerificationV1,
) -> WorkspaceUuidVerificationV1 {
    match verification {
        WorkspaceUuidVerificationV1::Unique => WorkspaceUuidVerificationV1::RegistryCheckRequired,
        verification => verification,
    }
}

/// Complete result of read-only Git discovery and validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedWorktreeV1 {
    identity: DurableWorktreeIdentityV1,
    roots: WorktreeRootsV1,
    kind: WorktreeKindV1,
    containment: ContainmentMetadataV1,
    move_metadata: WorktreeMoveMetadataV1,
    repair_metadata: WorktreeRepairMetadataV1,
}

impl ValidatedWorktreeV1 {
    pub fn new(
        identity: DurableWorktreeIdentityV1,
        roots: WorktreeRootsV1,
        kind: WorktreeKindV1,
        containment: ContainmentMetadataV1,
        move_metadata: WorktreeMoveMetadataV1,
        repair_metadata: WorktreeRepairMetadataV1,
    ) -> Result<Self, ValidatedWorktreeValidationErrorV1> {
        if identity.root_directory_fingerprint().is_none() {
            return Err(ValidatedWorktreeValidationErrorV1::MissingRootDirectoryFingerprint);
        }
        match (
            identity.state(),
            repair_metadata.workspace_uuid_verification(),
        ) {
            (
                WorkspaceIdentityStateV1::Candidate,
                WorkspaceUuidVerificationV1::PendingStoreInitialization,
            )
            | (
                WorkspaceIdentityStateV1::Bound,
                WorkspaceUuidVerificationV1::RegistryCheckRequired,
            ) => {}
            (WorkspaceIdentityStateV1::UnvalidatedLegacy, _) => {
                return Err(ValidatedWorktreeValidationErrorV1::UnvalidatedLegacyIdentity);
            }
            (_, WorkspaceUuidVerificationV1::Unique) => {
                return Err(
                    ValidatedWorktreeValidationErrorV1::RegistryCheckRequiredForLegacyVerification,
                );
            }
            (WorkspaceIdentityStateV1::Candidate, _) => {
                return Err(ValidatedWorktreeValidationErrorV1::PendingStoreInitializationRequired);
            }
            (WorkspaceIdentityStateV1::Bound, _) => {
                return Err(
                    ValidatedWorktreeValidationErrorV1::RegistryCheckRequiredForBoundIdentity,
                );
            }
        }
        if !identity
            .last_validated_root()
            .same_canonical_path(roots.worktree_root())
        {
            return Err(ValidatedWorktreeValidationErrorV1::IdentityRootMismatch);
        }
        if !containment
            .workspace_root()
            .same_canonical_path(roots.worktree_root())
        {
            return Err(ValidatedWorktreeValidationErrorV1::ContainmentRootMismatch);
        }
        if !has_exact_containment_path(
            roots.worktree_root(),
            containment.podway_directory(),
            b".podway",
        ) {
            return Err(ValidatedWorktreeValidationErrorV1::PodwayDirectoryOutsideWorkspace);
        }
        if !has_exact_containment_path(
            roots.worktree_root(),
            containment.runtime_directory(),
            b".podway/runtime",
        ) {
            return Err(ValidatedWorktreeValidationErrorV1::RuntimeDirectoryOutsidePodway);
        }
        if !move_metadata
            .current_root()
            .same_canonical_path(roots.worktree_root())
        {
            return Err(ValidatedWorktreeValidationErrorV1::MoveRootMismatch);
        }
        match (
            move_metadata.previous_root(),
            repair_metadata.registry_action(),
        ) {
            (None, RegistryRepairActionV1::NotRequired) => {}
            (
                Some(previous_root),
                RegistryRepairActionV1::UpdateValidatedRoot {
                    previous_root: repair_root,
                },
            ) if previous_root.same_canonical_path(repair_root) => {}
            _ => return Err(ValidatedWorktreeValidationErrorV1::RepairMetadataMismatch),
        }
        Ok(Self {
            identity,
            roots,
            kind,
            containment,
            move_metadata,
            repair_metadata,
        })
    }

    pub fn identity(&self) -> &DurableWorktreeIdentityV1 {
        &self.identity
    }

    pub fn roots(&self) -> &WorktreeRootsV1 {
        &self.roots
    }

    pub fn kind(&self) -> &WorktreeKindV1 {
        &self.kind
    }

    pub fn containment(&self) -> &ContainmentMetadataV1 {
        &self.containment
    }

    pub fn move_metadata(&self) -> &WorktreeMoveMetadataV1 {
        &self.move_metadata
    }

    pub fn repair_metadata(&self) -> &WorktreeRepairMetadataV1 {
        &self.repair_metadata
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectorValidationErrorV1 {
    UnsupportedVersion {
        found: u16,
    },
    EmptyPathBytes,
    EmbeddedNulPathByte,
    RelativePath,
    NonLocalPath,
    NonCanonicalPath,
    EmptyDisplay,
    NulInDisplay,
    FieldTooLong {
        field: &'static str,
        maximum_bytes: usize,
    },
    InvalidBase64Url,
    NonCanonicalBase64Url,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidatedWorktreeValidationErrorV1 {
    RelocationWithoutRootChange,
    MissingRootDirectoryFingerprint,
    UnvalidatedLegacyIdentity,
    PendingStoreInitializationRequired,
    RegistryCheckRequiredForBoundIdentity,
    RegistryCheckRequiredForLegacyVerification,
    IdentityRootMismatch,
    ContainmentRootMismatch,
    MoveRootMismatch,
    RepairMetadataMismatch,
    PodwayDirectoryOutsideWorkspace,
    RuntimeDirectoryOutsidePodway,
}

/// A stable digest of one canonical local artifact inside a validated worktree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HashedLocalArtifactV1 {
    canonical_path: LosslessPathV1,
    digest: Sha256Digest,
    byte_length: u64,
}

impl HashedLocalArtifactV1 {
    pub(crate) fn new(
        canonical_path: LosslessPathV1,
        digest: Sha256Digest,
        byte_length: u64,
    ) -> Self {
        Self {
            canonical_path,
            digest,
            byte_length,
        }
    }

    /// Returns the canonical, native-byte-preserving path that was hashed.
    pub fn canonical_path(&self) -> &LosslessPathV1 {
        &self.canonical_path
    }

    /// Returns the SHA-256 digest of the stable artifact bytes.
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    /// Returns the exact number of streamed artifact bytes.
    pub fn byte_length(&self) -> u64 {
        self.byte_length
    }
}

/// Frozen contract name for typed failures at the read-only Git/filesystem boundary.
pub type GitResolveErrorV1 = GitResolverErrorV1;

/// Typed failures at the read-only Git/filesystem boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitResolverErrorV1 {
    Selector(SelectorValidationErrorV1),
    NonGitRepository,
    BareRepository,
    PathEscape {
        path: LosslessPathV1,
    },
    SymlinkEscape {
        path: LosslessPathV1,
    },
    CopiedWorkspaceUuid {
        workspace_id: WorkspaceId,
    },
    IdentityMismatch {
        expected: Box<DurableWorktreeIdentityV1>,
        actual: Box<DurableWorktreeIdentityV1>,
    },
    MoveConflict {
        previous_root: LosslessPathV1,
        current_root: LosslessPathV1,
    },
    WorktreeDeleted,
    PermissionDenied {
        path: LosslessPathV1,
    },
    Io {
        operation: GitReadOperationV1,
    },
    Representation {
        problem: GitRepresentationProblemV1,
    },
    Invariant {
        problem: GitInvariantViolationV1,
    },
    /// A primary layout failure whose required rollback or cleanup also failed.
    WorkspaceLayoutCleanup {
        primary: Box<GitResolverErrorV1>,
        cleanup: Box<GitResolverErrorV1>,
    },
}
/// Typed failures while initializing the worktree-local Podway layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceLayoutErrorV1 {
    /// The supplied validation no longer describes the Git worktree.
    Revalidation { source: GitResolveErrorV1 },
    /// A descriptor-anchored layout operation failed.
    Initialization { source: GitResolverErrorV1 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitReadOperationV1 {
    DiscoverRepository,
    ReadGitFile,
    ReadGitDirectory,
    CanonicalizePath,
    ReadWorktreeMetadata,
    /// Validates the linked common directory record and target.
    ValidateCommonDirectory,
    /// Validates the linked worktree administration graph.
    ValidateWorktreeAdministration,
    /// Inspects `.podway` and its runtime directory.
    InspectRuntimeDirectory,
    /// Opens a local artifact with no-follow semantics.
    OpenLocalArtifact,
    /// Streams a local artifact after it was opened.
    ReadLocalArtifact,
    /// Creates or verifies the worktree-local Podway layout.
    InitializeWorkspaceLayout,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitRepresentationProblemV1 {
    MalformedGitDirectory,
    MalformedGitFile,
    /// The linked `commondir` record is malformed or inaccessible.
    MalformedCommonDirectory,
    /// The linked worktree reciprocal `gitdir` record is malformed.
    MalformedLinkedBacklink,
    /// The linked administration directory is not under `<common>/worktrees/<name>`.
    UnsupportedAdministrationRelationship,
    UnsupportedRepositoryLayout,
    /// An artifact is not a regular file.
    NonRegularArtifact,
    InvalidPathEncoding,
    /// A required worktree-local layout component is not a directory.
    WorkspaceLayoutComponentNotDirectory,
    /// The worktree-local ignore file is not a regular file.
    WorkspaceLayoutIgnoreNotRegular,
    /// The worktree-local ignore file exceeds the bounded initialization size.
    WorkspaceLayoutIgnoreFileTooLarge,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitInvariantViolationV1 {
    WorktreeRootIsBare,
    CommonDirectoryIdentityMissing,
    WorktreeAdministrationIdentityMissing,
    RuntimeDirectoryIsSymlink,
    /// Metadata changed while a directory layout was being validated.
    MetadataChangedDuringResolution,
    /// Metadata changed while an artifact was opened or streamed.
    MetadataChangedDuringArtifactHash,
    /// A worktree-local layout entry changed while it was being initialized.
    MetadataChangedDuringWorkspaceLayout,
    /// The platform cannot provide the required local filesystem identity.
    UnsupportedFilesystemIdentity,
}

fn has_exact_containment_path(
    root: &LosslessPathV1,
    candidate: &LosslessPathV1,
    suffix: &[u8],
) -> bool {
    let (Ok(mut expected), Ok(candidate)) =
        (root.decode_path_bytes(), candidate.decode_path_bytes())
    else {
        return false;
    };
    if expected != b"/" {
        expected.push(b'/');
    }
    expected.extend_from_slice(suffix);
    expected == candidate
}

fn validate_component_size(
    value: &str,
    field: &'static str,
) -> Result<(), SelectorValidationErrorV1> {
    if value.len() > MAX_SELECTOR_COMPONENT_BYTES_V1 {
        return Err(SelectorValidationErrorV1::FieldTooLong {
            field,
            maximum_bytes: MAX_SELECTOR_COMPONENT_BYTES_V1,
        });
    }
    Ok(())
}

fn validate_path_bytes(value: &[u8]) -> Result<(), SelectorValidationErrorV1> {
    if value.is_empty() {
        return Err(SelectorValidationErrorV1::EmptyPathBytes);
    }
    if value.contains(&0) {
        return Err(SelectorValidationErrorV1::EmbeddedNulPathByte);
    }
    if value.len() > MAX_SELECTOR_COMPONENT_BYTES_V1 {
        return Err(SelectorValidationErrorV1::FieldTooLong {
            field: "path_bytes",
            maximum_bytes: MAX_SELECTOR_COMPONENT_BYTES_V1,
        });
    }
    if is_non_local_path_representation(value) {
        return Err(SelectorValidationErrorV1::NonLocalPath);
    }
    if value.first() != Some(&b'/') {
        return Err(SelectorValidationErrorV1::RelativePath);
    }
    if value != b"/"
        && value[1..]
            .split(|byte| *byte == b'/')
            .any(|component| component.is_empty() || component == b"." || component == b"..")
    {
        return Err(SelectorValidationErrorV1::NonCanonicalPath);
    }
    Ok(())
}

fn is_non_local_path_representation(value: &[u8]) -> bool {
    value.starts_with(b"//")
        || value.starts_with(b"\\\\")
        || value.starts_with(b"file:")
        || (value.len() >= 3
            && value[0].is_ascii_alphabetic()
            && value[1] == b':'
            && matches!(value[2], b'/' | b'\\'))
}

fn encode_base64url(value: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    let mut encoded = String::with_capacity(value.len().div_ceil(3) * 4);
    for chunk in value.chunks(3) {
        let first = chunk[0];
        encoded.push(char::from(ALPHABET[usize::from(first >> 2)]));
        match chunk.len() {
            1 => {
                encoded.push(char::from(
                    ALPHABET[usize::from((first & 0b0000_0011) << 4)],
                ));
            }
            2 => {
                let second = chunk[1];
                encoded.push(char::from(
                    ALPHABET[usize::from(((first & 0b0000_0011) << 4) | (second >> 4))],
                ));
                encoded.push(char::from(
                    ALPHABET[usize::from((second & 0b0000_1111) << 2)],
                ));
            }
            3 => {
                let second = chunk[1];
                let third = chunk[2];
                encoded.push(char::from(
                    ALPHABET[usize::from(((first & 0b0000_0011) << 4) | (second >> 4))],
                ));
                encoded.push(char::from(
                    ALPHABET[usize::from(((second & 0b0000_1111) << 2) | (third >> 6))],
                ));
                encoded.push(char::from(ALPHABET[usize::from(third & 0b0011_1111)]));
            }
            _ => unreachable!(),
        }
    }
    encoded
}

fn decode_base64url(value: &str) -> Result<Vec<u8>, SelectorValidationErrorV1> {
    if value.is_empty() || value.len() % 4 == 1 {
        return Err(SelectorValidationErrorV1::InvalidBase64Url);
    }

    let mut decoded = Vec::with_capacity(value.len() / 4 * 3 + 2);
    let mut buffer = 0_u32;
    let mut buffered_bits = 0_u8;
    for byte in value.bytes() {
        let sextet = decode_base64url_byte(byte)?;
        buffer = (buffer << 6) | u32::from(sextet);
        buffered_bits += 6;
        while buffered_bits >= 8 {
            buffered_bits -= 8;
            decoded.push(((buffer >> buffered_bits) & 0xff) as u8);
            if buffered_bits == 0 {
                buffer = 0;
            } else {
                buffer &= (1_u32 << buffered_bits) - 1;
            }
        }
    }
    if buffer != 0 {
        return Err(SelectorValidationErrorV1::NonCanonicalBase64Url);
    }
    Ok(decoded)
}

fn decode_base64url_byte(value: u8) -> Result<u8, SelectorValidationErrorV1> {
    match value {
        b'A'..=b'Z' => Ok(value - b'A'),
        b'a'..=b'z' => Ok(value - b'a' + 26),
        b'0'..=b'9' => Ok(value - b'0' + 52),
        b'-' => Ok(62),
        b'_' => Ok(63),
        _ => Err(SelectorValidationErrorV1::InvalidBase64Url),
    }
}
