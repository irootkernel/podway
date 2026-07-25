//! Versioned internal JSON codecs for durable store/domain values.
//!
//! These envelopes are deliberately not protocol DTOs. They preserve the command data a worker
//! needs after restart and the terminal facts needed for durable idempotency replay.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CanonicalExecutionJsonV1, CanonicalRequestDigestV1, ClaimedExecutionV1, CommandV1, DomainError,
    DomainResult, EpochMillisV1, JobReceiptV1, RevisionAttemptItemPreconditionsV1, RevisionV1,
    TerminalReceiptV1, TerminalResultV1,
};
use podway_core::{
    DomainCommandKind, ItemId, SessionId, SessionLifecycle, Sha256Digest, WorkspaceId,
};

pub const STORE_COMMAND_SCHEMA_V1: &str = "podway.store-command/v1";
pub const STORE_COMMAND_SCHEMA_V2: &str = "podway.store-command/v2";
pub const STORE_TERMINAL_SCHEMA_V0: &str = "podway.store-terminal/v0";
pub const STORE_TERMINAL_SCHEMA_V1: &str = "podway.store-terminal/v1";

/// Fail-closed rejection from a versioned internal store/domain envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreCodecErrorV1 {
    InvalidJson,
    UnsupportedSchema {
        expected: &'static str,
        found: String,
    },
    InvalidValue {
        field: &'static str,
    },
}

impl fmt::Display for StoreCodecErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson => formatter.write_str("invalid internal store JSON"),
            Self::UnsupportedSchema { expected, found } => {
                write!(
                    formatter,
                    "expected internal schema {expected}, found {found}"
                )
            }
            Self::InvalidValue { field } => write!(formatter, "invalid persisted {field}"),
        }
    }
}

impl std::error::Error for StoreCodecErrorV1 {}

/// Serializable mirror of every current [`CommandV1`] variant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersistedDomainCommandV1 {
    WorkspaceInitialize,
    WorkspaceResetAll,
    SessionStart,
    SessionStartReplace,
    SessionComplete,
    SessionSkip,
    SessionRetry,
    SessionReturn,
    SessionBlock,
    SessionUnblock,
    SessionCancel,
    SessionReopen,
    SessionReset,
    ItemCheck { item_id: ItemId },
    ItemUncheck { item_id: ItemId },
    ItemSet { item_id: ItemId },
    ItemAdd { item_id: ItemId },
    ItemRemove { item_id: ItemId },
    ItemAttach { item_id: ItemId },
    ItemClear { item_id: ItemId },
}

impl PersistedDomainCommandV1 {
    pub fn from_command(command: &CommandV1) -> Self {
        match command {
            CommandV1::WorkspaceInitialize => Self::WorkspaceInitialize,
            CommandV1::WorkspaceResetAll => Self::WorkspaceResetAll,
            CommandV1::SessionStart => Self::SessionStart,
            CommandV1::SessionStartReplace => Self::SessionStartReplace,
            CommandV1::SessionComplete => Self::SessionComplete,
            CommandV1::SessionSkip => Self::SessionSkip,
            CommandV1::SessionRetry => Self::SessionRetry,
            CommandV1::SessionReturn => Self::SessionReturn,
            CommandV1::SessionBlock => Self::SessionBlock,
            CommandV1::SessionUnblock => Self::SessionUnblock,
            CommandV1::SessionCancel => Self::SessionCancel,
            CommandV1::SessionReopen => Self::SessionReopen,
            CommandV1::SessionReset => Self::SessionReset,
            CommandV1::ItemCheck { item_id } => Self::ItemCheck {
                item_id: item_id.clone(),
            },
            CommandV1::ItemUncheck { item_id } => Self::ItemUncheck {
                item_id: item_id.clone(),
            },
            CommandV1::ItemSet { item_id } => Self::ItemSet {
                item_id: item_id.clone(),
            },
            CommandV1::ItemAdd { item_id } => Self::ItemAdd {
                item_id: item_id.clone(),
            },
            CommandV1::ItemRemove { item_id } => Self::ItemRemove {
                item_id: item_id.clone(),
            },
            CommandV1::ItemAttach { item_id } => Self::ItemAttach {
                item_id: item_id.clone(),
            },
            CommandV1::ItemClear { item_id } => Self::ItemClear {
                item_id: item_id.clone(),
            },
        }
    }

