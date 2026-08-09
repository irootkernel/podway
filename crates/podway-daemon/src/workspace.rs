//! Daemon-authoritative, read-only workspace resolution.
//!
//! Git first establishes a validated worktree snapshot. The Store binding is then inspected
//! without mutation, and Git is resolved again with the durable binding before a caller may open
//! or update SQLite. Paths remain diagnostic and routing data; the scheduler key is derived only
//! from the store-validated UUID and Git administration fingerprints.

use std::{
    error::Error,
    fmt,
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(unix)]
use std::{
    ffi::OsString,
    os::unix::{
        ffi::OsStringExt,
        fs::{MetadataExt, PermissionsExt},
    },
};

#[cfg(unix)]
use nix::{
    errno::Errno,
    fcntl::{AtFlags, OFlag, open, openat},
    sys::stat::Mode,
    unistd::{UnlinkatFlags, getuid, linkat, unlinkat},
};
use podway_core::{UnixMillis, WorkspaceId, canonicalize_json_v1, verify_canonical_json_v1};
use podway_git::{
    DiagnosticPathDisplayV1, DurableWorktreeIdentityV1 as GitWorktreeIdentityV1, GitResolveErrorV1,
    GitResolverContractV1, LosslessPathV1, SelectorValidationErrorV1, ValidatedWorktreeV1,
    WorkspaceIdentityStateV1, WorktreeMoveMetadataV1, WorktreeSelectorV1,
};
use podway_protocol::RequestIdV1;
use podway_store::{
    CanonicalRequestDigestV1, DurableWorktreeIdentityV1,
    DurableWorktreeIdentityV1 as StoreWorktreeIdentityV1, IdempotencyKeyV1, JobIdV1,
    SqliteStoreOptionsV1, SqliteStoreV1, StoreErrorV1, StoreValueErrorV1, ValidatedWorkspaceRootV1,
    WorkspaceBindingV1,
};

use crate::scheduler::WorkspaceSchedulerKeyV1;

const STATE_DATABASE_FILE_NAME_V1: &str = "state.sqlite3";
const STORED_ROOT_DIAGNOSTIC_V1: &str = "store-validated workspace root";
/// The sole reset-marker schema accepted as destructive-reset recovery authority.
pub const RESET_MARKER_SCHEMA_V1: &str = "podway.reset-marker/v1";
/// Reset marker carrying the immutable public request correlation needed by terminal replay.
pub const RESET_MARKER_SCHEMA_V2: &str = "podway.reset-marker/v2";
/// Reset-marker decoding is bounded before parsing so a local corrupt file cannot amplify recovery.
pub const MAX_RESET_MARKER_BYTES_V1: u64 = 16 * 1024;
const RESET_MARKER_FILE_NAME_V1: &str = "reset.marker";
pub(crate) const DEVELOPMENT_V2_MARKER_FILE_NAME_V1: &str = "development-v2.marker";
pub(crate) const MAX_DEVELOPMENT_V2_MARKER_BYTES_V1: u64 = 4 * 1024;
const STATE_DATABASE_FILE_WAL_NAME_V1: &str = "state.sqlite3-wal";
const STATE_DATABASE_FILE_SHM_NAME_V1: &str = "state.sqlite3-shm";
const PRIVATE_RUNTIME_DIRECTORY_MODE_V1: u32 = 0o700;
const PRIVATE_RUNTIME_FILE_MODE_V1: u32 = 0o600;
const RESET_MARKER_TEMPORARY_NAME_ATTEMPTS_V1: usize = 128;

static RESET_MARKER_TEMPORARY_SEQUENCE_V1: AtomicU64 = AtomicU64::new(0);

/// Exact durable recovery input for a reset-all operation.
///
/// This document deliberately contains no path, selector, or terminal timestamp. The
/// Git-validated runtime descriptor remains filesystem authority; the immutable predecessor UUID
/// binds recovery to one registry generation before it can seed or verify the target Store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResetMarkerV1 {
    operation_id: JobIdV1,
    idempotency_key: IdempotencyKeyV1,
    request_digest: CanonicalRequestDigestV1,
    previous_workspace_uuid: WorkspaceId,
    target_workspace_uuid: WorkspaceId,
    submitted_at_ms: UnixMillis,
    response_request_id: Option<RequestIdV1>,
}

impl ResetMarkerV1 {
    pub fn new(
        operation_id: JobIdV1,
        idempotency_key: IdempotencyKeyV1,
        request_digest: CanonicalRequestDigestV1,
        previous_workspace_uuid: WorkspaceId,
        target_workspace_uuid: WorkspaceId,
        submitted_at_ms: UnixMillis,
    ) -> Self {
        Self {
            operation_id,
            idempotency_key,
            request_digest,
            previous_workspace_uuid,
            target_workspace_uuid,
            submitted_at_ms,
            response_request_id: None,
        }
    }

    pub fn new_with_response_request_id(
        operation_id: JobIdV1,
        idempotency_key: IdempotencyKeyV1,
        request_digest: CanonicalRequestDigestV1,
        previous_workspace_uuid: WorkspaceId,
        target_workspace_uuid: WorkspaceId,
        submitted_at_ms: UnixMillis,
        response_request_id: RequestIdV1,
    ) -> Self {
        Self {
            operation_id,
            idempotency_key,
            request_digest,
            previous_workspace_uuid,
            target_workspace_uuid,
            submitted_at_ms,
            response_request_id: Some(response_request_id),
        }
    }

    pub fn operation_id(&self) -> &JobIdV1 {
        &self.operation_id
    }

    pub fn idempotency_key(&self) -> &IdempotencyKeyV1 {
        &self.idempotency_key
    }

    pub fn request_digest(&self) -> &CanonicalRequestDigestV1 {
        &self.request_digest
    }
    pub fn previous_workspace_uuid(&self) -> &WorkspaceId {
        &self.previous_workspace_uuid
    }

    pub fn target_workspace_uuid(&self) -> &WorkspaceId {
        &self.target_workspace_uuid
    }

    pub const fn submitted_at_ms(&self) -> UnixMillis {
        self.submitted_at_ms
    }

