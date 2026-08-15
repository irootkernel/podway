#![forbid(unsafe_code)]

//! Pure domain contracts for Podway.
//!
//! This crate deliberately contains no runtime, filesystem, Git, database, IPC, or service
//! integration. Infrastructure supplies IDs and time; domain code receives them explicitly.

use std::fmt;

use serde::{Deserialize, Serialize};
pub mod aggregate;
pub mod authoring;
pub mod canonical;
pub mod goal_record_v2;
pub mod procedure;
pub mod procedure_v2;
pub mod record_v2;
pub mod session_v2;

pub use aggregate::*;
pub use authoring::*;
pub use canonical::*;
pub use goal_record_v2::*;
pub use procedure::*;
pub use procedure_v2::*;
pub use record_v2::*;
pub use session_v2::*;

pub const MAX_PROCEDURE_IDENTIFIER_BYTES: usize = 64;
/// Maximum UTF-8 byte length of an admitted canonical procedure document.
pub const MAX_PROCEDURE_DOCUMENT_BYTES: usize = 1_048_576;

/// A typed failure raised while constructing or applying a domain contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DomainError {
    EmptyValue {
        field: &'static str,
    },
    ValueTooLong {
        field: &'static str,
        maximum: usize,
        actual: usize,
    },
    InvalidUuid {
        field: &'static str,
    },
    InvalidIdentifier {
        field: &'static str,
    },
    InvalidSha256Digest,
    RevisionOverflow {
        revision: Revision,
    },
    InvalidState {
        reason: &'static str,
    },
    RequiredItemsMissing,
    BlockersPresent,
    BlockerLimitReached {
        maximum_open_blockers: usize,
    },
    ArtifactChanged,
    InvalidTransition {
        command: DomainCommandKind,
        state: SessionLifecycle,
    },
    PreconditionFailed {
        expected: Revision,
        actual: Revision,
    },
    SessionIdentityMismatch {
        expected: SessionId,
        actual: Option<SessionId>,
    },
    AttemptNotCurrent {
        expected: AttemptId,
        actual: Option<AttemptId>,
    },
    ItemNotFound {
        item_id: ItemId,
    },
    BlockerNotCurrent,
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyValue { field } => write!(formatter, "{field} must not be empty"),
            Self::ValueTooLong {
                field,
                maximum,
                actual,
            } => write!(
                formatter,
                "{field} exceeds its maximum of {maximum} bytes (received {actual})"
            ),
            Self::InvalidUuid { field } => write!(formatter, "{field} must be a canonical UUID"),
            Self::InvalidIdentifier { field } => {
                write!(
                    formatter,
                    "{field} must be a lowercase kebab-case identifier"
                )
            }
            Self::InvalidSha256Digest => {
                write!(
                    formatter,
                    "digest must be a lowercase sha256: digest with 64 hex digits"
                )
            }
            Self::RevisionOverflow { revision } => {
                write!(
                    formatter,
                    "revision {} cannot be incremented",
                    revision.get()
                )
            }
            Self::InvalidState { reason } => write!(formatter, "invalid domain state: {reason}"),
            Self::RequiredItemsMissing => {
                formatter.write_str("required items are missing from the active attempt")
            }
            Self::BlockersPresent => {
                formatter.write_str("the active attempt still has open blockers")
            }
            Self::BlockerLimitReached {
                maximum_open_blockers,
            } => write!(
                formatter,
                "the active attempt cannot contain more than {maximum_open_blockers} open blockers"
            ),
            Self::ArtifactChanged => {
                formatter.write_str("the local artifact changed since it was attached")
            }
            Self::InvalidTransition { command, state } => {
                write!(
                    formatter,
                    "{command:?} is not valid while the session is {state:?}"
                )
            }
            Self::PreconditionFailed { expected, actual } => write!(
                formatter,
                "revision precondition failed: expected {}, found {}",
                expected.get(),
                actual.get()
            ),
            Self::SessionIdentityMismatch { expected, actual } => match actual {
                Some(actual) => write!(
                    formatter,
                    "session identity precondition failed: expected {expected}, found {actual}"
                ),
                None => write!(
                    formatter,
                    "session identity precondition failed: expected {expected}, found no session"
                ),
            },
            Self::AttemptNotCurrent { expected, actual } => match actual {
                Some(actual) => write!(
                    formatter,
                    "attempt precondition failed: expected {expected}, found {actual}"
                ),
                None => write!(
                    formatter,
                    "attempt precondition failed: expected {expected}, found no active attempt"
                ),
            },
            Self::ItemNotFound { item_id } => write!(formatter, "item {item_id} was not found"),
            Self::BlockerNotCurrent => {
                write!(formatter, "the blocker is not on the active attempt")
            }
        }
    }
}