    fn into_command(self) -> CommandV1 {
        match self {
            Self::WorkspaceInitialize => CommandV1::WorkspaceInitialize,
            Self::WorkspaceResetAll => CommandV1::WorkspaceResetAll,
            Self::SessionStart => CommandV1::SessionStart,
            Self::SessionStartReplace => CommandV1::SessionStartReplace,
            Self::SessionComplete => CommandV1::SessionComplete,
            Self::SessionSkip => CommandV1::SessionSkip,
            Self::SessionRetry => CommandV1::SessionRetry,
            Self::SessionReturn => CommandV1::SessionReturn,
            Self::SessionBlock => CommandV1::SessionBlock,
            Self::SessionUnblock => CommandV1::SessionUnblock,
            Self::SessionCancel => CommandV1::SessionCancel,
            Self::SessionReopen => CommandV1::SessionReopen,
            Self::SessionReset => CommandV1::SessionReset,
            Self::ItemCheck { item_id } => CommandV1::ItemCheck { item_id },
            Self::ItemUncheck { item_id } => CommandV1::ItemUncheck { item_id },
            Self::ItemSet { item_id } => CommandV1::ItemSet { item_id },
            Self::ItemAdd { item_id } => CommandV1::ItemAdd { item_id },
            Self::ItemRemove { item_id } => CommandV1::ItemRemove { item_id },
            Self::ItemAttach { item_id } => CommandV1::ItemAttach { item_id },
            Self::ItemClear { item_id } => CommandV1::ItemClear { item_id },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
struct RequiredOption<T>(Option<T>);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedPreconditionsV1 {
    expected_session_revision: RequiredOption<RevisionV1>,
    expected_attempt_id: RequiredOption<podway_core::AttemptId>,
    expected_item_id: RequiredOption<ItemId>,
    expected_item_revision: RequiredOption<RevisionV1>,
}

impl From<&RevisionAttemptItemPreconditionsV1> for PersistedPreconditionsV1 {
    fn from(preconditions: &RevisionAttemptItemPreconditionsV1) -> Self {
        Self {
            expected_session_revision: RequiredOption(preconditions.expected_session_revision()),
            expected_attempt_id: RequiredOption(preconditions.expected_attempt_id().cloned()),
            expected_item_id: RequiredOption(preconditions.expected_item_id().cloned()),
            expected_item_revision: RequiredOption(preconditions.expected_item_revision()),
        }
    }
}

impl PersistedPreconditionsV1 {
    fn into_preconditions(self) -> Result<RevisionAttemptItemPreconditionsV1, StoreCodecErrorV1> {
        RevisionAttemptItemPreconditionsV1::new(
            self.expected_session_revision.0,
            self.expected_attempt_id.0,
            self.expected_item_id.0,
            self.expected_item_revision.0,
        )
        .map_err(|_| StoreCodecErrorV1::InvalidValue {
            field: "admission preconditions",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandEnvelopeV1 {
    schema: String,
    command: PersistedDomainCommandV1,
    preconditions: PersistedPreconditionsV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandEnvelopeV2 {
    schema: String,
    command: PersistedDomainCommandV1,
    preconditions: PersistedPreconditionsV1,
    canonical_execution_json: String,
}

/// Canonically encodes the command and all admission preconditions required after a claim.
pub fn encode_command_v1(execution: &ClaimedExecutionV1) -> Result<String, StoreCodecErrorV1> {
    if execution.has_complete_execution_document() {
        return canonical_json(&CommandEnvelopeV2 {
            schema: STORE_COMMAND_SCHEMA_V2.to_owned(),
            command: PersistedDomainCommandV1::from_command(execution.command()),
            preconditions: execution.preconditions().into(),
            canonical_execution_json: execution.canonical_execution().as_str().to_owned(),
        });
    }

    canonical_json(&CommandEnvelopeV1 {
        schema: STORE_COMMAND_SCHEMA_V1.to_owned(),
        command: PersistedDomainCommandV1::from_command(execution.command()),
        preconditions: execution.preconditions().into(),
    })
}

/// Decodes a persisted command only when its schema, fields, identifiers, and preconditions verify.
pub fn decode_command_v1(value: &str) -> Result<ClaimedExecutionV1, StoreCodecErrorV1> {
    let document: Value =
        serde_json::from_str(value).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
    let schema = document
        .get("schema")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(StoreCodecErrorV1::InvalidValue {
            field: "command schema",
        })?;

    let execution = match schema.as_str() {
        STORE_COMMAND_SCHEMA_V1 => {
            let envelope: CommandEnvelopeV1 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            ClaimedExecutionV1::new(
                envelope.command.into_command(),
                envelope.preconditions.into_preconditions()?,
            )
        }
        STORE_COMMAND_SCHEMA_V2 => {
            let envelope: CommandEnvelopeV2 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            let canonical_execution =
                CanonicalExecutionJsonV1::new(envelope.canonical_execution_json).map_err(|_| {
                    StoreCodecErrorV1::InvalidValue {
                        field: "canonical execution JSON",
                    }
                })?;
            ClaimedExecutionV1::new_with_canonical_execution(
                envelope.command.into_command(),
                envelope.preconditions.into_preconditions()?,
                canonical_execution,
            )
        }
        found => {
            return Err(StoreCodecErrorV1::UnsupportedSchema {
                expected: STORE_COMMAND_SCHEMA_V2,
                found: found.to_owned(),
            });
        }
    };

    if encode_command_v1(&execution)? != value {
        return Err(StoreCodecErrorV1::InvalidValue {
            field: "canonical command",
        });
    }
    Ok(execution)
}

/// Serializable mirror of every current [`DomainCommandKind`] variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedDomainCommandKindV1 {
    WorkspaceInitialize,
    WorkspaceResetAll,
    SessionStart,
    SessionStartReplace,
    SessionComplete,
    SessionSkip,
    SessionRetry,
    SessionReturn,
    SessionBlock,
    SessionUnblock,
    SessionCancel,
    SessionReopen,
    SessionReset,
    ItemCheck,
    ItemUncheck,
    ItemSet,
    ItemAdd,
    ItemRemove,
    ItemAttach,
    ItemClear,
}

impl From<DomainCommandKind> for PersistedDomainCommandKindV1 {
    fn from(kind: DomainCommandKind) -> Self {
        match kind {
            DomainCommandKind::WorkspaceInitialize => Self::WorkspaceInitialize,
            DomainCommandKind::WorkspaceResetAll => Self::WorkspaceResetAll,
            DomainCommandKind::SessionStart => Self::SessionStart,
            DomainCommandKind::SessionStartReplace => Self::SessionStartReplace,
            DomainCommandKind::SessionComplete => Self::SessionComplete,
            DomainCommandKind::SessionSkip => Self::SessionSkip,
            DomainCommandKind::SessionRetry => Self::SessionRetry,
            DomainCommandKind::SessionReturn => Self::SessionReturn,
            DomainCommandKind::SessionBlock => Self::SessionBlock,
            DomainCommandKind::SessionUnblock => Self::SessionUnblock,
            DomainCommandKind::SessionCancel => Self::SessionCancel,
            DomainCommandKind::SessionReopen => Self::SessionReopen,
            DomainCommandKind::SessionReset => Self::SessionReset,
            DomainCommandKind::ItemCheck => Self::ItemCheck,
            DomainCommandKind::ItemUncheck => Self::ItemUncheck,
            DomainCommandKind::ItemSet => Self::ItemSet,
            DomainCommandKind::ItemAdd => Self::ItemAdd,
            DomainCommandKind::ItemRemove => Self::ItemRemove,
            DomainCommandKind::ItemAttach => Self::ItemAttach,
            DomainCommandKind::ItemClear => Self::ItemClear,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedSessionLifecycleV1 {
    Running,
    Completed,
    Cancelled,
}

impl From<SessionLifecycle> for PersistedSessionLifecycleV1 {
    fn from(lifecycle: SessionLifecycle) -> Self {
        match lifecycle {
            SessionLifecycle::Running => Self::Running,
            SessionLifecycle::Completed => Self::Completed,
            SessionLifecycle::Cancelled => Self::Cancelled,
        }
    }
}

/// Owned persisted representation of every current [`DomainResult`] variant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersistedDomainResultV1 {
    WorkspaceInitialized {
        workspace_id: WorkspaceId,
        revision: RevisionV1,
    },
    WorkspaceReset {
        workspace_id: WorkspaceId,
        revision: RevisionV1,
    },
    SessionChanged {
        session_id: SessionId,
        revision_before: RevisionV1,
        revision_after: RevisionV1,
        changed: bool,
    },
    ItemChanged {
        session_id: SessionId,
        item_id: ItemId,
        revision_before: RevisionV1,
        revision_after: RevisionV1,
        changed: bool,
    },
}

impl PersistedDomainResultV1 {
    pub fn from_domain(result: &DomainResult) -> Self {
        match result {
            DomainResult::WorkspaceInitialized {
                workspace_id,
                revision,
            } => Self::WorkspaceInitialized {
                workspace_id: workspace_id.clone(),
                revision: *revision,
            },
            DomainResult::WorkspaceReset {
                workspace_id,
                revision,
            } => Self::WorkspaceReset {
                workspace_id: workspace_id.clone(),
                revision: *revision,
            },
            DomainResult::SessionChanged {
                session_id,
                revision_before,
                revision_after,
                changed,
            } => Self::SessionChanged {
                session_id: session_id.clone(),
                revision_before: *revision_before,
                revision_after: *revision_after,
                changed: *changed,
            },
            DomainResult::ItemChanged {
                session_id,
                item_id,
                revision_before,
                revision_after,
                changed,
            } => Self::ItemChanged {
                session_id: session_id.clone(),
                item_id: item_id.clone(),
                revision_before: *revision_before,
                revision_after: *revision_after,
                changed: *changed,
            },
        }
    }
}

/// Owned persisted representation of every current [`DomainError`] variant.
///
/// The domain type uses several `&'static str` members, so decoding remains owned rather than
/// leaking arbitrary persisted strings to reconstruct a borrowed domain error.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersistedDomainErrorV1 {
    EmptyValue {
        field: String,
    },
    ValueTooLong {
        field: String,
        maximum: u64,
        actual: u64,
    },
    InvalidUuid {
        field: String,
    },
    InvalidIdentifier {
        field: String,
    },
    InvalidSha256Digest,
    RevisionOverflow {
        revision: RevisionV1,
    },
    InvalidState {
        reason: String,
    },
    RequiredItemsMissing,
    BlockersPresent,
    ArtifactChanged,
    InvalidTransition {
        command: PersistedDomainCommandKindV1,
        state: PersistedSessionLifecycleV1,
    },
    PreconditionFailed {
        expected: RevisionV1,
        actual: RevisionV1,
    },
    SessionIdentityMismatch {
        expected: podway_core::SessionId,
        actual: Option<podway_core::SessionId>,
    },
    AttemptNotCurrent {
        expected: podway_core::AttemptId,
        actual: Option<podway_core::AttemptId>,
    },
    ItemNotFound {
        item_id: ItemId,
    },
    BlockerNotCurrent,
}

impl PersistedDomainErrorV1 {
    pub fn from_domain(error: &DomainError) -> Self {
        match error {
            DomainError::EmptyValue { field } => Self::EmptyValue {
                field: (*field).to_owned(),
            },
            DomainError::ValueTooLong {
                field,
                maximum,
                actual,
            } => Self::ValueTooLong {
                field: (*field).to_owned(),
                maximum: u64::try_from(*maximum).expect("usize fits in u64"),
                actual: u64::try_from(*actual).expect("usize fits in u64"),
            },
            DomainError::InvalidUuid { field } => Self::InvalidUuid {
                field: (*field).to_owned(),
            },
            DomainError::InvalidIdentifier { field } => Self::InvalidIdentifier {
                field: (*field).to_owned(),
            },
            DomainError::InvalidSha256Digest => Self::InvalidSha256Digest,
            DomainError::RevisionOverflow { revision } => Self::RevisionOverflow {
                revision: *revision,
            },
            DomainError::InvalidState { reason } => Self::InvalidState {
                reason: (*reason).to_owned(),
            },
            DomainError::RequiredItemsMissing => Self::RequiredItemsMissing,
            DomainError::BlockersPresent => Self::BlockersPresent,
            DomainError::ArtifactChanged => Self::ArtifactChanged,
            DomainError::InvalidTransition { command, state } => Self::InvalidTransition {
                command: (*command).into(),
                state: (*state).into(),
            },
            DomainError::PreconditionFailed { expected, actual } => Self::PreconditionFailed {
                expected: *expected,
                actual: *actual,
            },
            DomainError::SessionIdentityMismatch { expected, actual } => {
                Self::SessionIdentityMismatch {
                    expected: expected.clone(),
                    actual: actual.clone(),
                }
            }
            DomainError::AttemptNotCurrent { expected, actual } => Self::AttemptNotCurrent {
                expected: expected.clone(),
                actual: actual.clone(),
            },
            DomainError::ItemNotFound { item_id } => Self::ItemNotFound {
                item_id: item_id.clone(),
            },
            DomainError::BlockerNotCurrent => Self::BlockerNotCurrent,
        }
    }
}

/// Persisted terminal result, including queued cancellation which is not a core `DomainResult`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PersistedTerminalResultV1 {
    Success(PersistedDomainResultV1),
    Failure(PersistedDomainErrorV1),
    Cancelled,
}

impl PersistedTerminalResultV1 {
    pub fn from_terminal_result(result: &TerminalResultV1) -> Self {
        match result {
            TerminalResultV1::Success(result) => {
                Self::Success(PersistedDomainResultV1::from_domain(result))
            }
            TerminalResultV1::Failure(error) => {
                Self::Failure(PersistedDomainErrorV1::from_domain(error))
            }
        }
    }
}

/// Terminal durable states preserved for exact terminal-response replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedTerminalJobStateV1 {
    Succeeded,
    Failed,
    Cancelled,
}

/// Bounded immutable job facts captured when a terminal response is committed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedTerminalJobProjectionV1 {
    state: PersistedTerminalJobStateV1,
    submitted_at: EpochMillisV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_at: Option<EpochMillisV1>,
    finished_at: EpochMillisV1,
}

impl PersistedTerminalJobProjectionV1 {
    pub fn new(
        state: PersistedTerminalJobStateV1,
        submitted_at: EpochMillisV1,
        claimed_at: Option<EpochMillisV1>,
        finished_at: EpochMillisV1,
    ) -> Result<Self, StoreCodecErrorV1> {
        let projection = Self {
            state,
            submitted_at,
            claimed_at,
            finished_at,
        };
        projection.validate()?;
        Ok(projection)
    }

    pub fn state(&self) -> PersistedTerminalJobStateV1 {
        self.state
    }

    pub fn submitted_at(&self) -> EpochMillisV1 {
        self.submitted_at
    }

    pub fn claimed_at(&self) -> Option<EpochMillisV1> {
        self.claimed_at
    }

    pub fn finished_at(&self) -> EpochMillisV1 {
        self.finished_at
    }

    fn validate(&self) -> Result<(), StoreCodecErrorV1> {
        let cancelled_after_claim =
            self.state == PersistedTerminalJobStateV1::Cancelled && self.claimed_at.is_some();
        if cancelled_after_claim
            || self.submitted_at > self.finished_at
            || self.claimed_at.is_some_and(|claimed_at| {
                claimed_at < self.submitted_at || claimed_at > self.finished_at
            })
        {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal job timestamps",
            });
        }
        Ok(())
    }
}

/// Bounded immutable session facts captured from the post-transition aggregate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedTerminalSessionProjectionV1 {
    session_id: SessionId,
    task_title: String,
    lifecycle: PersistedSessionLifecycleV1,
    revision_before: RevisionV1,
    revision_after: RevisionV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    procedure_digest: Option<Sha256Digest>,
}

impl PersistedTerminalSessionProjectionV1 {
    pub fn new(
        session_id: SessionId,
        task_title: String,
        lifecycle: PersistedSessionLifecycleV1,
        revision_before: RevisionV1,
        revision_after: RevisionV1,
    ) -> Result<Self, StoreCodecErrorV1> {
        let projection = Self {
            session_id,
            task_title,
            lifecycle,
            revision_before,
            revision_after,
            procedure_digest: None,
        };
        projection.validate()?;
        Ok(projection)
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn task_title(&self) -> &str {
        &self.task_title
    }

    pub fn lifecycle(&self) -> PersistedSessionLifecycleV1 {
        self.lifecycle
    }

    pub fn revision_before(&self) -> RevisionV1 {
        self.revision_before
    }

    pub fn revision_after(&self) -> RevisionV1 {
        self.revision_after
    }

    pub fn procedure_digest(&self) -> Option<&Sha256Digest> {
        self.procedure_digest.as_ref()
    }

    pub fn with_procedure_digest(mut self, procedure_digest: Sha256Digest) -> Self {
        self.procedure_digest = Some(procedure_digest);
        self
    }

    fn validate(&self) -> Result<(), StoreCodecErrorV1> {
        if self.task_title.trim().is_empty() || self.task_title.chars().count() > 500 {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal session title",
            });
        }
        if self.revision_after < self.revision_before
            && !(self.revision_after == RevisionV1::new(1)
                && self.revision_before > RevisionV1::new(1))
        {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal session revisions",
            });
        }
        Ok(())
    }
}