    pub fn response_request_id(&self) -> Option<&RequestIdV1> {
        self.response_request_id.as_ref()
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ResetMarkerErrorV1> {
        let encoded = match &self.response_request_id {
            Some(response_request_id) => canonicalize_json_v1(&SerializableResetMarkerRefV2 {
                schema: RESET_MARKER_SCHEMA_V2,
                operation_id: self.operation_id.as_str(),
                idempotency_key: self.idempotency_key.as_str(),
                request_digest: self.request_digest.as_str(),
                previous_workspace_uuid: self.previous_workspace_uuid.as_str(),
                target_workspace_uuid: self.target_workspace_uuid.as_str(),
                submitted_at_ms: self.submitted_at_ms.get(),
                response_request_id: response_request_id.as_str(),
            }),
            None => canonicalize_json_v1(&SerializableResetMarkerRefV1 {
                schema: RESET_MARKER_SCHEMA_V1,
                operation_id: self.operation_id.as_str(),
                idempotency_key: self.idempotency_key.as_str(),
                request_digest: self.request_digest.as_str(),
                previous_workspace_uuid: self.previous_workspace_uuid.as_str(),
                target_workspace_uuid: self.target_workspace_uuid.as_str(),
                submitted_at_ms: self.submitted_at_ms.get(),
            }),
        };
        encoded
            .map(String::into_bytes)
            .map_err(|_| ResetMarkerErrorV1::Encode)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, ResetMarkerErrorV1> {
        if bytes.len() as u64 > MAX_RESET_MARKER_BYTES_V1 {
            return Err(ResetMarkerErrorV1::TooLarge {
                maximum: MAX_RESET_MARKER_BYTES_V1,
                actual: bytes.len() as u64,
            });
        }
        match verify_canonical_json_v1(bytes) {
            Ok(()) => {}
            Err(podway_core::CanonicalJsonErrorV1::InvalidJson(_)) => {
                return Err(ResetMarkerErrorV1::InvalidJson);
            }
            Err(_) => return Err(ResetMarkerErrorV1::NonCanonical),
        }
        let serialized: SerializedResetMarkerV1 =
            serde_json::from_slice(bytes).map_err(|_| ResetMarkerErrorV1::InvalidShape)?;
        if serialized.schema != RESET_MARKER_SCHEMA_V1
            && serialized.schema != RESET_MARKER_SCHEMA_V2
        {
            return Err(ResetMarkerErrorV1::UnsupportedSchema);
        }
        if (serialized.schema == RESET_MARKER_SCHEMA_V1 && serialized.response_request_id.is_some())
            || (serialized.schema == RESET_MARKER_SCHEMA_V2
                && serialized.response_request_id.is_none())
        {
            return Err(ResetMarkerErrorV1::InvalidShape);
        }
        let marker = Self {
            operation_id: JobIdV1::new(serialized.operation_id)
                .map_err(|_| ResetMarkerErrorV1::InvalidOperationId)?,
            idempotency_key: IdempotencyKeyV1::new(serialized.idempotency_key)
                .map_err(|_| ResetMarkerErrorV1::InvalidIdempotencyKey)?,
            request_digest: CanonicalRequestDigestV1::new(serialized.request_digest)
                .map_err(|_| ResetMarkerErrorV1::InvalidRequestDigest)?,
            previous_workspace_uuid: WorkspaceId::new(serialized.previous_workspace_uuid)
                .map_err(|_| ResetMarkerErrorV1::InvalidTargetWorkspaceUuid)?,
            target_workspace_uuid: WorkspaceId::new(serialized.target_workspace_uuid)
                .map_err(|_| ResetMarkerErrorV1::InvalidTargetWorkspaceUuid)?,
            submitted_at_ms: UnixMillis::new(serialized.submitted_at_ms),
            response_request_id: serialized
                .response_request_id
                .map(RequestIdV1::new)
                .transpose()
                .map_err(|_| ResetMarkerErrorV1::InvalidResponseRequestId)?,
        };
        if marker.canonical_bytes()?.as_slice() != bytes {
            return Err(ResetMarkerErrorV1::NonCanonical);
        }
        Ok(marker)
    }
}

/// Strict reset-marker codec failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResetMarkerErrorV1 {
    TooLarge { maximum: u64, actual: u64 },
    InvalidJson,
    NonCanonical,
    InvalidShape,
    UnsupportedSchema,
    InvalidOperationId,
    InvalidIdempotencyKey,
    InvalidRequestDigest,
    InvalidTargetWorkspaceUuid,
    InvalidResponseRequestId,
    Encode,
}

impl fmt::Display for ResetMarkerErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { maximum, actual } => {
                write!(
                    formatter,
                    "reset marker is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::InvalidJson => formatter.write_str("reset marker is not valid JSON"),
            Self::NonCanonical => formatter.write_str("reset marker is not canonical JSON"),
            Self::InvalidShape => formatter.write_str("reset marker has an invalid strict shape"),
            Self::UnsupportedSchema => {
                formatter.write_str("reset marker has an unsupported schema")
            }
            Self::InvalidOperationId => {
                formatter.write_str("reset marker has an invalid operation ID")
            }
            Self::InvalidIdempotencyKey => {
                formatter.write_str("reset marker has an invalid idempotency key")
            }
            Self::InvalidRequestDigest => {
                formatter.write_str("reset marker has an invalid request digest")
            }
            Self::InvalidTargetWorkspaceUuid => {
                formatter.write_str("reset marker has an invalid target workspace UUID")
            }
            Self::InvalidResponseRequestId => {
                formatter.write_str("reset marker has an invalid response request ID")
            }
            Self::Encode => formatter.write_str("reset marker cannot be canonically encoded"),
        }
    }
}

impl Error for ResetMarkerErrorV1 {}

/// A path property that makes a Git-validated runtime directory or one of its fixed files unsafe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeDirectoryPathViolationV1 {
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

impl fmt::Display for RuntimeDirectoryPathViolationV1 {
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
/// Opaque manager-issued authority for descriptor-relative destructive reset maintenance.
pub(crate) struct ResetMaintenanceFilesystemTokenV1 {
    _private: (),
}

impl ResetMaintenanceFilesystemTokenV1 {
    pub(crate) const fn issue() -> Self {
        Self { _private: () }
    }
}

/// Deterministic marker-publication failure boundary for filesystem contract tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetMarkerPublicationFailpointV1 {
    BeforeLinkAndTemporaryCleanup,
    AfterLinkBeforeDirectorySync,
    AfterDirectorySyncBeforeCleanup,
}

/// The strongest admission conclusion available when reset-marker publication fails.
#[derive(Debug)]
pub enum ResetMarkerPublicationErrorV1 {
    NotPublished(ValidatedRuntimeDirectoryErrorV1),
    OutcomeUnknown(ValidatedRuntimeDirectoryErrorV1),
    Published(ValidatedRuntimeDirectoryErrorV1),
}