impl std::error::Error for DomainError {}

macro_rules! uuid_newtype {
    ($name:ident) => {
        /// An opaque, canonical UUID supplied by infrastructure.
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
                let value = value.into();
                validate_uuid(&value, stringify!($name))?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl TryFrom<String> for $name {
            type Error = DomainError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.into_inner()
            }
        }
    };
}

macro_rules! procedure_identifier_newtype {
    ($name:ident) => {
        /// A stable lowercase kebab-case identifier from a procedure snapshot.
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
                let value = value.into();
                validate_procedure_identifier(&value, stringify!($name))?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl TryFrom<String> for $name {
            type Error = DomainError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.into_inner()
            }
        }
    };
}

uuid_newtype!(WorkspaceId);
uuid_newtype!(SessionId);
uuid_newtype!(AttemptId);
uuid_newtype!(JobId);
uuid_newtype!(ProcedureSnapshotId);
uuid_newtype!(BlockerId);
procedure_identifier_newtype!(ItemId);
procedure_identifier_newtype!(NodeDefinitionId);
procedure_identifier_newtype!(GraphNodeId);
procedure_identifier_newtype!(OptionId);
procedure_identifier_newtype!(CriterionId);

/// An optimistic-concurrency revision. Zero is valid for absent-or-never-written slots.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Result<Self, DomainError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(DomainError::RevisionOverflow { revision: self })
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A UTC Unix timestamp with millisecond precision, supplied by infrastructure.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct UnixMillis(u64);

impl UnixMillis {
    pub const UNIX_EPOCH: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for UnixMillis {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A lowercase SHA-256 digest serialized as `sha256:` followed by 64 hexadecimal digits.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest(String);

impl Sha256Digest {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_sha256_digest(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for Sha256Digest {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<String> for Sha256Digest {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Sha256Digest> for String {
    fn from(value: Sha256Digest) -> Self {
        value.into_inner()
    }
}

/// Persistent session lifecycle states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionLifecycle {
    Running,
    Completed,
    Cancelled,
}

/// Persistent attempt lifecycle states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptLifecycle {
    Active,
    Completed,
    Skipped,
    Abandoned,
}

/// Persistent blocker lifecycle states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockerState {
    Open,
    Resolved,
}

/// Persistent durable-job lifecycle states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// The mutation families recognized by the pure domain layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainCommandKind {
    WorkspaceInitialize,
    WorkspaceResetAll,
    SessionStart,
    SessionStartReplace,
    SessionComplete,
    SessionSkip,
    SessionRetry,
    SessionBlock,
    SessionUnblock,
    SessionCancel,
    SessionReset,
    SessionDecide,
    SessionRework,
    GoalDefine,
    GoalRevise,
    GoalAssessCriterion,
    ItemCheck,
    ItemUncheck,
    ItemSet,
    ItemAdd,
    ItemRemove,
    ItemAttach,
    ItemClear,
    ItemRecordMany,
}

/// A pure domain mutation. Payload interpretation remains inside the domain rather than IPC DTOs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DomainCommand {
    WorkspaceInitialize,
    WorkspaceResetAll,
    SessionStart,
    SessionStartReplace,
    SessionComplete,
    SessionSkip,
    SessionRetry,
    SessionBlock,
    SessionUnblock,
    SessionCancel,
    SessionReset,
    SessionDecide,
    SessionRework,
    GoalDefine,
    GoalRevise,
    GoalAssessCriterion,
    ItemCheck { item_id: ItemId },
    ItemUncheck { item_id: ItemId },
    ItemSet { item_id: ItemId },
    ItemAdd { item_id: ItemId },
    ItemRemove { item_id: ItemId },
    ItemAttach { item_id: ItemId },
    ItemClear { item_id: ItemId },
    ItemRecordMany,
}

impl DomainCommand {
    pub const fn kind(&self) -> DomainCommandKind {
        match self {
            Self::WorkspaceInitialize => DomainCommandKind::WorkspaceInitialize,
            Self::WorkspaceResetAll => DomainCommandKind::WorkspaceResetAll,
            Self::SessionStart => DomainCommandKind::SessionStart,
            Self::SessionStartReplace => DomainCommandKind::SessionStartReplace,
            Self::SessionComplete => DomainCommandKind::SessionComplete,
            Self::SessionSkip => DomainCommandKind::SessionSkip,
            Self::SessionRetry => DomainCommandKind::SessionRetry,
            Self::SessionBlock => DomainCommandKind::SessionBlock,
            Self::SessionUnblock => DomainCommandKind::SessionUnblock,
            Self::SessionCancel => DomainCommandKind::SessionCancel,
            Self::SessionReset => DomainCommandKind::SessionReset,
            Self::SessionDecide => DomainCommandKind::SessionDecide,
            Self::SessionRework => DomainCommandKind::SessionRework,
            Self::GoalDefine => DomainCommandKind::GoalDefine,
            Self::GoalRevise => DomainCommandKind::GoalRevise,
            Self::GoalAssessCriterion => DomainCommandKind::GoalAssessCriterion,
            Self::ItemCheck { .. } => DomainCommandKind::ItemCheck,
            Self::ItemUncheck { .. } => DomainCommandKind::ItemUncheck,
            Self::ItemSet { .. } => DomainCommandKind::ItemSet,
            Self::ItemAdd { .. } => DomainCommandKind::ItemAdd,
            Self::ItemRemove { .. } => DomainCommandKind::ItemRemove,
            Self::ItemAttach { .. } => DomainCommandKind::ItemAttach,
            Self::ItemClear { .. } => DomainCommandKind::ItemClear,
            Self::ItemRecordMany => DomainCommandKind::ItemRecordMany,
        }
    }
}

/// Minimal coherent state needed by infrastructure contracts. Detailed snapshot data remains owned
/// by the procedure/config and storage layers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceState {
    workspace_id: WorkspaceId,
    revision: Revision,
}