/// A fully validated terminal receipt decoded from the internal terminal envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedTerminalReceiptV1 {
    job: JobReceiptV1,
    result: PersistedTerminalResultV1,
    job_projection: Option<PersistedTerminalJobProjectionV1>,
    session_projection: Option<PersistedTerminalSessionProjectionV1>,
}

impl PersistedTerminalReceiptV1 {
    /// Builds a legacy v0 terminal receipt without immutable replay projections.
    pub fn new(job: JobReceiptV1, result: PersistedTerminalResultV1) -> Self {
        Self {
            job,
            result,
            job_projection: None,
            session_projection: None,
        }
    }

    pub fn new_with_projections(
        job: JobReceiptV1,
        result: PersistedTerminalResultV1,
        job_projection: PersistedTerminalJobProjectionV1,
        session_projection: Option<PersistedTerminalSessionProjectionV1>,
    ) -> Result<Self, StoreCodecErrorV1> {
        let receipt = Self {
            job,
            result,
            job_projection: Some(job_projection),
            session_projection,
        };
        receipt.validate_v1_projections()?;
        Ok(receipt)
    }

    pub fn cancelled(job: JobReceiptV1) -> Self {
        Self::new(job, PersistedTerminalResultV1::Cancelled)
    }

    pub fn from_terminal_receipt(receipt: &TerminalReceiptV1) -> Self {
        Self::new(
            receipt.job().clone(),
            PersistedTerminalResultV1::from_terminal_result(receipt.result()),
        )
    }