impl ResetMarkerPublicationErrorV1 {
    pub fn into_source(self) -> ValidatedRuntimeDirectoryErrorV1 {
        match self {
            Self::NotPublished(source) | Self::OutcomeUnknown(source) | Self::Published(source) => {
                source
            }
        }
    }
}

/// Fail-closed descriptor-relative reset-marker and fixed-file maintenance errors.
#[derive(Debug)]
pub enum ValidatedRuntimeDirectoryErrorV1 {
    UnsupportedPlatform,
    Path {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    UnsafeDirectory {
        path: PathBuf,
        violation: RuntimeDirectoryPathViolationV1,
    },
    UnsafeFile {
        path: PathBuf,
        violation: RuntimeDirectoryPathViolationV1,
    },
    Marker(ResetMarkerErrorV1),
    MarkerAlreadyExists,
    MarkerMissing,
    FileIdentityChanged {
        path: PathBuf,
    },
    TemporaryNameExhausted {
        path: PathBuf,
    },
    /// Both errors are retained when a publication failure also leaves its temporary behind.
    PublicationAndTemporaryCleanup {
        publication: Box<ValidatedRuntimeDirectoryErrorV1>,
        temporary_cleanup: Box<ValidatedRuntimeDirectoryErrorV1>,
    },
    /// A published marker or failed publication left an exact temporary path for recovery.
    TemporaryCleanup {
        temporary_path: PathBuf,
        cleanup: Box<ValidatedRuntimeDirectoryErrorV1>,
    },
    /// A deterministic test seam interrupted marker publication.
    Failpoint {
        point: ResetMarkerPublicationFailpointV1,
    },
}

impl ValidatedRuntimeDirectoryErrorV1 {
    /// Returns true only when opening the validated runtime directory proved it does not exist.
    /// Other path errors remain fail-closed.
    pub(crate) fn is_missing_directory(&self) -> bool {
        matches!(
            self,
            Self::Path {
                operation: "open runtime directory",
                source,
                ..
            } if source.kind() == io::ErrorKind::NotFound
        )
    }
}

impl fmt::Display for ValidatedRuntimeDirectoryErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                formatter.write_str("reset runtime-directory operations require Unix support")
            }
            Self::Path {
                operation,
                path,
                source,
            } => write!(formatter, "cannot {operation} {}: {source}", path.display()),
            Self::UnsafeDirectory { path, violation } => {
                write!(
                    formatter,
                    "runtime directory {} {violation}",
                    path.display()
                )
            }
            Self::UnsafeFile { path, violation } => {
                write!(formatter, "runtime file {} {violation}", path.display())
            }
            Self::Marker(error) => error.fmt(formatter),
            Self::MarkerAlreadyExists => formatter.write_str("reset marker already exists"),
            Self::MarkerMissing => {
                formatter.write_str("reset marker disappeared during maintenance")
            }
            Self::FileIdentityChanged { path } => {
                write!(
                    formatter,
                    "runtime file {} changed during maintenance",
                    path.display()
                )
            }
            Self::TemporaryNameExhausted { path } => write!(
                formatter,
                "could not create a reset marker temporary in {}",
                path.display()
            ),
            Self::PublicationAndTemporaryCleanup {
                publication,
                temporary_cleanup,
            } => write!(
                formatter,
                "reset marker publication failed: {publication}; temporary cleanup also failed: {temporary_cleanup}"
            ),
            Self::TemporaryCleanup {
                temporary_path,
                cleanup,
            } => write!(
                formatter,
                "reset marker temporary {} could not be removed: {cleanup}",
                temporary_path.display()
            ),
            Self::Failpoint { point } => {
                write!(
                    formatter,
                    "reset marker publication failpoint {point:?} triggered"
                )
            }
        }
    }
}

impl Error for ValidatedRuntimeDirectoryErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path { source, .. } => Some(source),
            Self::Marker(error) => Some(error),
            Self::PublicationAndTemporaryCleanup { publication, .. } => Some(publication),
            Self::TemporaryCleanup { cleanup, .. } => Some(cleanup),
            _ => None,
        }
    }
}

/// A descriptor opened only from Git-validated containment evidence.
///
/// Its fixed-name methods never accept a caller-derived deletion path.
pub struct ValidatedRuntimeDirectoryV1 {
    path: PathBuf,
    #[cfg(unix)]
    directory: File,
    #[cfg(unix)]
    current_uid: u32,
}

impl ValidatedRuntimeDirectoryV1 {
    pub fn open(worktree: &ValidatedWorktreeV1) -> Result<Self, ValidatedRuntimeDirectoryErrorV1> {
        #[cfg(unix)]
        {
            open_validated_runtime_directory_unix(worktree)
        }
        #[cfg(not(unix))]
        {
            let _ = worktree;
            Err(ValidatedRuntimeDirectoryErrorV1::UnsupportedPlatform)
        }
    }

