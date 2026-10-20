#![forbid(unsafe_code)]

//! Direct durable state and Procedure v2 persistence for Podway workspaces.

use std::fmt;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

use podway_core::{Revision, SessionId, Sha256Digest, UnixMillis, WorkspaceId};

pub mod direct_store;
mod query_accounting;
mod read_family;
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod v2_delegation;
pub mod v2_goal;
pub mod v2_memory;
pub mod v2_state;

pub use v2_delegation::DelegationReceiptV1;
pub use v2_goal::{AttemptCriterionAssessmentStateV2, CriterionAssessmentStateV2, GoalStateV2};
pub use v2_memory::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, AttemptWorkflowMemoryV2, BlockerStateV2,
    EvidenceLookupErrorV2, EvidenceReadbackV2, EvidenceResolutionStateV2, GraphMutationErrorV2,
    ItemSlotStateV2, MAX_ACTIVE_ITEM_MUTATIONS_V2, MAX_OPEN_BLOCKERS_V2, SelectedEvidenceItemV2,
    WorkflowMemoryStateV2, canonical_recorded_items_json_v2, recorded_items_digest_v2,
    recorded_value_digest_v2, recorded_value_encoded_size_v2,
};
pub use v2_state::{
    AttemptMetadataV2, GraphActionCompletionOutcomeV2, GraphActionSkipOutcomeV2,
    GraphBeginOutcomeV2, GraphBlockOutcomeV2, GraphCancelOutcomeV2,
    GraphCriterionAssessmentOutcomeV2, GraphGoalDefinitionOutcomeV2, GraphGoalRevisionOutcomeV2,
    GraphInitialGoalV2, GraphItemMutationEntryV2, GraphItemMutationOutcomeV2,
    GraphItemsMutationOutcomeV2, GraphNodeCounterV2, GraphNodeSnapshotV2, GraphRetryOutcomeV2,
    GraphSessionStateV2, GraphUnblockOutcomeV2, GraphWorkspaceViewV2,
    MAX_ARCHIVED_SESSION_LIST_WINDOW_V2, ProcedureSnapshotV2, SessionArchiveListPageV2,
    SessionArchiveSummaryV2, admit_procedure_snapshot_v2,
};

pub const MAX_IDEMPOTENCY_KEY_BYTES_V1: usize = 256;
pub const MAX_CANONICAL_EXECUTION_JSON_BYTES_V1: usize = 1_048_576;
pub const MAX_CANONICAL_EXECUTION_JSON_DEPTH_V1: usize = 64;
pub const WORKSPACE_ROOT_TEXT_PREFIX_V1: &str = "podway.unix-path/v1:";
pub type EpochMillisV1 = UnixMillis;
pub type GitIdentityV1 = Sha256Digest;
pub type RevisionV1 = Revision;
pub type WorkspaceUuidV1 = WorkspaceId;

/// A bounded, exact canonical JSON document retained for direct outcomes and markers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalExecutionJsonV1(String);

impl CanonicalExecutionJsonV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, StoreValueErrorV1> {
        Self::admit(value.into()).map(|(canonical, _)| canonical)
    }

    fn admit(value: String) -> Result<(Self, serde_json::Value), StoreValueErrorV1> {
        if value.is_empty() {
            return Err(StoreValueErrorV1::EmptyValue {
                field: "canonical execution JSON",
            });
        }
        if value.len() > MAX_CANONICAL_EXECUTION_JSON_BYTES_V1 {
            return Err(StoreValueErrorV1::ValueTooLong {
                field: "canonical execution JSON",
                maximum_bytes: MAX_CANONICAL_EXECUTION_JSON_BYTES_V1,
            });
        }

        let document: serde_json::Value = serde_json::from_str(&value)
            .map_err(|_| StoreValueErrorV1::InvalidCanonicalExecutionJson)?;
        validate_canonical_execution_depth_v1(&document, 0)?;
        podway_core::verify_canonical_json_value_v1(value.as_bytes(), &document)
            .map_err(|_| StoreValueErrorV1::InvalidCanonicalExecutionJson)?;
        Ok((Self(value), document))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

fn validate_canonical_execution_depth_v1(
    value: &serde_json::Value,
    depth: usize,
) -> Result<(), StoreValueErrorV1> {
    if depth > MAX_CANONICAL_EXECUTION_JSON_DEPTH_V1 {
        return Err(StoreValueErrorV1::CanonicalExecutionJsonDepthExceeded {
            maximum: MAX_CANONICAL_EXECUTION_JSON_DEPTH_V1,
        });
    }

    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                validate_canonical_execution_depth_v1(value, depth + 1)?;
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                validate_canonical_execution_depth_v1(value, depth + 1)?;
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
    Ok(())
}

/// Lossless validated workspace-root evidence stored in workspace_state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedWorkspaceRootV1 {
    bytes: Vec<u8>,
    encoded: String,
}