    pub fn job(&self) -> &JobReceiptV1 {
        &self.job
    }

    pub fn result(&self) -> &PersistedTerminalResultV1 {
        &self.result
    }

    pub fn job_projection(&self) -> Option<&PersistedTerminalJobProjectionV1> {
        self.job_projection.as_ref()
    }

    pub fn session_projection(&self) -> Option<&PersistedTerminalSessionProjectionV1> {
        self.session_projection.as_ref()
    }

    fn from_v1_envelope(
        job: JobReceiptV1,
        result: PersistedTerminalResultV1,
        job_projection: PersistedTerminalJobProjectionV1,
        session_projection: Option<PersistedTerminalSessionProjectionV1>,
    ) -> Result<Self, StoreCodecErrorV1> {
        Self::new_with_projections(job, result, job_projection, session_projection)
    }

    fn validate_legacy_projections(&self) -> Result<(), StoreCodecErrorV1> {
        if self.job_projection.is_some() || self.session_projection.is_some() {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal replay projections",
            });
        }
        Ok(())
    }

    fn validate_v1_projections(&self) -> Result<(), StoreCodecErrorV1> {
        let job_projection =
            self.job_projection
                .as_ref()
                .ok_or(StoreCodecErrorV1::InvalidValue {
                    field: "terminal replay projections",
                })?;
        job_projection.validate()?;
        let state_matches_result = matches!(
            (job_projection.state(), &self.result),
            (
                PersistedTerminalJobStateV1::Succeeded,
                PersistedTerminalResultV1::Success(_)
            ) | (
                PersistedTerminalJobStateV1::Failed,
                PersistedTerminalResultV1::Failure(_)
            ) | (
                PersistedTerminalJobStateV1::Cancelled,
                PersistedTerminalResultV1::Cancelled
            )
        );
        if !state_matches_result {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal job state",
            });
        }

        match (&self.result, &self.session_projection) {
            (
                PersistedTerminalResultV1::Success(
                    PersistedDomainResultV1::SessionChanged {
                        session_id,
                        revision_before,
                        revision_after,
                        changed,
                        ..
                    }
                    | PersistedDomainResultV1::ItemChanged {
                        session_id,
                        revision_before,
                        revision_after,
                        changed,
                        ..
                    },
                ),
                Some(session_projection),
            ) => {
                let fresh_replacement = matches!(
                    &self.result,
                    PersistedTerminalResultV1::Success(
                        PersistedDomainResultV1::SessionChanged { .. }
                    )
                ) && *changed
                    && *revision_after == RevisionV1::new(1)
                    && *revision_before > RevisionV1::ZERO;
                if *changed != (*revision_before != *revision_after) && !fresh_replacement {
                    return Err(StoreCodecErrorV1::InvalidValue {
                        field: "terminal session projection",
                    });
                }
                session_projection.validate()?;
                if *revision_after == RevisionV1::ZERO {
                    return Err(StoreCodecErrorV1::InvalidValue {
                        field: "terminal session projection",
                    });
                }
                if session_projection.session_id() != session_id
                    || session_projection.revision_before() != *revision_before
                    || session_projection.revision_after() != *revision_after
                {
                    return Err(StoreCodecErrorV1::InvalidValue {
                        field: "terminal session projection",
                    });
                }
            }
            (
                PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                    revision_before,
                    revision_after,
                    changed,
                    ..
                }),
                None,
            ) if *revision_before != RevisionV1::ZERO
                && *revision_after == RevisionV1::ZERO
                && *changed => {}
            (
                PersistedTerminalResultV1::Success(
                    PersistedDomainResultV1::SessionChanged { .. }
                    | PersistedDomainResultV1::ItemChanged { .. },
                ),
                None,
            )
            | (
                PersistedTerminalResultV1::Success(
                    PersistedDomainResultV1::WorkspaceInitialized { .. }
                    | PersistedDomainResultV1::WorkspaceReset { .. },
                )
                | PersistedTerminalResultV1::Failure(_)
                | PersistedTerminalResultV1::Cancelled,
                Some(_),
            ) => {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "terminal session projection",
                });
            }
            (
                PersistedTerminalResultV1::Success(
                    PersistedDomainResultV1::WorkspaceInitialized { .. }
                    | PersistedDomainResultV1::WorkspaceReset { .. },
                )
                | PersistedTerminalResultV1::Failure(_)
                | PersistedTerminalResultV1::Cancelled,
                None,
            ) => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedJobReceiptV1 {
    identity_sequence: u64,
    job_id: podway_core::JobId,
    request_digest: CanonicalRequestDigestV1,
}