    /// Returns only a diagnostic path. It is never accepted back as maintenance authority.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read_reset_marker(
        &self,
    ) -> Result<Option<ResetMarkerV1>, ValidatedRuntimeDirectoryErrorV1> {
        #[cfg(unix)]
        {
            let Some((mut file, _)) = self.open_fixed_private_file(RESET_MARKER_FILE_NAME_V1)?
            else {
                return Ok(None);
            };
            let bytes = read_bounded_runtime_file(
                &mut file,
                MAX_RESET_MARKER_BYTES_V1,
                &self.path.join(RESET_MARKER_FILE_NAME_V1),
            )?;
            ResetMarkerV1::decode_canonical(&bytes)
                .map(Some)
                .map_err(ValidatedRuntimeDirectoryErrorV1::Marker)
        }
        #[cfg(not(unix))]
        {
            Err(ValidatedRuntimeDirectoryErrorV1::UnsupportedPlatform)
        }
    }

    /// Reads the fixed development-v2 marker through the already validated runtime-directory
    /// descriptor. The admission policy owns decoding; this boundary owns symlink-safe bounded
    /// filesystem access.
    pub(crate) fn read_development_v2_marker_bytes(
        &self,
    ) -> Result<Option<Vec<u8>>, ValidatedRuntimeDirectoryErrorV1> {
        #[cfg(unix)]
        {
            let Some((mut file, _)) =
                self.open_fixed_private_file(DEVELOPMENT_V2_MARKER_FILE_NAME_V1)?
            else {
                return Ok(None);
            };
            read_bounded_runtime_file(
                &mut file,
                MAX_DEVELOPMENT_V2_MARKER_BYTES_V1,
                &self.path.join(DEVELOPMENT_V2_MARKER_FILE_NAME_V1),
            )
            .map(Some)
        }
        #[cfg(not(unix))]
        {
            Err(ValidatedRuntimeDirectoryErrorV1::UnsupportedPlatform)
        }
    }

    /// Atomically publishes a strict marker with no replacement of an existing marker.
    pub(crate) fn publish_reset_marker(
        &self,
        _authority: &ResetMaintenanceFilesystemTokenV1,
        marker: &ResetMarkerV1,
    ) -> Result<(), ResetMarkerPublicationErrorV1> {
        self.publish_reset_marker_with_optional_failpoint(marker, None)
    }

    /// Injects a publication boundary failure for deterministic filesystem contract tests.
    #[cfg(test)]
    pub(crate) fn publish_reset_marker_with_failpoint(
        &self,
        _authority: &ResetMaintenanceFilesystemTokenV1,
        marker: &ResetMarkerV1,
        failpoint: ResetMarkerPublicationFailpointV1,
    ) -> Result<(), ResetMarkerPublicationErrorV1> {
        self.publish_reset_marker_with_optional_failpoint(marker, Some(failpoint))
    }

    fn publish_reset_marker_with_optional_failpoint(
        &self,
        marker: &ResetMarkerV1,
        failpoint: Option<ResetMarkerPublicationFailpointV1>,
    ) -> Result<(), ResetMarkerPublicationErrorV1> {
        #[cfg(unix)]
        {
            self.publish_reset_marker_unix(marker, failpoint)
        }
        #[cfg(not(unix))]
        {
            let _ = (marker, failpoint);
            Err(ResetMarkerPublicationErrorV1::NotPublished(
                ValidatedRuntimeDirectoryErrorV1::UnsupportedPlatform,
            ))
        }
    }

    /// Removes the marker only after re-opening, decoding, and identity-checking the exact marker.
    pub(crate) fn remove_reset_marker(
        &self,
        _authority: &ResetMaintenanceFilesystemTokenV1,
        expected: &ResetMarkerV1,
    ) -> Result<(), ValidatedRuntimeDirectoryErrorV1> {
        #[cfg(unix)]
        {
            let identity = self.require_exact_reset_marker(expected)?;
            self.unlink_owned_fixed_file(RESET_MARKER_FILE_NAME_V1, identity)?;
            self.sync_directory()
        }
        #[cfg(not(unix))]
        {
            let _ = expected;
            Err(ValidatedRuntimeDirectoryErrorV1::UnsupportedPlatform)
        }
    }
    /// Removes only the three fixed SQLite names while the exact published marker remains present.
    ///
    /// Missing names are idempotent; links, non-regular files, foreign owners, non-private modes,
    /// a missing marker, or a changed marker fail closed.
    pub(crate) fn remove_reset_database_files(
        &self,
        _authority: &ResetMaintenanceFilesystemTokenV1,
        expected_marker: &ResetMarkerV1,
    ) -> Result<(), ValidatedRuntimeDirectoryErrorV1> {
        #[cfg(unix)]
        {
            let marker_identity = self.require_exact_reset_marker(expected_marker)?;
            for name in [
                STATE_DATABASE_FILE_NAME_V1,
                STATE_DATABASE_FILE_WAL_NAME_V1,
                STATE_DATABASE_FILE_SHM_NAME_V1,
            ] {
                if let Some((file, identity)) = self.open_fixed_private_file(name)? {
                    drop(file);
                    self.require_exact_reset_marker_with_identity(
                        expected_marker,
                        marker_identity,
                    )?;
                    self.unlink_owned_fixed_file(name, identity)?;
                }
            }
            self.sync_directory()
        }
        #[cfg(not(unix))]
        {
            let _ = expected_marker;
            Err(ValidatedRuntimeDirectoryErrorV1::UnsupportedPlatform)
        }
    }
    #[cfg(unix)]
    fn require_exact_reset_marker(
        &self,
        expected: &ResetMarkerV1,
    ) -> Result<RuntimeFileIdentityV1, ValidatedRuntimeDirectoryErrorV1> {
        let Some((mut file, identity)) = self.open_fixed_private_file(RESET_MARKER_FILE_NAME_V1)?
        else {
            return Err(ValidatedRuntimeDirectoryErrorV1::MarkerMissing);
        };
        let marker_path = self.path.join(RESET_MARKER_FILE_NAME_V1);
        let bytes = read_bounded_runtime_file(&mut file, MAX_RESET_MARKER_BYTES_V1, &marker_path)?;
        let actual = ResetMarkerV1::decode_canonical(&bytes)
            .map_err(ValidatedRuntimeDirectoryErrorV1::Marker)?;
        if &actual != expected {
            return Err(ValidatedRuntimeDirectoryErrorV1::FileIdentityChanged {
                path: marker_path,
            });
        }
        Ok(identity)
    }
    #[cfg(unix)]
    fn require_exact_reset_marker_with_identity(
        &self,
        expected: &ResetMarkerV1,
        expected_identity: RuntimeFileIdentityV1,
    ) -> Result<(), ValidatedRuntimeDirectoryErrorV1> {
        let actual_identity = self.require_exact_reset_marker(expected)?;
        if actual_identity != expected_identity {
            return Err(ValidatedRuntimeDirectoryErrorV1::FileIdentityChanged {
                path: self.path.join(RESET_MARKER_FILE_NAME_V1),
            });
        }
        Ok(())
    }

    #[cfg(unix)]
    fn open_fixed_private_file(
        &self,
        name: &str,
    ) -> Result<Option<(File, RuntimeFileIdentityV1)>, ValidatedRuntimeDirectoryErrorV1> {
        let path = self.path.join(name);
        let descriptor = match openat(
            &self.directory,
            name,
            OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_RDONLY,
            Mode::empty(),
        ) {
            Ok(descriptor) => descriptor,
            Err(Errno::ENOENT) => return Ok(None),
            Err(source) => {
                return Err(runtime_path_error("open runtime file", path, source.into()));
            }
        };
        let file = File::from(descriptor);
        let metadata = file
            .metadata()
            .map_err(|source| runtime_path_error("inspect runtime file", path.clone(), source))?;
        validate_runtime_file(&path, &metadata, self.current_uid)?;
        Ok(Some((
            file,
            RuntimeFileIdentityV1::from_metadata(&metadata),
        )))
    }

    #[cfg(unix)]
    fn cleanup_marker_temporary(
        &self,
        temporary_name: &str,
        temporary_path: &Path,
        identity: RuntimeFileIdentityV1,
        injected_failure: Option<ResetMarkerPublicationFailpointV1>,
    ) -> Result<(), ValidatedRuntimeDirectoryErrorV1> {
        let cleanup = match injected_failure {
            Some(point) => Err(ValidatedRuntimeDirectoryErrorV1::Failpoint { point }),
            None => self.unlink_owned_fixed_file(temporary_name, identity),
        };
        cleanup.map_err(
            |cleanup| ValidatedRuntimeDirectoryErrorV1::TemporaryCleanup {
                temporary_path: temporary_path.to_path_buf(),
                cleanup: Box::new(cleanup),
            },
        )
    }

    #[cfg(unix)]
    fn publication_failure_with_temporary_cleanup(
        &self,
        temporary_name: &str,
        temporary_path: &Path,
        identity: RuntimeFileIdentityV1,
        publication: ValidatedRuntimeDirectoryErrorV1,
        injected_cleanup_failure: Option<ResetMarkerPublicationFailpointV1>,
    ) -> ValidatedRuntimeDirectoryErrorV1 {
        match self.cleanup_marker_temporary(
            temporary_name,
            temporary_path,
            identity,
            injected_cleanup_failure,
        ) {
            Ok(()) => publication,
            Err(temporary_cleanup) => {
                ValidatedRuntimeDirectoryErrorV1::PublicationAndTemporaryCleanup {
                    publication: Box::new(publication),
                    temporary_cleanup: Box::new(temporary_cleanup),
                }
            }
        }
    }

    #[cfg(unix)]
    fn publish_reset_marker_unix(
        &self,
        marker: &ResetMarkerV1,
        failpoint: Option<ResetMarkerPublicationFailpointV1>,
    ) -> Result<(), ResetMarkerPublicationErrorV1> {
        let bytes = marker
            .canonical_bytes()
            .map_err(ValidatedRuntimeDirectoryErrorV1::Marker)
            .map_err(ResetMarkerPublicationErrorV1::NotPublished)?;
        let (temporary_name, temporary_path, mut temporary, identity) = self
            .create_marker_temporary()
            .map_err(ResetMarkerPublicationErrorV1::NotPublished)?;
        if let Err(source) = temporary.write_all(&bytes) {
            drop(temporary);
            let publication = runtime_path_error(
                "write reset marker temporary",
                temporary_path.clone(),
                source,
            );
            return Err(ResetMarkerPublicationErrorV1::NotPublished(
                self.publication_failure_with_temporary_cleanup(
                    &temporary_name,
                    &temporary_path,
                    identity,
                    publication,
                    None,
                ),
            ));
        }
        if let Err(source) = temporary.sync_all() {
            drop(temporary);
            let publication = runtime_path_error(
                "sync reset marker temporary",
                temporary_path.clone(),
                source,
            );
            return Err(ResetMarkerPublicationErrorV1::NotPublished(
                self.publication_failure_with_temporary_cleanup(
                    &temporary_name,
                    &temporary_path,
                    identity,
                    publication,
                    None,
                ),
            ));
        }
        drop(temporary);
        if failpoint == Some(ResetMarkerPublicationFailpointV1::BeforeLinkAndTemporaryCleanup) {
            return Err(ResetMarkerPublicationErrorV1::NotPublished(
                self.publication_failure_with_temporary_cleanup(
                    &temporary_name,
                    &temporary_path,
                    identity,
                    ValidatedRuntimeDirectoryErrorV1::Failpoint {
                        point: ResetMarkerPublicationFailpointV1::BeforeLinkAndTemporaryCleanup,
                    },
                    Some(ResetMarkerPublicationFailpointV1::BeforeLinkAndTemporaryCleanup),
                ),
            ));
        }
        match linkat(
            &self.directory,
            temporary_name.as_str(),
            &self.directory,
            RESET_MARKER_FILE_NAME_V1,
            AtFlags::empty(),
        ) {
            Ok(()) => {}
            Err(Errno::EEXIST) => {
                return Err(ResetMarkerPublicationErrorV1::NotPublished(
                    self.publication_failure_with_temporary_cleanup(
                        &temporary_name,
                        &temporary_path,
                        identity,
                        ValidatedRuntimeDirectoryErrorV1::MarkerAlreadyExists,
                        None,
                    ),
                ));
            }
            Err(source) => {
                let publication = runtime_path_error(
                    "publish reset marker",
                    self.path.join(RESET_MARKER_FILE_NAME_V1),
                    source.into(),
                );
                return Err(ResetMarkerPublicationErrorV1::NotPublished(
                    self.publication_failure_with_temporary_cleanup(
                        &temporary_name,
                        &temporary_path,
                        identity,
                        publication,
                        None,
                    ),
                ));
            }
        }
        if failpoint == Some(ResetMarkerPublicationFailpointV1::AfterLinkBeforeDirectorySync) {
            return Err(ResetMarkerPublicationErrorV1::OutcomeUnknown(
                ValidatedRuntimeDirectoryErrorV1::Failpoint {
                    point: ResetMarkerPublicationFailpointV1::AfterLinkBeforeDirectorySync,
                },
            ));
        }
        if let Err(publication) = self.sync_directory() {
            return Err(ResetMarkerPublicationErrorV1::OutcomeUnknown(
                self.publication_failure_with_temporary_cleanup(
                    &temporary_name,
                    &temporary_path,
                    identity,
                    publication,
                    None,
                ),
            ));
        }
        if failpoint == Some(ResetMarkerPublicationFailpointV1::AfterDirectorySyncBeforeCleanup) {
            return Err(ResetMarkerPublicationErrorV1::Published(
                ValidatedRuntimeDirectoryErrorV1::Failpoint {
                    point: ResetMarkerPublicationFailpointV1::AfterDirectorySyncBeforeCleanup,
                },
            ));
        }
        self.cleanup_marker_temporary(&temporary_name, &temporary_path, identity, None)
            .map_err(ResetMarkerPublicationErrorV1::Published)?;
        self.sync_directory()
            .map_err(ResetMarkerPublicationErrorV1::Published)
    }

    #[cfg(unix)]
    fn create_marker_temporary(
        &self,
    ) -> Result<(String, PathBuf, File, RuntimeFileIdentityV1), ValidatedRuntimeDirectoryErrorV1>
    {
        for _ in 0..RESET_MARKER_TEMPORARY_NAME_ATTEMPTS_V1 {
            let sequence = RESET_MARKER_TEMPORARY_SEQUENCE_V1.fetch_add(1, Ordering::Relaxed);
            let name = format!(
                ".podway-reset-marker-v1-{}-{sequence}.tmp",
                std::process::id()
            );
            let path = self.path.join(&name);
            let descriptor = match openat(
                &self.directory,
                name.as_str(),
                OFlag::O_CLOEXEC
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_WRONLY,
                Mode::S_IRUSR | Mode::S_IWUSR,
            ) {
                Ok(descriptor) => descriptor,
                Err(Errno::EEXIST) => continue,
                Err(source) => {
                    return Err(runtime_path_error(
                        "create reset marker temporary",
                        path,
                        source.into(),
                    ));
                }
            };
            let file = File::from(descriptor);
            let metadata = file.metadata().map_err(|source| {
                runtime_path_error("inspect reset marker temporary", path.clone(), source)
            })?;
            validate_runtime_file(&path, &metadata, self.current_uid)?;
            return Ok((
                name,
                path,
                file,
                RuntimeFileIdentityV1::from_metadata(&metadata),
            ));
        }
        Err(ValidatedRuntimeDirectoryErrorV1::TemporaryNameExhausted {
            path: self.path.clone(),
        })
    }

    #[cfg(unix)]
    fn unlink_owned_fixed_file(
        &self,
        name: &str,
        expected: RuntimeFileIdentityV1,
    ) -> Result<(), ValidatedRuntimeDirectoryErrorV1> {
        let path = self.path.join(name);
        let metadata =
            match nix::sys::stat::fstatat(&self.directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
                Ok(metadata) => metadata,
                Err(Errno::ENOENT) => {
                    return Err(ValidatedRuntimeDirectoryErrorV1::FileIdentityChanged { path });
                }
                Err(source) => {
                    return Err(runtime_path_error(
                        "reinspect runtime file before unlink",
                        path,
                        source.into(),
                    ));
                }
            };
        if !expected.matches_file_stat(&metadata) {
            return Err(ValidatedRuntimeDirectoryErrorV1::FileIdentityChanged { path });
        }
        unlinkat(&self.directory, name, UnlinkatFlags::NoRemoveDir).map_err(|source| {
            runtime_path_error("unlink runtime file", self.path.join(name), source.into())
        })
    }

    #[cfg(unix)]
    fn sync_directory(&self) -> Result<(), ValidatedRuntimeDirectoryErrorV1> {
        self.directory.sync_all().map_err(|source| {
            runtime_path_error("sync runtime directory", self.path.clone(), source)
        })
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Eq, PartialEq)]
struct RuntimeFileIdentityV1 {
    device: u64,
    inode: u64,
    owner_uid: u32,
}