impl ValidatedWorkspaceRootV1 {
    #[cfg(unix)]
    pub fn from_path(path: &Path) -> Result<Self, StoreValueErrorV1> {
        Self::from_unix_bytes(path.as_os_str().as_bytes().to_vec())
    }

    #[cfg(not(unix))]
    pub fn from_path(_path: &Path) -> Result<Self, StoreValueErrorV1> {
        Err(StoreValueErrorV1::UnsupportedWorkspaceRootPlatform)
    }

    #[cfg(unix)]
    pub fn from_unix_bytes(bytes: Vec<u8>) -> Result<Self, StoreValueErrorV1> {
        if bytes.is_empty() {
            return Err(StoreValueErrorV1::InvalidWorkspaceRootEncoding);
        }
        Ok(Self {
            encoded: encode_lower_hex(&bytes),
            bytes,
        })
    }

    pub fn from_encoded(encoded: impl Into<String>) -> Result<Self, StoreValueErrorV1> {
        let encoded = encoded.into();
        let hex = encoded
            .strip_prefix(WORKSPACE_ROOT_TEXT_PREFIX_V1)
            .ok_or(StoreValueErrorV1::InvalidWorkspaceRootEncoding)?;
        let bytes = decode_lower_hex(hex)?;
        if bytes.is_empty() {
            return Err(StoreValueErrorV1::InvalidWorkspaceRootEncoding);
        }
        if encode_lower_hex(&bytes) != encoded {
            return Err(StoreValueErrorV1::InvalidWorkspaceRootEncoding);
        }
        Ok(Self { bytes, encoded })
    }

    pub fn as_encoded(&self) -> &str {
        &self.encoded
    }