impl From<&JobReceiptV1> for PersistedJobReceiptV1 {
    fn from(receipt: &JobReceiptV1) -> Self {
        Self {
            identity_sequence: receipt.identity_sequence(),
            job_id: receipt.job_id().clone(),
            request_digest: receipt.request_digest().clone(),
        }
    }
}

impl From<PersistedJobReceiptV1> for JobReceiptV1 {
    fn from(receipt: PersistedJobReceiptV1) -> Self {
        JobReceiptV1::new(
            receipt.identity_sequence,
            receipt.job_id,
            receipt.request_digest,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalEnvelopeV0 {
    schema: String,
    job: PersistedJobReceiptV1,
    result: PersistedTerminalResultV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalEnvelopeV1 {
    schema: String,
    job: PersistedJobReceiptV1,
    job_projection: PersistedTerminalJobProjectionV1,
    result: PersistedTerminalResultV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_projection: Option<PersistedTerminalSessionProjectionV1>,
}

/// Core terminal receipts lack the immutable projections required by terminal schema v1, so they
/// canonically encode as legacy schema v0.
pub fn encode_terminal_receipt_v1(
    receipt: &TerminalReceiptV1,
) -> Result<String, StoreCodecErrorV1> {
    encode_persisted_terminal_receipt_v1(&PersistedTerminalReceiptV1::from_terminal_receipt(
        receipt,
    ))
}

/// Canonically encodes a persisted terminal receipt, including queued cancellation.
pub fn encode_persisted_terminal_receipt_v1(
    receipt: &PersistedTerminalReceiptV1,
) -> Result<String, StoreCodecErrorV1> {
    if receipt.job().identity_sequence() == 0 {
        return Err(StoreCodecErrorV1::InvalidValue {
            field: "identity sequence",
        });
    }

    match receipt.job_projection() {
        Some(job_projection) => {
            receipt.validate_v1_projections()?;
            canonical_json(&TerminalEnvelopeV1 {
                schema: STORE_TERMINAL_SCHEMA_V1.to_owned(),
                job: receipt.job().into(),
                job_projection: job_projection.clone(),
                result: receipt.result().clone(),
                session_projection: receipt.session_projection().cloned(),
            })
        }
        None => {
            receipt.validate_legacy_projections()?;
            canonical_json(&TerminalEnvelopeV0 {
                schema: STORE_TERMINAL_SCHEMA_V0.to_owned(),
                job: receipt.job().into(),
                result: receipt.result().clone(),
            })
        }
    }
}

/// Decodes terminal facts with strict schema/field validation and checked core identifiers/digests.
pub fn decode_terminal_receipt_v1(
    value: &str,
) -> Result<PersistedTerminalReceiptV1, StoreCodecErrorV1> {
    let document: Value =
        serde_json::from_str(value).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
    let schema = document
        .get("schema")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(StoreCodecErrorV1::InvalidValue {
            field: "terminal schema",
        })?;
    let receipt = match schema.as_str() {
        STORE_TERMINAL_SCHEMA_V0 => {
            let envelope: TerminalEnvelopeV0 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            if envelope.job.identity_sequence == 0 {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "identity sequence",
                });
            }
            PersistedTerminalReceiptV1::new(envelope.job.into(), envelope.result)
        }
        STORE_TERMINAL_SCHEMA_V1 => {
            let envelope: TerminalEnvelopeV1 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            if envelope.job.identity_sequence == 0 {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "identity sequence",
                });
            }
            PersistedTerminalReceiptV1::from_v1_envelope(
                envelope.job.into(),
                envelope.result,
                envelope.job_projection,
                envelope.session_projection,
            )?
        }
        found => {
            return Err(StoreCodecErrorV1::UnsupportedSchema {
                expected: STORE_TERMINAL_SCHEMA_V1,
                found: found.to_owned(),
            });
        }
    };
    if encode_persisted_terminal_receipt_v1(&receipt)? != value {
        return Err(StoreCodecErrorV1::InvalidValue {
            field: "canonical terminal receipt",
        });
    }
    Ok(receipt)
}
pub(crate) fn validate_terminal_result_for_command_v1(
    command: &CommandV1,
    result: &TerminalResultV1,
) -> Result<(), StoreCodecErrorV1> {
    validate_persisted_terminal_result_for_command_v1(
        command,
        &PersistedTerminalResultV1::from_terminal_result(result),
    )
}