#[cfg(unix)]
impl RuntimeFileIdentityV1 {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner_uid: metadata.uid(),
        }
    }

    fn matches_file_stat(&self, stat: &nix::sys::stat::FileStat) -> bool {
        self.device == stat.st_dev as u64
            && self.inode == stat.st_ino
            && self.owner_uid == stat.st_uid
    }
}

#[cfg(unix)]
fn open_validated_runtime_directory_unix(
    worktree: &ValidatedWorktreeV1,
) -> Result<ValidatedRuntimeDirectoryV1, ValidatedRuntimeDirectoryErrorV1> {
    let runtime_bytes = worktree
        .containment()
        .runtime_directory()
        .decode_path_bytes()
        .map_err(|_| {
            runtime_path_error(
                "decode Git-validated runtime directory",
                PathBuf::new(),
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Git containment runtime directory cannot be decoded",
                ),
            )
        })?;
    let path = PathBuf::from(OsString::from_vec(runtime_bytes));
    let descriptor = open(
        &path,
        OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .map_err(|source| runtime_path_error("open runtime directory", path.clone(), source.into()))?;
    let directory = File::from(descriptor);
    let metadata = directory
        .metadata()
        .map_err(|source| runtime_path_error("inspect runtime directory", path.clone(), source))?;
    let current_uid = getuid().as_raw();
    validate_runtime_directory(&path, &metadata, current_uid)?;
    Ok(ValidatedRuntimeDirectoryV1 {
        path,
        directory,
        current_uid,
    })
}