impl WorkspaceState {
    pub fn new(workspace_id: WorkspaceId, revision: Revision) -> Self {
        Self {
            workspace_id,
            revision,
        }
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.workspace_id
    }

    pub const fn revision(&self) -> Revision {
        self.revision
    }
}

/// The result of a successful pure-domain command application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DomainResult {
    WorkspaceInitialized {
        workspace_id: WorkspaceId,
        revision: Revision,
    },
    WorkspaceReset {
        workspace_id: WorkspaceId,
        revision: Revision,
    },
    SessionChanged {
        session_id: SessionId,
        revision_before: Revision,
        revision_after: Revision,
        changed: bool,
    },
    ItemChanged {
        session_id: SessionId,
        item_id: ItemId,
        revision_before: Revision,
        revision_after: Revision,
        changed: bool,
    },
    ItemsChanged {
        session_id: SessionId,
        revision_before: Revision,
        revision_after: Revision,
        changed: bool,
    },
}

fn validate_uuid(value: &str, field: &'static str) -> Result<(), DomainError> {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return Err(DomainError::InvalidUuid { field });
    }

    for (index, byte) in bytes.iter().copied().enumerate() {
        let valid = match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'),
        };
        if !valid {
            return Err(DomainError::InvalidUuid { field });
        }
    }

    Ok(())
}

fn validate_procedure_identifier(value: &str, field: &'static str) -> Result<(), DomainError> {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return Err(DomainError::EmptyValue { field });
    }
    if bytes.len() > MAX_PROCEDURE_IDENTIFIER_BYTES {
        return Err(DomainError::ValueTooLong {
            field,
            maximum: MAX_PROCEDURE_IDENTIFIER_BYTES,
            actual: bytes.len(),
        });
    }
    if !bytes[0].is_ascii_lowercase() || bytes.last() == Some(&b'-') {
        return Err(DomainError::InvalidIdentifier { field });
    }

    let mut previous_was_hyphen = false;
    for byte in bytes.iter().copied() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' => previous_was_hyphen = false,
            b'-' if !previous_was_hyphen => previous_was_hyphen = true,
            _ => return Err(DomainError::InvalidIdentifier { field }),
        }
    }

    Ok(())
}

fn validate_sha256_digest(value: &str) -> Result<(), DomainError> {
    let bytes = value.as_bytes();
    if bytes.len() != 71 || !value.starts_with("sha256:") {
        return Err(DomainError::InvalidSha256Digest);
    }
    if bytes[7..]
        .iter()
        .copied()
        .any(|byte| !(byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    {
        return Err(DomainError::InvalidSha256Digest);
    }
    Ok(())
}