pub(crate) fn validate_persisted_terminal_result_for_command_v1(
    command: &CommandV1,
    result: &PersistedTerminalResultV1,
) -> Result<(), StoreCodecErrorV1> {
    match result {
        PersistedTerminalResultV1::Success(result) => {
            validate_success_result_for_command_v1(command, result)
        }
        PersistedTerminalResultV1::Failure(error) => {
            validate_failure_result_for_command_v1(command, error)
        }
        PersistedTerminalResultV1::Cancelled => Ok(()),
    }
}

fn validate_success_result_for_command_v1(
    command: &CommandV1,
    result: &PersistedDomainResultV1,
) -> Result<(), StoreCodecErrorV1> {
    let revisions_are_possible =
        |before: RevisionV1, after: RevisionV1, changed: bool| changed == (before != after);
    let monotonic_revisions_are_possible =
        |before: RevisionV1, after: RevisionV1, changed: bool| {
            revisions_are_possible(before, after, changed) && before <= after
        };
    let fresh_replacement_revisions_are_possible =
        |before: RevisionV1, after: RevisionV1, changed: bool| {
            changed && after == RevisionV1::new(1) && before > RevisionV1::ZERO
        };
    let compatible = match (command, result) {
        (CommandV1::WorkspaceInitialize, PersistedDomainResultV1::WorkspaceInitialized { .. })
        | (CommandV1::WorkspaceResetAll, PersistedDomainResultV1::WorkspaceReset { .. }) => true,
        (
            CommandV1::SessionStartReplace,
            PersistedDomainResultV1::SessionChanged {
                revision_before,
                revision_after,
                changed,
                ..
            },
        ) => {
            monotonic_revisions_are_possible(*revision_before, *revision_after, *changed)
                || fresh_replacement_revisions_are_possible(
                    *revision_before,
                    *revision_after,
                    *changed,
                )
        }
        (
            CommandV1::SessionReset,
            PersistedDomainResultV1::SessionChanged {
                revision_before,
                revision_after,
                changed,
                ..
            },
        ) => {
            revisions_are_possible(*revision_before, *revision_after, *changed)
                && *revision_after == RevisionV1::ZERO
        }
        (
            CommandV1::SessionStart
            | CommandV1::SessionComplete
            | CommandV1::SessionSkip
            | CommandV1::SessionRetry
            | CommandV1::SessionReturn
            | CommandV1::SessionBlock
            | CommandV1::SessionUnblock
            | CommandV1::SessionCancel
            | CommandV1::SessionReopen,
            PersistedDomainResultV1::SessionChanged {
                revision_before,
                revision_after,
                changed,
                ..
            },
        ) => monotonic_revisions_are_possible(*revision_before, *revision_after, *changed),
        (
            CommandV1::ItemCheck { item_id }
            | CommandV1::ItemUncheck { item_id }
            | CommandV1::ItemSet { item_id }
            | CommandV1::ItemAdd { item_id }
            | CommandV1::ItemRemove { item_id }
            | CommandV1::ItemAttach { item_id }
            | CommandV1::ItemClear { item_id },
            PersistedDomainResultV1::ItemChanged {
                item_id: result_item_id,
                revision_before,
                revision_after,
                changed,
                ..
            },
        ) => {
            item_id == result_item_id
                && monotonic_revisions_are_possible(*revision_before, *revision_after, *changed)
        }
        _ => false,
    };
    compatible
        .then_some(())
        .ok_or(StoreCodecErrorV1::InvalidValue {
            field: "command-compatible terminal result",
        })
}