#[cfg(unix)]
fn read_bounded_runtime_file(
    file: &mut File,
    maximum: u64,
    path: &Path,
) -> Result<Vec<u8>, ValidatedRuntimeDirectoryErrorV1> {
    let metadata = file
        .metadata()
        .map_err(|source| runtime_path_error("inspect runtime file", path.to_path_buf(), source))?;
    if metadata.len() > maximum {
        return Err(ValidatedRuntimeDirectoryErrorV1::Marker(
            ResetMarkerErrorV1::TooLarge {
                maximum,
                actual: metadata.len(),
            },
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| runtime_path_error("read runtime file", path.to_path_buf(), source))?;
    if bytes.len() as u64 > maximum {
        return Err(ValidatedRuntimeDirectoryErrorV1::Marker(
            ResetMarkerErrorV1::TooLarge {
                maximum,
                actual: bytes.len() as u64,
            },
        ));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn validate_runtime_directory(
    path: &Path,
    metadata: &Metadata,
    current_uid: u32,
) -> Result<(), ValidatedRuntimeDirectoryErrorV1> {
    let violation = if metadata.file_type().is_symlink() {
        Some(RuntimeDirectoryPathViolationV1::Symlink)
    } else if !metadata.is_dir() {
        Some(RuntimeDirectoryPathViolationV1::NotDirectory)
    } else if metadata.uid() != current_uid {
        Some(RuntimeDirectoryPathViolationV1::WrongOwner {
            expected_uid: current_uid,
            actual_uid: metadata.uid(),
        })
    } else {
        let actual_mode = metadata.permissions().mode() & 0o777;
        (actual_mode != PRIVATE_RUNTIME_DIRECTORY_MODE_V1).then_some(
            RuntimeDirectoryPathViolationV1::WrongMode {
                expected_mode: PRIVATE_RUNTIME_DIRECTORY_MODE_V1,
                actual_mode,
            },
        )
    };
    violation.map_or(Ok(()), |violation| {
        Err(ValidatedRuntimeDirectoryErrorV1::UnsafeDirectory {
            path: path.to_path_buf(),
            violation,
        })
    })
}

#[cfg(unix)]
fn validate_runtime_file(
    path: &Path,
    metadata: &Metadata,
    current_uid: u32,
) -> Result<(), ValidatedRuntimeDirectoryErrorV1> {
    let violation = if metadata.file_type().is_symlink() {
        Some(RuntimeDirectoryPathViolationV1::Symlink)
    } else if !metadata.file_type().is_file() {
        Some(RuntimeDirectoryPathViolationV1::NotRegularFile)
    } else if metadata.uid() != current_uid {
        Some(RuntimeDirectoryPathViolationV1::WrongOwner {
            expected_uid: current_uid,
            actual_uid: metadata.uid(),
        })
    } else {
        let actual_mode = metadata.permissions().mode() & 0o777;
        (actual_mode != PRIVATE_RUNTIME_FILE_MODE_V1).then_some(
            RuntimeDirectoryPathViolationV1::WrongMode {
                expected_mode: PRIVATE_RUNTIME_FILE_MODE_V1,
                actual_mode,
            },
        )
    };
    violation.map_or(Ok(()), |violation| {
        Err(ValidatedRuntimeDirectoryErrorV1::UnsafeFile {
            path: path.to_path_buf(),
            violation,
        })
    })
}

fn runtime_path_error(
    operation: &'static str,
    path: PathBuf,
    source: io::Error,
) -> ValidatedRuntimeDirectoryErrorV1 {
    ValidatedRuntimeDirectoryErrorV1::Path {
        operation,
        path,
        source,
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedResetMarkerV1 {
    schema: String,
    operation_id: String,
    idempotency_key: String,
    request_digest: String,
    previous_workspace_uuid: String,
    target_workspace_uuid: String,
    submitted_at_ms: u64,
    #[serde(default)]
    response_request_id: Option<String>,
}

#[derive(serde::Serialize)]
struct SerializableResetMarkerRefV1<'a> {
    schema: &'a str,
    operation_id: &'a str,
    idempotency_key: &'a str,
    request_digest: &'a str,
    previous_workspace_uuid: &'a str,
    target_workspace_uuid: &'a str,
    submitted_at_ms: u64,
}

#[derive(serde::Serialize)]
struct SerializableResetMarkerRefV2<'a> {
    schema: &'a str,
    operation_id: &'a str,
    idempotency_key: &'a str,
    request_digest: &'a str,
    previous_workspace_uuid: &'a str,
    target_workspace_uuid: &'a str,
    submitted_at_ms: u64,
    response_request_id: &'a str,
}

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
        match database_path.parent() {
            Some(parent)
                if matches!(
                    fs::symlink_metadata(parent),
                    Err(error) if error.kind() == io::ErrorKind::NotFound
                ) =>
            {
                return Ok(None);
            }
            _ => {}
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

/// Git-only reset resolution. It never inspects or opens the existing SQLite database, so a
/// missing or corrupt old Store cannot prevent marker recovery from reaching fixed-file deletion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResetWorkspaceResolutionV1 {
    worktree: ValidatedWorktreeV1,
    workspace_root: ValidatedWorkspaceRootV1,
    database_path: PathBuf,
}

impl ResetWorkspaceResolutionV1 {
    fn new(
        worktree: ValidatedWorktreeV1,
        database_path: PathBuf,
    ) -> Result<Self, WorkspaceResolutionErrorV1> {
        Ok(Self {
            workspace_root: store_root_from_worktree(&worktree)?,
            worktree,
            database_path,
        })
    }

    pub fn worktree(&self) -> &ValidatedWorktreeV1 {
        &self.worktree
    }

    pub fn workspace_root(&self) -> &ValidatedWorkspaceRootV1 {
        &self.workspace_root
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn target_identity(&self, target_workspace_uuid: WorkspaceId) -> DurableWorktreeIdentityV1 {
        DurableWorktreeIdentityV1::new(
            self.worktree
                .identity()
                .common_directory_fingerprint()
                .clone(),
            target_workspace_uuid,
            self.worktree
                .identity()
                .worktree_administration_fingerprint()
                .clone(),
        )
    }

    pub fn open_runtime_directory(
        &self,
    ) -> Result<ValidatedRuntimeDirectoryV1, ValidatedRuntimeDirectoryErrorV1> {
        ValidatedRuntimeDirectoryV1::open(&self.worktree)
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

        match expected_workspace_id {
            Some(expected) if expected != stored_identity.workspace_uuid() => {
                return Err(WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch {
                    expected: expected.clone(),
                    actual: stored_identity.workspace_uuid().clone(),
                });
            }
            _ => {}
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

    /// Resolves Git containment for reset recovery without inspecting the old database.
    ///
    /// The target UUID is intentionally not taken from the candidate identity. The marker carries
    /// the target UUID, while the returned worktree supplies only stable Git fingerprints and
    /// descriptor-safe runtime containment.
    pub fn resolve_for_reset(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<ResetWorkspaceResolutionV1, WorkspaceResolutionErrorV1> {
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
        ResetWorkspaceResolutionV1::new(revalidated, revalidated_database_path)
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
/// D01 reset-marker publication crash boundary (`BeforeLinkAndTemporaryCleanup`).
///
/// This lives at module scope so the crash registry can resolve it. It drives the
/// same publication failpoint as
/// `tests::manager_token_retains_marker_publication_cleanup_failure_evidence`
/// and additionally proves the boundary is atomic — an interruption before the
/// destination link publishes no reset marker, and a later publication converges
/// to exactly the interrupted marker.
#[cfg(all(test, unix))]
#[test]
fn d01_reset_marker_publication_interrupted_before_link_publishes_no_marker() {
    use std::os::unix::fs::PermissionsExt;

    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("fixture clock must be after the Unix epoch")
            .as_nanos()
    );
    let path = std::env::temp_dir().join(format!("podway-runtime-marker-d01-{unique}"));
    std::fs::create_dir(&path).expect("fixture runtime directory must be created");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .expect("fixture runtime directory must be private");
    let runtime = ValidatedRuntimeDirectoryV1 {
        path: path.clone(),
        directory: std::fs::File::open(&path).expect("fixture runtime directory must open"),
        current_uid: nix::unistd::getuid().as_raw(),
    };
    let authority = ResetMaintenanceFilesystemTokenV1::issue();
    let marker = ResetMarkerV1::new(
        JobIdV1::new("00000000-0000-4000-8000-0000000052d1")
            .expect("fixture operation ID must be valid"),
        IdempotencyKeyV1::new("workspace-marker-d01")
            .expect("fixture idempotency key must be valid"),
        CanonicalRequestDigestV1::new(format!("sha256:{}", "a".repeat(64)))
            .expect("fixture request digest must be valid"),
        WorkspaceId::new("00000000-0000-4000-8000-0000000052d0")
            .expect("fixture predecessor UUID must be valid"),
        WorkspaceId::new("00000000-0000-4000-8000-0000000052d2")
            .expect("fixture target UUID must be valid"),
        UnixMillis::new(1_700_000_000_123),
    );

    // Interrupt publication at the D01 boundary: the temporary is written and
    // synced, but the destination link never happens. Both the publication
    // failure and the surviving temporary are retained as recovery evidence.
    let error = runtime
        .publish_reset_marker_with_failpoint(
            &authority,
            &marker,
            ResetMarkerPublicationFailpointV1::BeforeLinkAndTemporaryCleanup,
        )
        .expect_err("an interrupted publication must retain its failure evidence");
    assert!(matches!(
        error,
        ResetMarkerPublicationErrorV1::NotPublished(
            ValidatedRuntimeDirectoryErrorV1::PublicationAndTemporaryCleanup { .. }
        )
    ));
    assert!(
        !path.join(RESET_MARKER_FILE_NAME_V1).exists(),
        "an interruption before the link must publish no reset marker"
    );

    // Recovery: a subsequent publication converges to exactly one durable marker.
    runtime
        .publish_reset_marker(&authority, &marker)
        .expect("a retry after the interrupted publication must publish the marker");
    let published = ResetMarkerV1::decode_canonical(
        &std::fs::read(path.join(RESET_MARKER_FILE_NAME_V1))
            .expect("the recovered reset marker must be readable"),
    )
    .expect("the recovered reset marker must decode canonically");
    assert_eq!(
        published.previous_workspace_uuid(),
        marker.previous_workspace_uuid(),
        "recovery must publish the interrupted marker's predecessor"
    );
    assert_eq!(
        published.target_workspace_uuid(),
        marker.target_workspace_uuid(),
        "recovery must publish the interrupted marker's target"
    );

    std::fs::remove_dir_all(&path).expect("fixture runtime directory must be removed");
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use podway_core::{UnixMillis, WorkspaceId};
    use podway_store::{CanonicalRequestDigestV1, IdempotencyKeyV1, JobIdV1};

    use super::{
        ResetMaintenanceFilesystemTokenV1, ResetMarkerPublicationErrorV1,
        ResetMarkerPublicationFailpointV1, ResetMarkerV1, ValidatedRuntimeDirectoryErrorV1,
        ValidatedRuntimeDirectoryV1,
    };

    static RUNTIME_TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn marker() -> ResetMarkerV1 {
        ResetMarkerV1::new(
            JobIdV1::new("00000000-0000-4000-8000-000000005201")
                .expect("fixture operation ID must be valid"),
            IdempotencyKeyV1::new("workspace-marker-unit")
                .expect("fixture idempotency key must be valid"),
            CanonicalRequestDigestV1::new(format!("sha256:{}", "a".repeat(64)))
                .expect("fixture request digest must be valid"),
            WorkspaceId::new("00000000-0000-4000-8000-000000005200")
                .expect("fixture predecessor UUID must be valid"),
            WorkspaceId::new("00000000-0000-4000-8000-000000005202")
                .expect("fixture target UUID must be valid"),
            UnixMillis::new(1_700_000_000_123),
        )
    }

    fn runtime_directory() -> (std::path::PathBuf, ValidatedRuntimeDirectoryV1) {
        use std::os::unix::fs::PermissionsExt;

        let sequence = RUNTIME_TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "podway-runtime-marker-unit-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("fixture runtime directory must be created");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("fixture runtime directory must be private");
        let runtime = ValidatedRuntimeDirectoryV1 {
            path: path.clone(),
            directory: fs::File::open(&path).expect("fixture runtime directory must open"),
            current_uid: nix::unistd::getuid().as_raw(),
        };
        (path, runtime)
    }

    #[test]
    fn manager_token_binds_fixed_database_deletion_to_the_exact_marker() {
        use std::os::unix::fs::PermissionsExt;

        let (path, runtime) = runtime_directory();
        let marker = marker();
        let authority = ResetMaintenanceFilesystemTokenV1::issue();
        let database = path.join("state.sqlite3");
        fs::write(&database, b"old database").expect("fixture database must exist");
        fs::set_permissions(&database, fs::Permissions::from_mode(0o600))
            .expect("fixture database must be private");

        assert!(matches!(
            runtime.remove_reset_database_files(&authority, &marker),
            Err(ValidatedRuntimeDirectoryErrorV1::MarkerMissing)
        ));
        runtime
            .publish_reset_marker(&authority, &marker)
            .expect("manager token must publish the marker");
        runtime
            .remove_reset_database_files(&authority, &marker)
            .expect("matching manager marker must authorize fixed database deletion");
        assert!(!database.exists());
        runtime
            .remove_reset_marker(&authority, &marker)
            .expect("matching manager marker must authorize removal");
        fs::remove_dir(path).expect("fixture runtime directory must be removed");
    }

    #[test]
    fn manager_token_retains_marker_publication_cleanup_failure_evidence() {
        let (path, runtime) = runtime_directory();
        let authority = ResetMaintenanceFilesystemTokenV1::issue();
        let error = runtime
            .publish_reset_marker_with_failpoint(
                &authority,
                &marker(),
                ResetMarkerPublicationFailpointV1::BeforeLinkAndTemporaryCleanup,
            )
            .expect_err("injected publication failure must be retained");
        assert!(matches!(
            error,
            ResetMarkerPublicationErrorV1::NotPublished(
                ValidatedRuntimeDirectoryErrorV1::PublicationAndTemporaryCleanup { .. }
            )
        ));
        fs::remove_dir_all(path).expect("fixture runtime directory must be removed");
    }

    #[test]
    fn marker_publication_failures_preserve_unknown_and_durable_admission_boundaries() {
        for (failpoint, expected_durable) in [
            (
                ResetMarkerPublicationFailpointV1::AfterLinkBeforeDirectorySync,
                false,
            ),
            (
                ResetMarkerPublicationFailpointV1::AfterDirectorySyncBeforeCleanup,
                true,
            ),
        ] {
            let (path, runtime) = runtime_directory();
            let authority = ResetMaintenanceFilesystemTokenV1::issue();
            let expected_marker = marker();
            let error = runtime
                .publish_reset_marker_with_failpoint(&authority, &expected_marker, failpoint)
                .expect_err("injected post-link publication failure must be reported");
            assert!(
                if expected_durable {
                    matches!(error, ResetMarkerPublicationErrorV1::Published(_))
                } else {
                    matches!(error, ResetMarkerPublicationErrorV1::OutcomeUnknown(_))
                },
                "publication phase must retain its strongest admission conclusion"
            );
            assert_eq!(
                runtime.read_reset_marker().unwrap(),
                Some(expected_marker),
                "both post-link boundaries must leave the exact reconciliation marker visible"
            );
            fs::remove_dir_all(path).expect("fixture runtime directory must be removed");
        }
    }
}