    pub fn unix_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[cfg(unix)]
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(std::ffi::OsString::from_vec(self.bytes.clone()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceBindingV1 {
    identity: DurableWorktreeIdentityV1,
    last_validated_root: ValidatedWorkspaceRootV1,
    schema_version: u32,
}

impl WorkspaceBindingV1 {
    pub fn new(
        identity: DurableWorktreeIdentityV1,
        last_validated_root: ValidatedWorkspaceRootV1,
    ) -> Self {
        Self {
            identity,
            last_validated_root,
            schema_version: direct_store::DIRECT_SCHEMA_GENERATION_V1,
        }
    }

    pub(crate) fn new_with_schema_version(
        identity: DurableWorktreeIdentityV1,
        last_validated_root: ValidatedWorkspaceRootV1,
        schema_version: u32,
    ) -> Self {
        Self {
            identity,
            last_validated_root,
            schema_version,
        }
    }

    pub fn identity(&self) -> &DurableWorktreeIdentityV1 {
        &self.identity
    }

    pub fn last_validated_root(&self) -> &ValidatedWorkspaceRootV1 {
        &self.last_validated_root
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

/// Durable identity that every workspace operation must match before access.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableWorktreeIdentityV1 {
    common_dir_identity: GitIdentityV1,
    workspace_uuid: WorkspaceUuidV1,
    worktree_admin_identity: GitIdentityV1,
}

impl DurableWorktreeIdentityV1 {
    pub fn new(
        common_dir_identity: GitIdentityV1,
        workspace_uuid: WorkspaceUuidV1,
        worktree_admin_identity: GitIdentityV1,
    ) -> Self {
        Self {
            common_dir_identity,
            workspace_uuid,
            worktree_admin_identity,
        }
    }

    pub fn common_dir_identity(&self) -> &GitIdentityV1 {
        &self.common_dir_identity
    }

    pub fn workspace_uuid(&self) -> &WorkspaceUuidV1 {
        &self.workspace_uuid
    }

    pub fn worktree_admin_identity(&self) -> &GitIdentityV1 {
        &self.worktree_admin_identity
    }
}

/// A caller-provided key that binds retries to one canonical request digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdempotencyKeyV1(String);

impl IdempotencyKeyV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, StoreValueErrorV1> {
        let value = value.into();
        validate_non_empty_bounded(&value, MAX_IDEMPOTENCY_KEY_BYTES_V1, "idempotency key")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Invalid bounded values or inconsistent Procedure v2 state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreValueErrorV1 {
    EmptyValue {
        field: &'static str,
    },
    ValueTooLong {
        field: &'static str,
        maximum_bytes: usize,
    },
    IntegerOutOfRange {
        field: &'static str,
    },
    InvalidWorkspaceRootEncoding,
    UnsupportedWorkspaceRootPlatform,
    InvalidCanonicalExecutionJson,
    CanonicalExecutionJsonDepthExceeded {
        maximum: usize,
    },
    InvalidProcedureV2State {
        reason: &'static str,
    },
}

/// Relational state failures. Public envelopes belong to podway-protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreErrorV1 {
    CorruptStateV1 {
        record: StoreRecordKindV1,
    },
    InternalInvariantViolationV1 {
        invariant: StoreInvariantV1,
    },
    InvalidStateV1(StoreValueErrorV1),
    PreconditionConflictV1 {
        expected: Option<RevisionV1>,
        actual: Option<RevisionV1>,
    },
    SessionIdentityConflictV1 {
        expected: Option<SessionId>,
        actual: Option<SessionId>,
    },
    SessionResetNotEligibleV1 {
        lifecycle: podway_core::SessionLifecycle,
        current_terminal_disposition: bool,
    },
    SessionArchiveNotEligibleV1 {
        lifecycle: podway_core::SessionLifecycle,
        current_terminal_disposition: bool,
    },
    SessionArchiveLimitReachedV1 {
        maximum: u32,
    },
    SessionStartStateConflictV1 {
        lifecycle: podway_core::SessionLifecycle,
        current_terminal_disposition: bool,
    },
    SessionArchiveNotFoundV1 {
        session_id: SessionId,
    },
    TerminalDispositionAlreadyRecordedV1 {
        session_revision: RevisionV1,
    },
    StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreRecordKindV1 {
    Workspace,
    Session,
    Snapshot,
    Attempt,
    DelegationReceipt,
    Item,
    Blocker,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreUnavailableReasonV1 {
    Busy,
    Locked,
    StorageIo,
}

#[derive(Clone, Debug)]
pub(crate) enum RusqliteErrorContextV1 {
    Record(StoreRecordKindV1),
}

impl RusqliteErrorContextV1 {
    fn persisted_data_error(self) -> StoreErrorV1 {
        match self {
            Self::Record(record) => StoreErrorV1::CorruptStateV1 { record },
        }
    }
}

/// Maps a rusqlite failure at the caller's persisted-data boundary.
///
/// Busy, lock, and operating-system SQLite errors remain unavailable. SQLite corruption,
/// row-conversion, required-row, and query-shape failures are persisted-data failures and retain
/// the boundary supplied by the caller.
pub(crate) fn map_rusqlite_error_v1(
    error: rusqlite::Error,
    context: RusqliteErrorContextV1,
) -> StoreErrorV1 {
    match error {
        rusqlite::Error::SqliteFailure(failure, _) => match failure.code {
            rusqlite::ErrorCode::DatabaseBusy => StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Busy,
            },
            rusqlite::ErrorCode::DatabaseLocked => StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Locked,
            },
            rusqlite::ErrorCode::DatabaseCorrupt
            | rusqlite::ErrorCode::NotADatabase
            | rusqlite::ErrorCode::InternalMalfunction
            | rusqlite::ErrorCode::SchemaChanged
            | rusqlite::ErrorCode::ConstraintViolation
            | rusqlite::ErrorCode::TypeMismatch
            | rusqlite::ErrorCode::ApiMisuse
            | rusqlite::ErrorCode::ParameterOutOfRange => context.persisted_data_error(),
            _ => StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::StorageIo,
            },
        },
        rusqlite::Error::FromSqlConversionFailure(..)
        | rusqlite::Error::IntegralValueOutOfRange(..)
        | rusqlite::Error::InvalidColumnType(..)
        | rusqlite::Error::Utf8Error(..)
        | rusqlite::Error::QueryReturnedNoRows
        | rusqlite::Error::InvalidColumnIndex(_)
        | rusqlite::Error::InvalidColumnName(_)
        | rusqlite::Error::InvalidParameterName(_)
        | rusqlite::Error::ExecuteReturnedResults
        | rusqlite::Error::StatementChangedRows(_)
        | rusqlite::Error::InvalidQuery => context.persisted_data_error(),
        _ => StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::StorageIo,
        },
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreInvariantV1 {
    ProcedureV2GraphState,
}

impl From<podway_core::ProcedureStateErrorV2> for StoreValueErrorV1 {
    fn from(error: podway_core::ProcedureStateErrorV2) -> Self {
        Self::InvalidProcedureV2State {
            reason: error.reason(),
        }
    }
}
impl From<podway_core::ProcedureStateErrorV2> for StoreErrorV1 {
    fn from(error: podway_core::ProcedureStateErrorV2) -> Self {
        Self::InvalidStateV1(error.into())
    }
}

impl fmt::Display for StoreValueErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyValue { field } => write!(formatter, "{field} must not be empty"),
            Self::ValueTooLong {
                field,
                maximum_bytes,
            } => write!(formatter, "{field} exceeds {maximum_bytes} bytes"),
            Self::IntegerOutOfRange { field } => {
                write!(formatter, "{field} exceeds SQLite's signed integer range")
            }
            Self::InvalidWorkspaceRootEncoding => {
                formatter.write_str("workspace root is not canonical lowercase hexadecimal")
            }
            Self::UnsupportedWorkspaceRootPlatform => {
                formatter.write_str("workspace root byte conversion requires a Unix platform")
            }
            Self::InvalidCanonicalExecutionJson => {
                formatter.write_str("execution JSON is not exact canonical JSON")
            }
            Self::CanonicalExecutionJsonDepthExceeded { maximum } => {
                write!(formatter, "execution JSON nesting exceeds {maximum}")
            }
            Self::InvalidProcedureV2State { reason } => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for StoreValueErrorV1 {}
impl fmt::Display for StoreErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for StoreErrorV1 {}

fn encode_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(WORKSPACE_ROOT_TEXT_PREFIX_V1.len() + bytes.len() * 2);
    encoded.push_str(WORKSPACE_ROOT_TEXT_PREFIX_V1);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_lower_hex(hex: &str) -> Result<Vec<u8>, StoreValueErrorV1> {
    if hex.is_empty() || (hex.len() & 1) != 0 {
        return Err(StoreValueErrorV1::InvalidWorkspaceRootEncoding);
    }

    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        let high = decode_lower_hex_nibble(pair[0])?;
        let low = decode_lower_hex_nibble(pair[1])?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn decode_lower_hex_nibble(byte: u8) -> Result<u8, StoreValueErrorV1> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(StoreValueErrorV1::InvalidWorkspaceRootEncoding),
    }
}
fn validate_non_empty_bounded(
    value: &str,
    maximum_bytes: usize,
    field: &'static str,
) -> Result<(), StoreValueErrorV1> {
    if value.is_empty() {
        return Err(StoreValueErrorV1::EmptyValue { field });
    }
    if value.len() > maximum_bytes {
        return Err(StoreValueErrorV1::ValueTooLong {
            field,
            maximum_bytes,
        });
    }
    Ok(())
}