fn validate_failure_result_for_command_v1(
    command: &CommandV1,
    error: &PersistedDomainErrorV1,
) -> Result<(), StoreCodecErrorV1> {
    let compatible = match error {
        PersistedDomainErrorV1::InvalidTransition {
            command: failed_command,
            ..
        } => *failed_command == command.kind().into(),
        PersistedDomainErrorV1::ItemNotFound { item_id } => match command {
            CommandV1::ItemCheck {
                item_id: command_item_id,
            }
            | CommandV1::ItemUncheck {
                item_id: command_item_id,
            }
            | CommandV1::ItemSet {
                item_id: command_item_id,
            }
            | CommandV1::ItemAdd {
                item_id: command_item_id,
            }
            | CommandV1::ItemRemove {
                item_id: command_item_id,
            }
            | CommandV1::ItemAttach {
                item_id: command_item_id,
            }
            | CommandV1::ItemClear {
                item_id: command_item_id,
            } => item_id == command_item_id,
            _ => false,
        },
        _ => true,
    };
    compatible
        .then_some(())
        .ok_or(StoreCodecErrorV1::InvalidValue {
            field: "command-compatible terminal failure",
        })
}

fn canonical_json<T: Serialize>(value: &T) -> Result<String, StoreCodecErrorV1> {
    let value = serde_json::to_value(value).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
    serde_json::to_string(&canonicalize_json(value)).map_err(|_| StoreCodecErrorV1::InvalidJson)
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let sorted: BTreeMap<_, _> = values.into_iter().collect();
            let mut canonical = serde_json::Map::new();
            for (key, value) in sorted {
                canonical.insert(key, canonicalize_json(value));
            }
            Value::Object(canonical)
        }
        primitive => primitive,
    }
}
