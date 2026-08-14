//! Versioned internal JSON codecs for durable store/domain values.
//!
//! These envelopes are deliberately not protocol DTOs. They preserve the command data a worker
//! needs after restart and the terminal facts needed for durable idempotency replay.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CanonicalExecutionJsonV1, CanonicalRequestDigestV1, ClaimedExecutionV1, CommandV1, DomainError,
    DomainResult, EpochMillisV1, JobReceiptV1, RevisionAttemptItemPreconditionsV1, RevisionV1,
    TerminalReceiptV1, TerminalResultV1,
};
use podway_core::{
    ActorAttributionV2, AttemptId, AttemptNumberV2, BlockerId, CriterionAssessmentReasonV2,
    CriterionAssessmentResultV2, CriterionCitationV2, CriterionId, CriterionStatusV2,
    DomainCommandKind, GoalOutcome, GraphNodeId, ItemId, NodeDefinitionId, OptionId,
    ProcedureSnapshotId, ReasonV2, SessionId, SessionLifecycle, Sha256Digest, WorkspaceId,
};

pub const STORE_COMMAND_SCHEMA_V1: &str = "podway.store-command/v1";
pub const STORE_COMMAND_SCHEMA_V2: &str = "podway.store-command/v2";
pub const STORE_GRAPH_COMMAND_SCHEMA_V1: &str = "podway.store-graph-command/v1";
pub const STORE_TERMINAL_SCHEMA_V0: &str = "podway.store-terminal/v0";
pub const STORE_TERMINAL_SCHEMA_V1: &str = "podway.store-terminal/v1";
pub const STORE_TERMINAL_SCHEMA_V2: &str = "podway.store-terminal/v2";
pub const STORE_TERMINAL_SCHEMA_V3: &str = "podway.store-terminal/v3";
pub const STORE_TERMINAL_SCHEMA_V4: &str = "podway.store-terminal/v4";
pub const STORE_TERMINAL_SCHEMA_V5: &str = "podway.store-terminal/v5";
pub const STORE_GRAPH_TERMINAL_SCHEMA_V1: &str = "podway.store-graph-terminal/v1";
pub const STORE_GRAPH_TERMINAL_SCHEMA_V2: &str = "podway.store-graph-terminal/v2";
pub const STORE_GRAPH_TERMINAL_SCHEMA_V3: &str = "podway.store-graph-terminal/v3";

/// Minimal immutable response correlation retained independently of semantic request identity.
///
/// This is deliberately transport-neutral storage data. The daemon validates it against the
/// public protocol before admission and reconstructs the public envelope after terminal commit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedResponseContextV1 {
    request_id: String,
    command: String,
    workspace_uuid: WorkspaceId,
    workspace_root: String,
    workspace_sequence: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    freeze_public_terminal_envelope: bool,
}

impl PersistedResponseContextV1 {
    pub fn new(
        request_id: impl Into<String>,
        command: impl Into<String>,
        workspace_uuid: WorkspaceId,
        workspace_root: impl Into<String>,
        workspace_sequence: u64,
    ) -> Result<Self, StoreCodecErrorV1> {
        let context = Self {
            request_id: request_id.into(),
            command: command.into(),
            workspace_uuid,
            workspace_root: workspace_root.into(),
            workspace_sequence,
            freeze_public_terminal_envelope: false,
        };
        context.validate()?;
        Ok(context)
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn workspace_uuid(&self) -> &WorkspaceId {
        &self.workspace_uuid
    }

    pub fn workspace_root(&self) -> &str {
        &self.workspace_root
    }

    pub fn workspace_sequence(&self) -> u64 {
        self.workspace_sequence
    }

    /// Marks production requests whose exact public terminal envelope must be sealed atomically.
    pub fn with_frozen_public_terminal_envelope(mut self) -> Self {
        self.freeze_public_terminal_envelope = true;
        self
    }

    pub(crate) fn freezes_public_terminal_envelope(&self) -> bool {
        self.freeze_public_terminal_envelope
    }

    /// Binds response metadata to the sequence allocated by the admission transaction.
    pub fn with_workspace_sequence(mut self, workspace_sequence: u64) -> Self {
        self.workspace_sequence = workspace_sequence;
        self
    }

    fn validate(&self) -> Result<(), StoreCodecErrorV1> {
        if self.request_id.is_empty() || self.request_id.len() > 128 {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "response request ID",
            });
        }
        if self.command.is_empty() || self.command.len() > 128 {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "response command",
            });
        }
        if self.workspace_root.is_empty() || self.workspace_root.len() > 16 * 1024 {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "response workspace root",
            });
        }
        Ok(())
    }
}

pub fn encode_response_context_v1(
    context: &PersistedResponseContextV1,
) -> Result<String, StoreCodecErrorV1> {
    context.validate()?;
    canonical_json(context)
}

pub fn decode_response_context_v1(
    value: &str,
) -> Result<PersistedResponseContextV1, StoreCodecErrorV1> {
    let context: PersistedResponseContextV1 =
        serde_json::from_str(value).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
    context.validate()?;
    if encode_response_context_v1(&context)? != value {
        return Err(StoreCodecErrorV1::InvalidValue {
            field: "canonical response context",
        });
    }
    Ok(context)
}

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
            CommandV1::SessionBlock => Self::SessionBlock,
            CommandV1::SessionUnblock => Self::SessionUnblock,
            CommandV1::SessionCancel => Self::SessionCancel,
            CommandV1::SessionReset => Self::SessionReset,
            CommandV1::SessionDecide => Self::SessionDecide,
            CommandV1::SessionRework => Self::SessionRework,
            CommandV1::GoalDefine => Self::GoalDefine,
            CommandV1::GoalRevise => Self::GoalRevise,
            CommandV1::GoalAssessCriterion => Self::GoalAssessCriterion,
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
            Self::SessionBlock => CommandV1::SessionBlock,
            Self::SessionUnblock => CommandV1::SessionUnblock,
            Self::SessionCancel => CommandV1::SessionCancel,
            Self::SessionReset => CommandV1::SessionReset,
            Self::SessionDecide => CommandV1::SessionDecide,
            Self::SessionRework => CommandV1::SessionRework,
            Self::GoalDefine => CommandV1::GoalDefine,
            Self::GoalRevise => CommandV1::GoalRevise,
            Self::GoalAssessCriterion => CommandV1::GoalAssessCriterion,
            Self::ItemCheck { item_id } => CommandV1::ItemCheck { item_id },
            Self::ItemUncheck { item_id } => CommandV1::ItemUncheck { item_id },
            Self::ItemSet { item_id } => CommandV1::ItemSet { item_id },
            Self::ItemAdd { item_id } => CommandV1::ItemAdd { item_id },
            Self::ItemRemove { item_id } => CommandV1::ItemRemove { item_id },
            Self::ItemAttach { item_id } => CommandV1::ItemAttach { item_id },
            Self::ItemClear { item_id } => CommandV1::ItemClear { item_id },
        }
    }

    pub fn command(&self) -> CommandV1 {
        self.clone().into_command()
    }

    pub const fn public_command_name(&self) -> &'static str {
        match self {
            Self::WorkspaceInitialize => "workspace.init",
            Self::WorkspaceResetAll => "workspace.reset_all",
            Self::SessionStart => "session.start",
            Self::SessionStartReplace => "session.start_replace",
            Self::SessionComplete => "session.complete",
            Self::SessionSkip => "session.skip",
            Self::SessionRetry => "session.retry",
            Self::SessionBlock => "session.block",
            Self::SessionUnblock => "session.unblock",
            Self::SessionCancel => "session.cancel",
            Self::SessionReset => "session.reset",
            Self::SessionDecide => "session.decide",
            Self::SessionRework => "session.rework",
            Self::GoalDefine => "goal.define",
            Self::GoalRevise => "goal.revise",
            Self::GoalAssessCriterion => "goal.assess_criterion",
            Self::ItemCheck { .. } => "item.check",
            Self::ItemUncheck { .. } => "item.uncheck",
            Self::ItemSet { .. } => "item.set",
            Self::ItemAdd { .. } => "item.add",
            Self::ItemRemove { .. } => "item.remove",
            Self::ItemAttach { .. } => "item.attach",
            Self::ItemClear { .. } => "item.clear",
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandEnvelopeV3 {
    schema: String,
    command: PersistedDomainCommandV1,
    preconditions: PersistedPreconditionsV1,
    canonical_execution_json: String,
    execution_flavor: String,
    expected_session_id: Option<SessionId>,
}

/// Canonically encodes the command and all admission preconditions required after a claim.
pub fn encode_command_v1(execution: &ClaimedExecutionV1) -> Result<String, StoreCodecErrorV1> {
    if execution.execution_flavor() == crate::DurableExecutionFlavorV1::ProcedureV2 {
        if !execution.has_complete_execution_document()
            || !procedure_v2_runtime_command(execution.command())
        {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "Procedure v2 execution flavor",
            });
        }
        let expected_session_id = match (execution.command(), execution.session_identity()) {
            (CommandV1::SessionStart, crate::AdmissionSessionIdentityV1::Absent) => None,
            (
                CommandV1::SessionStartReplace,
                crate::AdmissionSessionIdentityV1::Exact(session_id),
            ) => Some(session_id.clone()),
            (command, crate::AdmissionSessionIdentityV1::Exact(session_id))
                if procedure_v2_current_session_command(command) =>
            {
                Some(session_id.clone())
            }
            _ => {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "Procedure v2 session identity",
                });
            }
        };
        return canonical_json(&CommandEnvelopeV3 {
            schema: STORE_GRAPH_COMMAND_SCHEMA_V1.to_owned(),
            command: PersistedDomainCommandV1::from_command(execution.command()),
            preconditions: execution.preconditions().into(),
            canonical_execution_json: execution.canonical_execution().as_str().to_owned(),
            execution_flavor: "procedure_v2".to_owned(),
            expected_session_id,
        });
    }
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
        STORE_GRAPH_COMMAND_SCHEMA_V1 => {
            let envelope: CommandEnvelopeV3 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            if envelope.execution_flavor != "procedure_v2" {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "Procedure v2 execution flavor",
                });
            }
            let command = envelope.command.into_command();
            if !procedure_v2_runtime_command(&command) {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "Procedure v2 execution command",
                });
            }
            let canonical_execution =
                CanonicalExecutionJsonV1::new(envelope.canonical_execution_json).map_err(|_| {
                    StoreCodecErrorV1::InvalidValue {
                        field: "canonical execution JSON",
                    }
                })?;
            let preconditions = envelope.preconditions.into_preconditions()?;
            let session_identity = match (&command, envelope.expected_session_id) {
                (CommandV1::SessionStart, None)
                    if preconditions.expected_session_revision().is_none() =>
                {
                    crate::AdmissionSessionIdentityV1::Absent
                }
                (CommandV1::SessionStartReplace, Some(session_id))
                    if preconditions.expected_session_revision().is_some() =>
                {
                    crate::AdmissionSessionIdentityV1::Exact(session_id)
                }
                (command, Some(session_id))
                    if procedure_v2_current_session_command(command)
                        && procedure_v2_preconditions_match(command, &preconditions) =>
                {
                    crate::AdmissionSessionIdentityV1::Exact(session_id)
                }
                _ => {
                    return Err(StoreCodecErrorV1::InvalidValue {
                        field: "Procedure v2 session identity",
                    });
                }
            };
            ClaimedExecutionV1::new_procedure_v2(
                command,
                preconditions,
                canonical_execution,
                session_identity,
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

fn procedure_v2_runtime_command(command: &CommandV1) -> bool {
    matches!(
        command,
        CommandV1::SessionStart
            | CommandV1::SessionStartReplace
            | CommandV1::SessionComplete
            | CommandV1::SessionRetry
            | CommandV1::SessionSkip
            | CommandV1::SessionBlock
            | CommandV1::SessionUnblock
            | CommandV1::SessionCancel
            | CommandV1::SessionReset
            | CommandV1::SessionDecide
            | CommandV1::SessionRework
            | CommandV1::GoalDefine
            | CommandV1::GoalRevise
            | CommandV1::GoalAssessCriterion
            | CommandV1::ItemCheck { .. }
            | CommandV1::ItemUncheck { .. }
            | CommandV1::ItemSet { .. }
            | CommandV1::ItemAdd { .. }
            | CommandV1::ItemRemove { .. }
            | CommandV1::ItemAttach { .. }
            | CommandV1::ItemClear { .. }
    )
}

fn procedure_v2_current_session_command(command: &CommandV1) -> bool {
    matches!(
        command,
        CommandV1::SessionComplete
            | CommandV1::SessionRetry
            | CommandV1::SessionSkip
            | CommandV1::SessionBlock
            | CommandV1::SessionUnblock
            | CommandV1::SessionCancel
            | CommandV1::SessionReset
            | CommandV1::SessionDecide
            | CommandV1::SessionRework
            | CommandV1::GoalDefine
            | CommandV1::GoalRevise
            | CommandV1::GoalAssessCriterion
            | CommandV1::ItemCheck { .. }
            | CommandV1::ItemUncheck { .. }
            | CommandV1::ItemSet { .. }
            | CommandV1::ItemAdd { .. }
            | CommandV1::ItemRemove { .. }
            | CommandV1::ItemAttach { .. }
            | CommandV1::ItemClear { .. }
    )
}

fn procedure_v2_preconditions_match(
    command: &CommandV1,
    preconditions: &RevisionAttemptItemPreconditionsV1,
) -> bool {
    match command {
        CommandV1::SessionComplete
        | CommandV1::SessionRetry
        | CommandV1::SessionSkip
        | CommandV1::SessionBlock
        | CommandV1::SessionUnblock
        | CommandV1::SessionCancel => {
            preconditions.expected_session_revision().is_some()
                && preconditions.expected_attempt_id().is_some()
                && preconditions.expected_item_id().is_none()
                && preconditions.expected_item_revision().is_none()
        }
        CommandV1::SessionDecide => {
            preconditions.expected_session_revision().is_some()
                && preconditions.expected_attempt_id().is_some()
                && preconditions.expected_item_id().is_none()
                && preconditions.expected_item_revision().is_none()
        }
        CommandV1::SessionRework => {
            preconditions.expected_session_revision().is_some()
                && preconditions.expected_item_id().is_none()
                && preconditions.expected_item_revision().is_none()
        }
        CommandV1::GoalDefine => {
            preconditions.expected_session_revision().is_some()
                && preconditions.expected_attempt_id().is_none()
                && preconditions.expected_item_id().is_none()
                && preconditions.expected_item_revision().is_none()
        }
        CommandV1::GoalRevise => {
            preconditions.expected_session_revision().is_some()
                && preconditions.expected_item_id().is_none()
                && preconditions.expected_item_revision().is_none()
        }
        CommandV1::GoalAssessCriterion => {
            preconditions.expected_session_revision().is_some()
                && preconditions.expected_attempt_id().is_some()
                && preconditions.expected_item_id().is_none()
                && preconditions.expected_item_revision().is_none()
        }
        CommandV1::SessionReset => {
            preconditions.expected_session_revision().is_some()
                && preconditions.expected_attempt_id().is_none()
                && preconditions.expected_item_id().is_none()
                && preconditions.expected_item_revision().is_none()
        }
        CommandV1::ItemCheck { item_id }
        | CommandV1::ItemUncheck { item_id }
        | CommandV1::ItemSet { item_id }
        | CommandV1::ItemAdd { item_id }
        | CommandV1::ItemRemove { item_id }
        | CommandV1::ItemAttach { item_id }
        | CommandV1::ItemClear { item_id } => {
            preconditions.expected_session_revision().is_none()
                && preconditions.expected_attempt_id().is_some()
                && preconditions.expected_item_id() == Some(item_id)
                && preconditions.expected_item_revision().is_some()
        }
        _ => false,
    }
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
            DomainCommandKind::SessionBlock => Self::SessionBlock,
            DomainCommandKind::SessionUnblock => Self::SessionUnblock,
            DomainCommandKind::SessionCancel => Self::SessionCancel,
            DomainCommandKind::SessionReset => Self::SessionReset,
            DomainCommandKind::SessionDecide => Self::SessionDecide,
            DomainCommandKind::SessionRework => Self::SessionRework,
            DomainCommandKind::GoalDefine => Self::GoalDefine,
            DomainCommandKind::GoalRevise => Self::GoalRevise,
            DomainCommandKind::GoalAssessCriterion => Self::GoalAssessCriterion,
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
    BlockerLimitReached {
        maximum_open_blockers: u64,
    },
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
            DomainError::BlockerLimitReached {
                maximum_open_blockers,
            } => Self::BlockerLimitReached {
                maximum_open_blockers: u64::try_from(*maximum_open_blockers)
                    .expect("usize fits in u64"),
            },
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

/// Bounded immutable Procedure v2 session facts captured in the same transaction as a terminal
/// job receipt. This is separate from the legacy aggregate projection so no v1 shape is forged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersistedGraphTerminalOperationV2 {
    GoalDefine {
        record: Value,
    },
    GoalRevise {
        record: Value,
        target_attempt_id: AttemptId,
    },
    GoalAssessCriterion {
        record: Value,
    },
    Decide {
        record: Value,
        target_attempt_id: AttemptId,
    },
    Rework {
        record: Value,
    },
    Complete {
        from_graph_node_id: GraphNodeId,
        from_attempt_id: AttemptId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_graph_node_id: Option<GraphNodeId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_attempt_id: Option<AttemptId>,
    },
    Retry {
        graph_node_id: GraphNodeId,
        from_attempt_id: AttemptId,
        to_attempt_id: AttemptId,
        reason: String,
    },
    Skip {
        from_graph_node_id: GraphNodeId,
        from_attempt_id: AttemptId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_graph_node_id: Option<GraphNodeId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_attempt_id: Option<AttemptId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Block {
        graph_node_id: GraphNodeId,
        attempt_id: AttemptId,
        blocker_id: BlockerId,
        reason: String,
    },
    Unblock {
        graph_node_id: GraphNodeId,
        attempt_id: AttemptId,
        all: bool,
        blocker_ids: Vec<BlockerId>,
    },
    Cancel {
        graph_node_id: GraphNodeId,
        attempt_id: AttemptId,
        reason: String,
    },
    Reset {
        session_id: SessionId,
    },
    ItemMutation {
        graph_node_id: GraphNodeId,
        attempt_id: AttemptId,
        attempt_number: u64,
        item_id: ItemId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value_digest: Option<Sha256Digest>,
    },
    Failure {
        error: PersistedGraphMutationFailureV2,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersistedGraphMutationFailureV2 {
    GoalTrackingNotEnabled,
    SessionGoalAlreadyDefined {
        goal_revision: u64,
    },
    GoalRevisionStale {
        expected_goal_revision: u64,
        actual_goal_revision: u64,
    },
    GoalRevisionTargetNotAllowed {
        target_graph_node_id: GraphNodeId,
    },
    GoalRevisionTargetNotRevisionSafe {
        target_graph_node_id: GraphNodeId,
    },
    ReactivationFlagRequired,
    SessionNotRunning,
    SessionCancelled,
    SessionRevisionConflict {
        expected: RevisionV1,
        actual: RevisionV1,
    },
    AttemptNotCurrent {
        expected: AttemptId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actual: Option<AttemptId>,
    },
    GraphNodeTypeMismatch {
        graph_node_id: GraphNodeId,
        actual: String,
    },
    OptionNotAllowed {
        graph_node_id: GraphNodeId,
        option_id: podway_core::OptionId,
        allowed_option_ids: Vec<podway_core::OptionId>,
    },
    RouteNotAllowed {
        graph_node_id: GraphNodeId,
        option_id: podway_core::OptionId,
    },
    ManualReworkTargetNotAllowed {
        target_graph_node_id: GraphNodeId,
    },
    ManualReworkTargetNotOnTrace {
        target_graph_node_id: GraphNodeId,
    },
    DecisionReasonMissing {
        graph_node_id: GraphNodeId,
    },
    EvidenceReferenceUnresolved {
        graph_node_id: GraphNodeId,
        source_graph_node_ids: Vec<GraphNodeId>,
    },
    EvidenceReferenceStale {
        graph_node_id: GraphNodeId,
        source_graph_node_id: GraphNodeId,
        expected_source_attempt_id: AttemptId,
        current_source_attempt_id: Option<AttemptId>,
    },
    GoalAssessmentDecisionRequiresAssessment {
        graph_node_id: GraphNodeId,
    },
    GoalAssessmentDecisionRequired {
        graph_node_id: GraphNodeId,
    },
    CriterionNotFound {
        criterion_id: podway_core::CriterionId,
    },
    CriterionResultAlreadyRecorded {
        criterion_id: podway_core::CriterionId,
    },
    CriterionModeMixed {
        criterion_id: podway_core::CriterionId,
        expected_mode: String,
        actual_status: String,
    },
    CriterionCitationInvalid {
        criterion_id: podway_core::CriterionId,
        citation: Value,
    },
    CriterionResultMissing {
        missing_criterion_ids: Vec<podway_core::CriterionId>,
    },
    GoalAssessmentOutcomeNotAllowed {
        option_id: podway_core::OptionId,
        determined_outcome: String,
        allowed_option_ids: Vec<podway_core::OptionId>,
    },
    SkipNotAllowed {
        graph_node_id: GraphNodeId,
    },
    SkipReasonRequired {
        graph_node_id: GraphNodeId,
    },
    BlockerIdAlreadyUsed {
        blocker_id: BlockerId,
    },
    TooManyOpenBlockers {
        maximum: u32,
    },
    BlockerNotFound {
        blocker_id: BlockerId,
    },
    BlockerNotCurrent {
        blocker_id: BlockerId,
    },
    BlockerAlreadyResolved {
        blocker_id: BlockerId,
    },
    NoOpenBlockers,
    ItemNotFound {
        item_id: ItemId,
    },
    ItemRevisionConflict {
        expected: RevisionV1,
        actual: RevisionV1,
    },
    ItemTypeMismatch,
    ItemConstraintFailed,
    ListValueNotFound,
    ListValueDuplicate,
    ArtifactChanged,
    RequiredItemsMissing {
        item_ids: Vec<ItemId>,
    },
    BlockersPresent,
    SessionGoalMissing,
    FreshGoalAssessmentMissing {
        goal_revision: u64,
    },
}

impl TryFrom<&crate::GraphMutationErrorV2> for PersistedGraphMutationFailureV2 {
    type Error = StoreCodecErrorV1;

    fn try_from(error: &crate::GraphMutationErrorV2) -> Result<Self, Self::Error> {
        Ok(match error {
            crate::GraphMutationErrorV2::GoalTrackingNotEnabled => Self::GoalTrackingNotEnabled,
            crate::GraphMutationErrorV2::SessionGoalAlreadyDefined { goal_revision } => {
                Self::SessionGoalAlreadyDefined {
                    goal_revision: goal_revision.get(),
                }
            }
            crate::GraphMutationErrorV2::GoalRevisionStale { expected, actual } => {
                Self::GoalRevisionStale {
                    expected_goal_revision: expected.get(),
                    actual_goal_revision: actual.get(),
                }
            }
            crate::GraphMutationErrorV2::GoalRevisionTargetNotAllowed {
                target_graph_node_id,
            } => Self::GoalRevisionTargetNotAllowed {
                target_graph_node_id: target_graph_node_id.clone(),
            },
            crate::GraphMutationErrorV2::GoalRevisionTargetNotRevisionSafe {
                target_graph_node_id,
            } => Self::GoalRevisionTargetNotRevisionSafe {
                target_graph_node_id: target_graph_node_id.clone(),
            },
            crate::GraphMutationErrorV2::ReactivationFlagRequired => Self::ReactivationFlagRequired,
            crate::GraphMutationErrorV2::SessionNotRunning => Self::SessionNotRunning,
            crate::GraphMutationErrorV2::SessionCancelled => Self::SessionCancelled,
            crate::GraphMutationErrorV2::SessionRevisionConflict { expected, actual } => {
                Self::SessionRevisionConflict {
                    expected: *expected,
                    actual: *actual,
                }
            }
            crate::GraphMutationErrorV2::AttemptNotCurrent { expected, actual } => {
                Self::AttemptNotCurrent {
                    expected: expected.clone(),
                    actual: actual.clone(),
                }
            }
            crate::GraphMutationErrorV2::GraphNodeTypeMismatch {
                graph_node_id,
                actual,
            } => Self::GraphNodeTypeMismatch {
                graph_node_id: graph_node_id.clone(),
                actual: match actual {
                    podway_core::NodeKindV2::Action => "action",
                    podway_core::NodeKindV2::Decision => "decision",
                }
                .to_owned(),
            },
            crate::GraphMutationErrorV2::OptionNotAllowed {
                graph_node_id,
                option_id,
                allowed_option_ids,
            } => Self::OptionNotAllowed {
                graph_node_id: graph_node_id.clone(),
                option_id: option_id.clone(),
                allowed_option_ids: allowed_option_ids.clone(),
            },
            crate::GraphMutationErrorV2::RouteNotAllowed {
                graph_node_id,
                option_id,
            } => Self::RouteNotAllowed {
                graph_node_id: graph_node_id.clone(),
                option_id: option_id.clone(),
            },
            crate::GraphMutationErrorV2::ManualReworkTargetNotAllowed {
                target_graph_node_id,
            } => Self::ManualReworkTargetNotAllowed {
                target_graph_node_id: target_graph_node_id.clone(),
            },
            crate::GraphMutationErrorV2::ManualReworkTargetNotOnTrace {
                target_graph_node_id,
            } => Self::ManualReworkTargetNotOnTrace {
                target_graph_node_id: target_graph_node_id.clone(),
            },
            crate::GraphMutationErrorV2::DecisionReasonMissing { graph_node_id } => {
                Self::DecisionReasonMissing {
                    graph_node_id: graph_node_id.clone(),
                }
            }
            crate::GraphMutationErrorV2::EvidenceReferenceUnresolved {
                graph_node_id,
                source_graph_node_ids,
            } => Self::EvidenceReferenceUnresolved {
                graph_node_id: graph_node_id.clone(),
                source_graph_node_ids: source_graph_node_ids.clone(),
            },
            crate::GraphMutationErrorV2::EvidenceReferenceStale {
                graph_node_id,
                source_graph_node_id,
                expected_source_attempt_id,
                current_source_attempt_id,
            } => Self::EvidenceReferenceStale {
                graph_node_id: graph_node_id.clone(),
                source_graph_node_id: source_graph_node_id.clone(),
                expected_source_attempt_id: expected_source_attempt_id.clone(),
                current_source_attempt_id: current_source_attempt_id.clone(),
            },
            crate::GraphMutationErrorV2::GoalAssessmentDecisionRequiresAssessment {
                graph_node_id,
            } => Self::GoalAssessmentDecisionRequiresAssessment {
                graph_node_id: graph_node_id.clone(),
            },
            crate::GraphMutationErrorV2::GoalAssessmentDecisionRequired { graph_node_id } => {
                Self::GoalAssessmentDecisionRequired {
                    graph_node_id: graph_node_id.clone(),
                }
            }
            crate::GraphMutationErrorV2::CriterionNotFound { criterion_id } => {
                Self::CriterionNotFound {
                    criterion_id: criterion_id.clone(),
                }
            }
            crate::GraphMutationErrorV2::CriterionResultAlreadyRecorded { criterion_id } => {
                Self::CriterionResultAlreadyRecorded {
                    criterion_id: criterion_id.clone(),
                }
            }
            crate::GraphMutationErrorV2::CriterionModeMixed {
                criterion_id,
                expected_mode,
                actual_status,
            } => Self::CriterionModeMixed {
                criterion_id: criterion_id.clone(),
                expected_mode: expected_mode.as_str().to_owned(),
                actual_status: actual_status.as_str().to_owned(),
            },
            crate::GraphMutationErrorV2::CriterionCitationInvalid {
                criterion_id,
                citation,
            } => Self::CriterionCitationInvalid {
                criterion_id: criterion_id.clone(),
                citation: match citation {
                    podway_core::CriterionCitationV2::Evidence(graph_node_id) => {
                        serde_json::json!({
                            "kind": "evidence",
                            "graph_node_id": graph_node_id.as_str(),
                        })
                    }
                    podway_core::CriterionCitationV2::Item(item_id) => serde_json::json!({
                        "kind": "item",
                        "item_id": item_id.as_str(),
                    }),
                },
            },
            crate::GraphMutationErrorV2::SkipNotAllowed { graph_node_id } => Self::SkipNotAllowed {
                graph_node_id: graph_node_id.clone(),
            },
            crate::GraphMutationErrorV2::SkipReasonRequired { graph_node_id } => {
                Self::SkipReasonRequired {
                    graph_node_id: graph_node_id.clone(),
                }
            }
            crate::GraphMutationErrorV2::CriterionResultMissing {
                missing_criterion_ids,
            } => Self::CriterionResultMissing {
                missing_criterion_ids: missing_criterion_ids.clone(),
            },
            crate::GraphMutationErrorV2::GoalAssessmentOutcomeNotAllowed {
                option_id,
                determined_outcome,
                allowed_option_ids,
            } => Self::GoalAssessmentOutcomeNotAllowed {
                option_id: option_id.clone(),
                determined_outcome: determined_outcome.as_str().to_owned(),
                allowed_option_ids: allowed_option_ids.clone(),
            },
            crate::GraphMutationErrorV2::BlockerIdAlreadyUsed { blocker_id } => {
                Self::BlockerIdAlreadyUsed {
                    blocker_id: blocker_id.clone(),
                }
            }
            crate::GraphMutationErrorV2::TooManyOpenBlockers { maximum } => {
                Self::TooManyOpenBlockers { maximum: *maximum }
            }
            crate::GraphMutationErrorV2::BlockerNotFound { blocker_id } => Self::BlockerNotFound {
                blocker_id: blocker_id.clone(),
            },
            crate::GraphMutationErrorV2::BlockerNotCurrent { blocker_id } => {
                Self::BlockerNotCurrent {
                    blocker_id: blocker_id.clone(),
                }
            }
            crate::GraphMutationErrorV2::BlockerAlreadyResolved { blocker_id } => {
                Self::BlockerAlreadyResolved {
                    blocker_id: blocker_id.clone(),
                }
            }
            crate::GraphMutationErrorV2::NoOpenBlockers => Self::NoOpenBlockers,
            crate::GraphMutationErrorV2::ItemNotFound { item_id } => Self::ItemNotFound {
                item_id: item_id.clone(),
            },
            crate::GraphMutationErrorV2::ItemRevisionConflict { expected, actual } => {
                Self::ItemRevisionConflict {
                    expected: *expected,
                    actual: *actual,
                }
            }
            crate::GraphMutationErrorV2::ItemTypeMismatch => Self::ItemTypeMismatch,
            crate::GraphMutationErrorV2::ItemConstraintFailed => Self::ItemConstraintFailed,
            crate::GraphMutationErrorV2::ListValueNotFound => Self::ListValueNotFound,
            crate::GraphMutationErrorV2::ListValueDuplicate => Self::ListValueDuplicate,
            crate::GraphMutationErrorV2::RequiredItemsMissing { item_ids } => {
                Self::RequiredItemsMissing {
                    item_ids: item_ids.clone(),
                }
            }
            crate::GraphMutationErrorV2::BlockersPresent => Self::BlockersPresent,
            crate::GraphMutationErrorV2::SessionGoalMissing => Self::SessionGoalMissing,
            crate::GraphMutationErrorV2::FreshGoalAssessmentMissing { goal_revision } => {
                Self::FreshGoalAssessmentMissing {
                    goal_revision: goal_revision.get(),
                }
            }
            crate::GraphMutationErrorV2::InvalidState(_)
            | crate::GraphMutationErrorV2::Domain(_) => {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "Procedure v2 graph mutation failure",
                });
            }
        })
    }
}

fn rfc3339_millis_shape_v2(value: &str) -> bool {
    let bytes = value.as_bytes();
    if !(bytes.len() == 24
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && bytes[23] == b'Z'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19 | 23) || byte.is_ascii_digit()
        }))
    {
        return false;
    }
    let number = |range: std::ops::Range<usize>| {
        value[range]
            .parse::<u32>()
            .expect("ASCII digit range is a number")
    };
    let year = number(0..4);
    let month = number(5..7);
    let day = number(8..10);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day) && number(11..13) < 24 && number(14..16) < 60 && number(17..19) < 60
}

fn decision_reference_projection_shape_v2(reference: &Value) -> Option<GraphNodeId> {
    let reference = reference.as_object()?;
    let source = reference
        .get("source_graph_node_id")
        .and_then(Value::as_str)
        .and_then(|value| GraphNodeId::new(value.to_owned()).ok())?;
    match reference.get("state").and_then(Value::as_str) {
        Some("unresolved") if reference.len() == 2 => Some(source),
        Some("resolved" | "skipped")
            if reference.len() == 5
                && reference
                    .get("source_attempt_id")
                    .and_then(Value::as_str)
                    .is_some_and(|value| AttemptId::new(value.to_owned()).is_ok())
                && reference
                    .get("source_attempt_number")
                    .and_then(Value::as_u64)
                    .is_some_and(|value| value > 0)
                && reference
                    .get("items_digest")
                    .and_then(Value::as_str)
                    .is_some_and(|value| Sha256Digest::new(value.to_owned()).is_ok()) =>
        {
            Some(source)
        }
        _ => None,
    }
}

fn decision_criterion_projection_v2(value: &Value) -> Option<CriterionAssessmentResultV2> {
    let value = value.as_object()?;
    if value.len() != 4 {
        return None;
    }
    let criterion_id = value
        .get("criterion_id")
        .and_then(Value::as_str)
        .and_then(|value| CriterionId::new(value.to_owned()).ok())?;
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .and_then(|value| CriterionStatusV2::from_str(value).ok())?;
    let reason = value
        .get("reason")
        .and_then(Value::as_str)
        .and_then(|value| CriterionAssessmentReasonV2::new(value.to_owned()).ok())?;
    let citations = value
        .get("citations")
        .and_then(Value::as_array)?
        .iter()
        .map(|citation| {
            let citation = citation.as_object()?;
            if citation.len() != 1 {
                return None;
            }
            if let Some(source) = citation
                .get("reference_graph_node_id")
                .and_then(Value::as_str)
                .and_then(|value| GraphNodeId::new(value.to_owned()).ok())
            {
                Some(CriterionCitationV2::Evidence(source))
            } else {
                citation
                    .get("local_item_id")
                    .and_then(Value::as_str)
                    .and_then(|value| ItemId::new(value.to_owned()).ok())
                    .map(CriterionCitationV2::Item)
            }
        })
        .collect::<Option<Vec<_>>>()?;
    CriterionAssessmentResultV2::new(criterion_id, status, reason, citations).ok()
}

fn decision_record_projection_shape_v2(record: &Value) -> bool {
    let Some(record) = record.as_object() else {
        return false;
    };
    const BASE_KEYS: [&str; 17] = [
        "trace_sequence",
        "session_id",
        "session_revision",
        "procedure_schema",
        "procedure_snapshot_id",
        "procedure_digest",
        "graph_node_id",
        "node_definition_id",
        "attempt_id",
        "attempt_number",
        "goal_revision",
        "option_id",
        "effect",
        "target_graph_node_id",
        "reason",
        "recorded_at",
        "references",
    ];
    const ASSESSMENT_KEYS: [&str; 4] = [
        "assessment",
        "assessment_mode",
        "goal_outcome",
        "criterion_results",
    ];
    let assessment_fields = ASSESSMENT_KEYS
        .iter()
        .filter(|key| record.contains_key(**key))
        .count();
    let assessment = assessment_fields == ASSESSMENT_KEYS.len();
    if !matches!(assessment_fields, 0 | 4)
        || record.len()
            != BASE_KEYS.len() + usize::from(record.contains_key("actor")) + assessment_fields
        || BASE_KEYS.iter().any(|key| !record.contains_key(*key))
        || !record
            .get("trace_sequence")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0)
        || !record
            .get("session_id")
            .and_then(Value::as_str)
            .is_some_and(|value| SessionId::new(value.to_owned()).is_ok())
        || !record
            .get("session_revision")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0)
        || record.get("procedure_schema").and_then(Value::as_str) != Some("podway.procedure/v2")
        || !record
            .get("procedure_snapshot_id")
            .and_then(Value::as_str)
            .is_some_and(|value| ProcedureSnapshotId::new(value.to_owned()).is_ok())
        || !record
            .get("procedure_digest")
            .and_then(Value::as_str)
            .is_some_and(|value| Sha256Digest::new(value.to_owned()).is_ok())
        || !record
            .get("graph_node_id")
            .and_then(Value::as_str)
            .is_some_and(|value| GraphNodeId::new(value.to_owned()).is_ok())
        || !record
            .get("node_definition_id")
            .and_then(Value::as_str)
            .is_some_and(|value| NodeDefinitionId::new(value.to_owned()).is_ok())
        || !record
            .get("attempt_id")
            .and_then(Value::as_str)
            .is_some_and(|value| AttemptId::new(value.to_owned()).is_ok())
        || !record
            .get("attempt_number")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0)
        || !record.get("goal_revision").is_some_and(|value| {
            value.is_null() || value.as_u64().is_some_and(|revision| revision > 0)
        })
        || !record
            .get("option_id")
            .and_then(Value::as_str)
            .is_some_and(|value| OptionId::new(value.to_owned()).is_ok())
        || !matches!(
            record.get("effect").and_then(Value::as_str),
            Some("advance" | "rework")
        )
        || !record
            .get("target_graph_node_id")
            .and_then(Value::as_str)
            .is_some_and(|value| GraphNodeId::new(value.to_owned()).is_ok())
        || !record
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|value| ReasonV2::new(value.to_owned()).is_ok())
        || !record.get("actor").is_none_or(|value| {
            value
                .as_str()
                .is_some_and(|value| ActorAttributionV2::new(value.to_owned()).is_ok())
        })
        || !record
            .get("recorded_at")
            .and_then(Value::as_str)
            .is_some_and(rfc3339_millis_shape_v2)
    {
        return false;
    }
    let Some(references) = record.get("references").and_then(Value::as_array) else {
        return false;
    };
    let references = references
        .iter()
        .map(decision_reference_projection_shape_v2)
        .collect::<Option<Vec<_>>>();
    if !references.is_some_and(|references| references.len() <= 8) {
        return false;
    }
    if !assessment {
        return true;
    }
    if record.get("assessment").and_then(Value::as_str) != Some("session_goal")
        || record
            .get("goal_revision")
            .and_then(Value::as_u64)
            .is_none()
    {
        return false;
    }
    let Some(results) = record
        .get("criterion_results")
        .and_then(Value::as_array)
        .filter(|results| (1..=16).contains(&results.len()))
        .and_then(|results| {
            results
                .iter()
                .map(decision_criterion_projection_v2)
                .collect::<Option<Vec<_>>>()
        })
    else {
        return false;
    };
    if results
        .iter()
        .map(CriterionAssessmentResultV2::criterion_id)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != results.len()
        || results
            .iter()
            .any(|result| result.mode() != results[0].mode())
        || record.get("assessment_mode").and_then(Value::as_str) != Some(results[0].mode().as_str())
    {
        return false;
    }
    let outcome = if results[0].mode() == podway_core::CriterionAssessmentModeV2::Applicability {
        GoalOutcome::Superseded
    } else if results
        .iter()
        .all(|result| result.status() == CriterionStatusV2::Satisfied)
    {
        GoalOutcome::Achieved
    } else {
        GoalOutcome::NotAchieved
    };
    record.get("goal_outcome").and_then(Value::as_str) == Some(outcome.as_str())
}

fn goal_revision_record_projection_shape_v2(record: &Value, define: bool) -> bool {
    let Some(record) = record.as_object() else {
        return false;
    };
    let required = if define {
        &["goal_revision", "statement", "criteria", "recorded_at"][..]
    } else {
        &[
            "goal_revision",
            "statement",
            "criteria",
            "reason",
            "recorded_at",
            "rework_to",
            "reactivated",
        ][..]
    };
    let actor = record.get("actor");
    if record.len() != required.len() + usize::from(actor.is_some())
        || required.iter().any(|key| !record.contains_key(*key))
    {
        return false;
    }
    let Some(goal_revision) = record.get("goal_revision").and_then(Value::as_u64) else {
        return false;
    };
    if (define && goal_revision != 1) || (!define && goal_revision < 2) {
        return false;
    }
    if !record
        .get("statement")
        .and_then(Value::as_str)
        .is_some_and(|value| podway_core::GoalStatementV2::new(value.to_owned()).is_ok())
        || !record
            .get("recorded_at")
            .and_then(Value::as_str)
            .is_some_and(rfc3339_millis_shape_v2)
        || !actor.is_none_or(|value| {
            value
                .as_str()
                .is_some_and(|value| ActorAttributionV2::new(value.to_owned()).is_ok())
        })
    {
        return false;
    }
    let Some(criteria) = record.get("criteria").and_then(Value::as_array) else {
        return false;
    };
    let criteria = criteria
        .iter()
        .map(|criterion| {
            let criterion = criterion.as_object()?;
            if criterion.len() != 2 {
                return None;
            }
            let id = criterion
                .get("criterion_id")
                .and_then(Value::as_str)
                .and_then(|value| podway_core::CriterionId::new(value.to_owned()).ok())?;
            let statement = criterion.get("statement").and_then(Value::as_str)?;
            podway_core::GoalCriterionV2::new(id, statement.to_owned()).ok()
        })
        .collect::<Option<Vec<_>>>();
    if !criteria.is_some_and(|criteria| podway_core::GoalDefinitionV2::new(criteria).is_ok()) {
        return false;
    }
    define
        || (record
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|value| podway_core::GoalRevisionReasonV2::new(value.to_owned()).is_ok())
            && record
                .get("rework_to")
                .and_then(Value::as_str)
                .is_some_and(|value| GraphNodeId::new(value.to_owned()).is_ok())
            && record.get("reactivated").and_then(Value::as_bool).is_some())
}

fn criterion_assessment_record_projection_shape_v2(record: &Value) -> bool {
    let Some(record) = record.as_object() else {
        return false;
    };
    let complete = record.get("complete").and_then(Value::as_bool);
    let determined_outcome = record.get("determined_outcome");
    if record.len()
        != 7 + usize::from(record.contains_key("actor")) + usize::from(determined_outcome.is_some())
        || !record
            .get("graph_node_id")
            .and_then(Value::as_str)
            .is_some_and(|value| GraphNodeId::new(value.to_owned()).is_ok())
        || !record
            .get("attempt_id")
            .and_then(Value::as_str)
            .is_some_and(|value| AttemptId::new(value.to_owned()).is_ok())
        || !record
            .get("goal_revision")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0)
        || !record
            .get("recorded_at")
            .and_then(Value::as_str)
            .is_some_and(rfc3339_millis_shape_v2)
        || !record.get("actor").is_none_or(|value| {
            value
                .as_str()
                .is_some_and(|value| ActorAttributionV2::new(value.to_owned()).is_ok())
        })
        || complete.is_none()
        || (complete == Some(true)) != determined_outcome.is_some()
        || !determined_outcome.is_none_or(|value| {
            value
                .as_str()
                .is_some_and(|value| podway_core::GoalOutcome::from_str(value).is_ok())
        })
    {
        return false;
    }
    let Some(result) = record.get("result").and_then(Value::as_object) else {
        return false;
    };
    if result.len() != 4 {
        return false;
    }
    let Some(criterion_id) = result
        .get("criterion_id")
        .and_then(Value::as_str)
        .and_then(|value| podway_core::CriterionId::new(value.to_owned()).ok())
    else {
        return false;
    };
    let Some(status) = result
        .get("status")
        .and_then(Value::as_str)
        .and_then(|value| podway_core::CriterionStatusV2::from_str(value).ok())
    else {
        return false;
    };
    if record.get("mode").and_then(Value::as_str)
        != Some(podway_core::CriterionAssessmentModeV2::from_status(status).as_str())
    {
        return false;
    }
    if complete == Some(true) {
        let outcome = determined_outcome.and_then(Value::as_str);
        match status {
            podway_core::CriterionStatusV2::Unsatisfied
                if outcome != Some(podway_core::GoalOutcome::NotAchieved.as_str()) =>
            {
                return false;
            }
            podway_core::CriterionStatusV2::NotApplicable
                if outcome != Some(podway_core::GoalOutcome::Superseded.as_str()) =>
            {
                return false;
            }
            _ => {}
        }
    }
    let Some(reason) = result
        .get("reason")
        .and_then(Value::as_str)
        .and_then(|value| podway_core::CriterionAssessmentReasonV2::new(value.to_owned()).ok())
    else {
        return false;
    };
    let Some(citations) = result.get("citations").and_then(Value::as_array) else {
        return false;
    };
    let citations = citations
        .iter()
        .map(|citation| {
            let citation = citation.as_object()?;
            match citation.get("kind").and_then(Value::as_str) {
                Some("evidence") if citation.len() == 2 => citation
                    .get("graph_node_id")
                    .and_then(Value::as_str)
                    .and_then(|value| GraphNodeId::new(value.to_owned()).ok())
                    .map(podway_core::CriterionCitationV2::Evidence),
                Some("item") if citation.len() == 2 => citation
                    .get("item_id")
                    .and_then(Value::as_str)
                    .and_then(|value| ItemId::new(value.to_owned()).ok())
                    .map(podway_core::CriterionCitationV2::Item),
                _ => None,
            }
        })
        .collect::<Option<Vec<_>>>();
    citations.is_some_and(|citations| {
        podway_core::CriterionAssessmentResultV2::new(criterion_id, status, reason, citations)
            .is_ok()
    })
}

impl PersistedGraphTerminalOperationV2 {
    pub fn goal_define(record: Value) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::GoalDefine { record };
        operation.validate()?;
        Ok(operation)
    }

    pub fn goal_revise(
        record: Value,
        target_attempt_id: AttemptId,
    ) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::GoalRevise {
            record,
            target_attempt_id,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn goal_assess_criterion(record: Value) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::GoalAssessCriterion { record };
        operation.validate()?;
        Ok(operation)
    }

    pub fn decide(record: Value, target_attempt_id: AttemptId) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::Decide {
            record,
            target_attempt_id,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn rework(record: Value) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::Rework { record };
        operation.validate()?;
        Ok(operation)
    }

    pub fn complete(
        from_graph_node_id: GraphNodeId,
        from_attempt_id: AttemptId,
        to_graph_node_id: Option<GraphNodeId>,
        to_attempt_id: Option<AttemptId>,
    ) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::Complete {
            from_graph_node_id,
            from_attempt_id,
            to_graph_node_id,
            to_attempt_id,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn item_mutation(
        graph_node_id: GraphNodeId,
        attempt_id: AttemptId,
        attempt_number: AttemptNumberV2,
        item_id: ItemId,
        value_digest: Option<Sha256Digest>,
    ) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::ItemMutation {
            graph_node_id,
            attempt_id,
            attempt_number: attempt_number.get(),
            item_id,
            value_digest,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn retry(
        graph_node_id: GraphNodeId,
        from_attempt_id: AttemptId,
        to_attempt_id: AttemptId,
        reason: ReasonV2,
    ) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::Retry {
            graph_node_id,
            from_attempt_id,
            to_attempt_id,
            reason: reason.into_inner(),
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn skip(
        from_graph_node_id: GraphNodeId,
        from_attempt_id: AttemptId,
        to_graph_node_id: Option<GraphNodeId>,
        to_attempt_id: Option<AttemptId>,
        reason: Option<ReasonV2>,
    ) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::Skip {
            from_graph_node_id,
            from_attempt_id,
            to_graph_node_id,
            to_attempt_id,
            reason: reason.map(ReasonV2::into_inner),
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn block(
        graph_node_id: GraphNodeId,
        attempt_id: AttemptId,
        blocker_id: BlockerId,
        reason: String,
    ) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::Block {
            graph_node_id,
            attempt_id,
            blocker_id,
            reason,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn unblock(
        graph_node_id: GraphNodeId,
        attempt_id: AttemptId,
        all: bool,
        blocker_ids: Vec<BlockerId>,
    ) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::Unblock {
            graph_node_id,
            attempt_id,
            all,
            blocker_ids,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn cancel(
        graph_node_id: GraphNodeId,
        attempt_id: AttemptId,
        reason: ReasonV2,
    ) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::Cancel {
            graph_node_id,
            attempt_id,
            reason: reason.into_inner(),
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn reset(session_id: SessionId) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::Reset { session_id };
        operation.validate()?;
        Ok(operation)
    }

    pub fn failure(error: PersistedGraphMutationFailureV2) -> Result<Self, StoreCodecErrorV1> {
        let operation = Self::Failure { error };
        operation.validate()?;
        Ok(operation)
    }

    fn validate(&self) -> Result<(), StoreCodecErrorV1> {
        let valid = match self {
            Self::GoalDefine { record } => goal_revision_record_projection_shape_v2(record, true),
            Self::GoalRevise {
                record,
                target_attempt_id,
            } => {
                goal_revision_record_projection_shape_v2(record, false)
                    && !target_attempt_id.as_str().is_empty()
            }
            Self::GoalAssessCriterion { record } => {
                criterion_assessment_record_projection_shape_v2(record)
            }
            Self::Decide {
                record,
                target_attempt_id,
            } => {
                decision_record_projection_shape_v2(record)
                    && !target_attempt_id.as_str().is_empty()
            }
            Self::Rework { record } => record.as_object().is_some_and(|record| {
                let actor = record.get("actor");
                let required_keys = [
                    "trace_sequence",
                    "kind",
                    "from_graph_node_id",
                    "to_graph_node_id",
                    "target_attempt_id",
                    "reason",
                    "reactivated",
                    "recorded_at_ms",
                ];
                record.len() == required_keys.len() + usize::from(actor.is_some())
                    && required_keys.iter().all(|key| record.contains_key(*key))
                    && record
                        .get("trace_sequence")
                        .and_then(Value::as_u64)
                        .is_some_and(|trace| trace > 0)
                    && record.get("kind").and_then(Value::as_str) == Some("manual")
                    && record
                        .get("from_graph_node_id")
                        .and_then(Value::as_str)
                        .is_some_and(|value| GraphNodeId::new(value.to_owned()).is_ok())
                    && record
                        .get("to_graph_node_id")
                        .and_then(Value::as_str)
                        .is_some_and(|value| GraphNodeId::new(value.to_owned()).is_ok())
                    && record
                        .get("target_attempt_id")
                        .and_then(Value::as_str)
                        .is_some_and(|value| AttemptId::new(value.to_owned()).is_ok())
                    && record
                        .get("reason")
                        .and_then(Value::as_str)
                        .is_some_and(|value| ReasonV2::new(value.to_owned()).is_ok())
                    && record.get("reactivated").and_then(Value::as_bool).is_some()
                    && record
                        .get("recorded_at_ms")
                        .and_then(Value::as_u64)
                        .is_some()
                    && actor.is_none_or(|value| {
                        value
                            .as_str()
                            .is_some_and(|actor| ActorAttributionV2::new(actor.to_owned()).is_ok())
                    })
            }),
            Self::Complete {
                to_graph_node_id,
                to_attempt_id,
                ..
            } => to_graph_node_id.is_some() == to_attempt_id.is_some(),
            Self::Retry {
                from_attempt_id,
                to_attempt_id,
                reason,
                ..
            } => from_attempt_id != to_attempt_id && ReasonV2::new(reason.clone()).is_ok(),
            Self::Skip {
                to_graph_node_id,
                to_attempt_id,
                reason,
                ..
            } => {
                to_graph_node_id.is_some() == to_attempt_id.is_some()
                    && reason
                        .as_ref()
                        .is_none_or(|reason| ReasonV2::new(reason.clone()).is_ok())
            }
            Self::Block { reason, .. } => {
                !reason.trim().is_empty() && reason.chars().count() <= 1_000
            }
            Self::Unblock { blocker_ids, .. } => {
                !blocker_ids.is_empty()
                    && blocker_ids.len() <= 64
                    && blocker_ids
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        == blocker_ids.len()
            }
            Self::Cancel { reason, .. } => ReasonV2::new(reason.clone()).is_ok(),
            Self::Reset { .. } => true,
            Self::ItemMutation { attempt_number, .. } => *attempt_number > 0,
            Self::Failure { error } => error.validate(),
        };
        if valid {
            Ok(())
        } else {
            Err(StoreCodecErrorV1::InvalidValue {
                field: "Procedure v2 terminal operation projection",
            })
        }
    }
}

impl PersistedGraphMutationFailureV2 {
    fn validate(&self) -> bool {
        match self {
            Self::SessionGoalAlreadyDefined { goal_revision } => *goal_revision > 0,
            Self::GoalRevisionStale {
                expected_goal_revision,
                actual_goal_revision,
            } => {
                *expected_goal_revision > 0
                    && *actual_goal_revision > 0
                    && expected_goal_revision != actual_goal_revision
            }
            Self::GraphNodeTypeMismatch { actual, .. } => {
                matches!(actual.as_str(), "action" | "decision")
            }
            Self::RequiredItemsMissing { item_ids } => {
                !item_ids.is_empty()
                    && item_ids.len() <= 64
                    && item_ids
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        == item_ids.len()
            }
            Self::FreshGoalAssessmentMissing { goal_revision } => *goal_revision > 0,
            Self::CriterionModeMixed {
                expected_mode,
                actual_status,
                ..
            } => matches!(
                (expected_mode.as_str(), actual_status.as_str()),
                ("assessment", "not_applicable") | ("applicability", "satisfied" | "unsatisfied")
            ),
            Self::CriterionCitationInvalid { citation, .. } => {
                citation.as_object().is_some_and(|citation| {
                    match citation.get("kind").and_then(Value::as_str) {
                        Some("evidence") if citation.len() == 2 => citation
                            .get("graph_node_id")
                            .and_then(Value::as_str)
                            .is_some_and(|value| GraphNodeId::new(value.to_owned()).is_ok()),
                        Some("item") if citation.len() == 2 => citation
                            .get("item_id")
                            .and_then(Value::as_str)
                            .is_some_and(|value| ItemId::new(value.to_owned()).is_ok()),
                        _ => false,
                    }
                })
            }
            Self::CriterionResultMissing {
                missing_criterion_ids,
            } => {
                !missing_criterion_ids.is_empty()
                    && missing_criterion_ids.len() <= 16
                    && missing_criterion_ids
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        == missing_criterion_ids.len()
            }
            Self::GoalAssessmentOutcomeNotAllowed {
                option_id,
                determined_outcome,
                allowed_option_ids,
            } => {
                podway_core::GoalOutcome::from_str(determined_outcome).is_ok()
                    && !allowed_option_ids.is_empty()
                    && !allowed_option_ids.contains(option_id)
                    && allowed_option_ids.len() <= 8
                    && allowed_option_ids
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        == allowed_option_ids.len()
            }
            Self::SessionNotRunning
            | Self::GoalTrackingNotEnabled
            | Self::GoalRevisionTargetNotAllowed { .. }
            | Self::GoalRevisionTargetNotRevisionSafe { .. }
            | Self::ReactivationFlagRequired
            | Self::SessionCancelled
            | Self::SessionRevisionConflict { .. }
            | Self::AttemptNotCurrent { .. }
            | Self::OptionNotAllowed { .. }
            | Self::RouteNotAllowed { .. }
            | Self::ManualReworkTargetNotAllowed { .. }
            | Self::ManualReworkTargetNotOnTrace { .. }
            | Self::DecisionReasonMissing { .. }
            | Self::EvidenceReferenceUnresolved { .. }
            | Self::EvidenceReferenceStale { .. }
            | Self::GoalAssessmentDecisionRequiresAssessment { .. }
            | Self::GoalAssessmentDecisionRequired { .. }
            | Self::CriterionNotFound { .. }
            | Self::CriterionResultAlreadyRecorded { .. }
            | Self::SkipNotAllowed { .. }
            | Self::SkipReasonRequired { .. }
            | Self::BlockerIdAlreadyUsed { .. }
            | Self::BlockerNotFound { .. }
            | Self::BlockerNotCurrent { .. }
            | Self::BlockerAlreadyResolved { .. }
            | Self::NoOpenBlockers
            | Self::ItemNotFound { .. }
            | Self::ItemRevisionConflict { .. }
            | Self::ItemTypeMismatch
            | Self::ItemConstraintFailed
            | Self::ListValueNotFound
            | Self::ListValueDuplicate
            | Self::ArtifactChanged
            | Self::BlockersPresent
            | Self::SessionGoalMissing => true,
            Self::TooManyOpenBlockers { maximum } => *maximum == 64,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedGraphTerminalSessionProjectionV2 {
    session_id: SessionId,
    task_title: String,
    lifecycle: PersistedSessionLifecycleV1,
    revision_before: RevisionV1,
    revision_after: RevisionV1,
    procedure_digest: Sha256Digest,
    entry_graph_node_id: podway_core::GraphNodeId,
    goal_tracking: bool,
    #[serde(default)]
    goal_defined: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operation: Option<PersistedGraphTerminalOperationV2>,
}

impl PersistedGraphTerminalSessionProjectionV2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: SessionId,
        task_title: String,
        lifecycle: PersistedSessionLifecycleV1,
        revision_before: RevisionV1,
        revision_after: RevisionV1,
        procedure_digest: Sha256Digest,
        entry_graph_node_id: podway_core::GraphNodeId,
        goal_tracking: bool,
        goal_defined: bool,
    ) -> Result<Self, StoreCodecErrorV1> {
        let projection = Self {
            session_id,
            task_title,
            lifecycle,
            revision_before,
            revision_after,
            procedure_digest,
            entry_graph_node_id,
            goal_tracking,
            goal_defined,
            operation: None,
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
    pub const fn lifecycle(&self) -> PersistedSessionLifecycleV1 {
        self.lifecycle
    }
    pub const fn revision_before(&self) -> RevisionV1 {
        self.revision_before
    }
    pub const fn revision_after(&self) -> RevisionV1 {
        self.revision_after
    }
    pub fn procedure_digest(&self) -> &Sha256Digest {
        &self.procedure_digest
    }
    pub fn entry_graph_node_id(&self) -> &podway_core::GraphNodeId {
        &self.entry_graph_node_id
    }
    pub const fn goal_tracking(&self) -> bool {
        self.goal_tracking
    }
    pub const fn goal_defined(&self) -> bool {
        self.goal_defined
    }

    pub fn operation(&self) -> Option<&PersistedGraphTerminalOperationV2> {
        self.operation.as_ref()
    }

    pub fn with_operation(
        mut self,
        operation: PersistedGraphTerminalOperationV2,
    ) -> Result<Self, StoreCodecErrorV1> {
        operation.validate()?;
        self.operation = Some(operation);
        self.validate()?;
        Ok(self)
    }

    fn validate(&self) -> Result<(), StoreCodecErrorV1> {
        if self.task_title.trim().is_empty()
            || self.task_title.chars().count() > 500
            || self.revision_after == RevisionV1::ZERO
            || (self.goal_defined && !self.goal_tracking)
            || (self.revision_after < self.revision_before
                && !(self.revision_after == RevisionV1::new(1)
                    && self.revision_before > RevisionV1::ZERO))
        {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "Procedure v2 terminal session projection",
            });
        }
        if let Some(operation) = &self.operation {
            if matches!(
                operation,
                PersistedGraphTerminalOperationV2::GoalDefine { .. }
                    | PersistedGraphTerminalOperationV2::GoalRevise { .. }
                    | PersistedGraphTerminalOperationV2::GoalAssessCriterion { .. }
            ) && !self.goal_defined
            {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "Procedure v2 goal terminal projection",
                });
            }
            if matches!(
                operation,
                PersistedGraphTerminalOperationV2::Decide { record, .. }
                    if record.get("goal_revision").is_some_and(|value| !value.is_null())
            ) && !self.goal_defined
            {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "Procedure v2 goal terminal projection",
                });
            }
            operation.validate()?;
        }
        Ok(())
    }
}

/// Bounded start identity retained after terminal job pruning.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedStartIdentityV1 {
    execution_version: u8,
    procedure_digest: Sha256Digest,
}

impl PersistedStartIdentityV1 {
    pub fn new(
        execution_version: u8,
        procedure_digest: Sha256Digest,
    ) -> Result<Self, StoreCodecErrorV1> {
        if !matches!(execution_version, 4 | 5) {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal start execution version",
            });
        }
        Ok(Self {
            execution_version,
            procedure_digest,
        })
    }

    pub fn execution_version(&self) -> u8 {
        self.execution_version
    }

    pub fn procedure_digest(&self) -> &Sha256Digest {
        &self.procedure_digest
    }

    fn validate(&self) -> Result<(), StoreCodecErrorV1> {
        if !matches!(self.execution_version, 4 | 5) {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal start execution version",
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
    graph_session_projection: Option<PersistedGraphTerminalSessionProjectionV2>,
    start_identity: Option<PersistedStartIdentityV1>,
    lookup_command: Option<PersistedDomainCommandV1>,
    response_context: Option<PersistedResponseContextV1>,
    public_terminal_envelope: Option<Value>,
    execution_flavor: crate::DurableExecutionFlavorV1,
}

impl PersistedTerminalReceiptV1 {
    /// Builds a legacy v0 terminal receipt without immutable replay projections.
    pub fn new(job: JobReceiptV1, result: PersistedTerminalResultV1) -> Self {
        Self {
            job,
            result,
            job_projection: None,
            session_projection: None,
            graph_session_projection: None,
            start_identity: None,
            lookup_command: None,
            response_context: None,
            public_terminal_envelope: None,
            execution_flavor: crate::DurableExecutionFlavorV1::LegacyV1,
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
            graph_session_projection: None,
            start_identity: None,
            lookup_command: None,
            response_context: None,
            public_terminal_envelope: None,
            execution_flavor: crate::DurableExecutionFlavorV1::LegacyV1,
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

    pub fn graph_session_projection(&self) -> Option<&PersistedGraphTerminalSessionProjectionV2> {
        self.graph_session_projection.as_ref()
    }

    pub fn new_with_graph_projection(
        job: JobReceiptV1,
        result: PersistedTerminalResultV1,
        job_projection: PersistedTerminalJobProjectionV1,
        graph_session_projection: PersistedGraphTerminalSessionProjectionV2,
    ) -> Result<Self, StoreCodecErrorV1> {
        let receipt = Self {
            job,
            result,
            job_projection: Some(job_projection),
            session_projection: None,
            graph_session_projection: Some(graph_session_projection),
            start_identity: None,
            lookup_command: None,
            response_context: None,
            public_terminal_envelope: None,
            execution_flavor: crate::DurableExecutionFlavorV1::ProcedureV2,
        };
        receipt.validate_v1_projections()?;
        Ok(receipt)
    }

    pub fn start_identity(&self) -> Option<&PersistedStartIdentityV1> {
        self.start_identity.as_ref()
    }

    pub fn lookup_command(&self) -> Option<&PersistedDomainCommandV1> {
        self.lookup_command.as_ref()
    }

    pub fn response_context(&self) -> Option<&PersistedResponseContextV1> {
        self.response_context.as_ref()
    }

    pub fn public_terminal_envelope(&self) -> Option<&Value> {
        self.public_terminal_envelope.as_ref()
    }

    pub const fn execution_flavor(&self) -> crate::DurableExecutionFlavorV1 {
        self.execution_flavor
    }

    pub(crate) fn with_procedure_v2_execution(mut self) -> Result<Self, StoreCodecErrorV1> {
        self.execution_flavor = crate::DurableExecutionFlavorV1::ProcedureV2;
        self.validate_v2_projection()?;
        Ok(self)
    }

    pub fn with_public_terminal_envelope(
        mut self,
        envelope: Value,
    ) -> Result<Self, StoreCodecErrorV1> {
        let schema = envelope.get("schema").and_then(Value::as_str);
        if !matches!(schema, Some("podway.output/v3" | "podway.error/v1")) {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "public terminal envelope",
            });
        }
        self.public_terminal_envelope = Some(envelope);
        self.validate_v4_projection()?;
        Ok(self)
    }

    pub fn with_response_context(
        mut self,
        context: PersistedResponseContextV1,
    ) -> Result<Self, StoreCodecErrorV1> {
        context.validate()?;
        if let Some(stored) = &self.response_context
            && stored != &context
        {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal response context",
            });
        }
        self.response_context = Some(context);
        self.validate_v3_projection()?;
        Ok(self)
    }

    pub fn with_lookup_command(
        mut self,
        command: PersistedDomainCommandV1,
    ) -> Result<Self, StoreCodecErrorV1> {
        if let Some(stored) = &self.lookup_command
            && stored != &command
        {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal lookup command",
            });
        }
        self.lookup_command = Some(command);
        self.validate_v2_projection()?;
        Ok(self)
    }

    pub fn with_start_identity(mut self, start_identity: PersistedStartIdentityV1) -> Self {
        self.start_identity = Some(start_identity);
        self
    }

    pub fn with_session_procedure_digest(mut self, procedure_digest: Sha256Digest) -> Self {
        if let Some(session_projection) = self.session_projection.take() {
            self.session_projection =
                Some(session_projection.with_procedure_digest(procedure_digest));
        }
        self
    }

    fn from_v1_envelope(
        job: JobReceiptV1,
        result: PersistedTerminalResultV1,
        job_projection: PersistedTerminalJobProjectionV1,
        session_projection: Option<PersistedTerminalSessionProjectionV1>,
        start_identity: Option<PersistedStartIdentityV1>,
    ) -> Result<Self, StoreCodecErrorV1> {
        let receipt = Self::new_with_projections(job, result, job_projection, session_projection)?;
        Ok(match start_identity {
            Some(start_identity) => receipt.with_start_identity(start_identity),
            None => receipt,
        })
    }

    fn from_v2_envelope(
        job: JobReceiptV1,
        result: PersistedTerminalResultV1,
        job_projection: PersistedTerminalJobProjectionV1,
        session_projection: Option<PersistedTerminalSessionProjectionV1>,
        start_identity: Option<PersistedStartIdentityV1>,
        lookup_command: PersistedDomainCommandV1,
    ) -> Result<Self, StoreCodecErrorV1> {
        let receipt = Self::from_v1_envelope(
            job,
            result,
            job_projection,
            session_projection,
            start_identity,
        )?;
        receipt.with_lookup_command(lookup_command)
    }

    fn from_v3_envelope(
        job: JobReceiptV1,
        result: PersistedTerminalResultV1,
        job_projection: PersistedTerminalJobProjectionV1,
        session_projection: Option<PersistedTerminalSessionProjectionV1>,
        start_identity: Option<PersistedStartIdentityV1>,
        lookup_command: PersistedDomainCommandV1,
        response_context: PersistedResponseContextV1,
    ) -> Result<Self, StoreCodecErrorV1> {
        Self::from_v2_envelope(
            job,
            result,
            job_projection,
            session_projection,
            start_identity,
            lookup_command,
        )?
        .with_response_context(response_context)
    }

    fn from_v4_envelope(envelope: TerminalEnvelopeV4) -> Result<Self, StoreCodecErrorV1> {
        Self::from_v3_envelope(
            envelope.job.into(),
            envelope.result,
            envelope.job_projection,
            envelope.session_projection,
            envelope.start_identity,
            envelope.command,
            envelope.response_context,
        )?
        .with_public_terminal_envelope(envelope.public_terminal_envelope)
    }

    fn validate_legacy_projections(&self) -> Result<(), StoreCodecErrorV1> {
        if let Some(start_identity) = &self.start_identity {
            start_identity.validate()?;
        }
        if self.job_projection.is_some()
            || self.session_projection.is_some()
            || self.graph_session_projection.is_some()
            || self.lookup_command.is_some()
            || self.response_context.is_some()
            || self.public_terminal_envelope.is_some()
            || self.execution_flavor != crate::DurableExecutionFlavorV1::LegacyV1
        {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal replay projections",
            });
        }
        Ok(())
    }

    fn validate_v1_projections(&self) -> Result<(), StoreCodecErrorV1> {
        if let Some(start_identity) = &self.start_identity {
            start_identity.validate()?;
        }
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

        if self.session_projection.is_some() && self.graph_session_projection.is_some() {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal session projection",
            });
        }
        match self.execution_flavor {
            crate::DurableExecutionFlavorV1::LegacyV1 => {
                if self.graph_session_projection.is_some() {
                    return Err(StoreCodecErrorV1::InvalidValue {
                        field: "Procedure v2 terminal execution flavor",
                    });
                }
            }
            crate::DurableExecutionFlavorV1::ProcedureV2 => {
                let graph_receipt = self.graph_session_projection.is_some();
                let non_graph_receipt = self.session_projection.is_none()
                    && self.start_identity.is_none()
                    && self
                        .lookup_command
                        .as_ref()
                        .is_some_and(|command| procedure_v2_runtime_command(&command.command()));
                if !graph_receipt && !non_graph_receipt {
                    return Err(StoreCodecErrorV1::InvalidValue {
                        field: "Procedure v2 terminal execution flavor",
                    });
                }
            }
        }
        if let Some(graph) = &self.graph_session_projection {
            graph.validate()?;
            match (graph.operation(), &self.result) {
                (
                    None
                    | Some(PersistedGraphTerminalOperationV2::GoalDefine { .. })
                    | Some(PersistedGraphTerminalOperationV2::GoalRevise { .. })
                    | Some(PersistedGraphTerminalOperationV2::GoalAssessCriterion { .. })
                    | Some(PersistedGraphTerminalOperationV2::Decide { .. })
                    | Some(PersistedGraphTerminalOperationV2::Rework { .. })
                    | Some(PersistedGraphTerminalOperationV2::Complete { .. })
                    | Some(PersistedGraphTerminalOperationV2::Retry { .. })
                    | Some(PersistedGraphTerminalOperationV2::Skip { .. })
                    | Some(PersistedGraphTerminalOperationV2::Block { .. })
                    | Some(PersistedGraphTerminalOperationV2::Unblock { .. })
                    | Some(PersistedGraphTerminalOperationV2::Cancel { .. })
                    | Some(PersistedGraphTerminalOperationV2::Reset { .. }),
                    PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                        session_id,
                        revision_before,
                        revision_after,
                        changed,
                    }),
                ) if graph.session_id() == session_id
                    && graph.revision_before() == *revision_before
                    && graph.revision_after() == *revision_after
                    && *changed => {}
                (
                    Some(PersistedGraphTerminalOperationV2::ItemMutation { item_id, .. }),
                    PersistedTerminalResultV1::Success(PersistedDomainResultV1::ItemChanged {
                        session_id,
                        item_id: result_item_id,
                        revision_before,
                        revision_after,
                        changed,
                    }),
                ) if graph.session_id() == session_id
                    && item_id == result_item_id
                    && graph.revision_before() == *revision_before
                    && graph.revision_after() == *revision_after
                    && *changed == (*revision_before != *revision_after) => {}
                (
                    Some(PersistedGraphTerminalOperationV2::Failure { .. }),
                    PersistedTerminalResultV1::Failure(_),
                ) if graph.revision_before() == graph.revision_after() => {}
                _ => {
                    return Err(StoreCodecErrorV1::InvalidValue {
                        field: "Procedure v2 terminal session projection",
                    });
                }
            }
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
            ) if self.graph_session_projection.is_some() => {}
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

    fn validate_v2_projection(&self) -> Result<(), StoreCodecErrorV1> {
        self.validate_v1_projections()?;
        let command = self
            .lookup_command
            .as_ref()
            .ok_or(StoreCodecErrorV1::InvalidValue {
                field: "terminal lookup command",
            })?
            .command();
        let graph_reset =
            command == CommandV1::SessionReset && persisted_graph_reset_receipt_is_exact_v2(self);
        if !graph_reset {
            validate_persisted_terminal_result_for_command_v1(&command, &self.result)?;
        }
        Ok(())
    }

    fn validate_v3_projection(&self) -> Result<(), StoreCodecErrorV1> {
        self.validate_v2_projection()?;
        let context = self
            .response_context
            .as_ref()
            .ok_or(StoreCodecErrorV1::InvalidValue {
                field: "terminal response context",
            })?;
        context.validate()?;
        let command = self
            .lookup_command
            .as_ref()
            .ok_or(StoreCodecErrorV1::InvalidValue {
                field: "terminal lookup command",
            })?;
        if context.command() != command.public_command_name() {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "terminal response command",
            });
        }
        Ok(())
    }

    fn validate_v4_projection(&self) -> Result<(), StoreCodecErrorV1> {
        self.validate_v3_projection()?;
        let schema = self
            .public_terminal_envelope
            .as_ref()
            .and_then(|envelope| envelope.get("schema"))
            .and_then(Value::as_str);
        if !matches!(schema, Some("podway.output/v3" | "podway.error/v1")) {
            return Err(StoreCodecErrorV1::InvalidValue {
                field: "public terminal envelope",
            });
        }
        Ok(())
    }
}

pub(crate) fn persisted_graph_reset_receipt_is_exact_v2(
    receipt: &PersistedTerminalReceiptV1,
) -> bool {
    matches!(
        (receipt.graph_session_projection(), receipt.result()),
        (
            Some(graph),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                session_id: result_session_id,
                revision_before,
                revision_after,
                changed: true,
            }),
        ) if matches!(
            graph.operation(),
            Some(PersistedGraphTerminalOperationV2::Reset { session_id })
                if session_id == graph.session_id() && session_id == result_session_id
        )
            && graph.revision_before() == *revision_before
            && graph.revision_after() == *revision_after
            && *revision_before == *revision_after
            && *revision_before > RevisionV1::ZERO
    )
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_identity: Option<PersistedStartIdentityV1>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_identity: Option<PersistedStartIdentityV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalEnvelopeV2 {
    schema: String,
    command: PersistedDomainCommandV1,
    job: PersistedJobReceiptV1,
    job_projection: PersistedTerminalJobProjectionV1,
    result: PersistedTerminalResultV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_projection: Option<PersistedTerminalSessionProjectionV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_identity: Option<PersistedStartIdentityV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalEnvelopeV3 {
    schema: String,
    command: PersistedDomainCommandV1,
    job: PersistedJobReceiptV1,
    job_projection: PersistedTerminalJobProjectionV1,
    result: PersistedTerminalResultV1,
    response_context: PersistedResponseContextV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_projection: Option<PersistedTerminalSessionProjectionV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_identity: Option<PersistedStartIdentityV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalEnvelopeV4 {
    schema: String,
    command: PersistedDomainCommandV1,
    job: PersistedJobReceiptV1,
    job_projection: PersistedTerminalJobProjectionV1,
    result: PersistedTerminalResultV1,
    response_context: PersistedResponseContextV1,
    public_terminal_envelope: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_projection: Option<PersistedTerminalSessionProjectionV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_identity: Option<PersistedStartIdentityV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalEnvelopeV5 {
    schema: String,
    execution_flavor: String,
    command: PersistedDomainCommandV1,
    job: PersistedJobReceiptV1,
    job_projection: PersistedTerminalJobProjectionV1,
    result: PersistedTerminalResultV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response_context: Option<PersistedResponseContextV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public_terminal_envelope: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_projection: Option<PersistedTerminalSessionProjectionV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_identity: Option<PersistedStartIdentityV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphTerminalEnvelopeV1 {
    schema: String,
    command: PersistedDomainCommandV1,
    job: PersistedJobReceiptV1,
    job_projection: PersistedTerminalJobProjectionV1,
    result: PersistedTerminalResultV1,
    response_context: PersistedResponseContextV1,
    graph_session_projection: PersistedGraphTerminalSessionProjectionV2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public_terminal_envelope: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_identity: Option<PersistedStartIdentityV1>,
}

type GraphTerminalEnvelopeV2 = GraphTerminalEnvelopeV1;
type GraphTerminalEnvelopeV3 = GraphTerminalEnvelopeV1;

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

    if let Some(graph_session_projection) = receipt.graph_session_projection() {
        receipt.validate_v3_projection()?;
        if let Some(public_terminal_envelope) = receipt.public_terminal_envelope() {
            receipt.validate_v4_projection()?;
            let _ = public_terminal_envelope;
        }
        return canonical_json(&GraphTerminalEnvelopeV1 {
            schema: if graph_session_projection.goal_defined() {
                STORE_GRAPH_TERMINAL_SCHEMA_V3
            } else if graph_session_projection.operation().is_some() {
                STORE_GRAPH_TERMINAL_SCHEMA_V2
            } else {
                STORE_GRAPH_TERMINAL_SCHEMA_V1
            }
            .to_owned(),
            command: receipt
                .lookup_command()
                .cloned()
                .ok_or(StoreCodecErrorV1::InvalidValue {
                    field: "terminal lookup command",
                })?,
            job: receipt.job().into(),
            job_projection: receipt.job_projection().cloned().ok_or(
                StoreCodecErrorV1::InvalidValue {
                    field: "terminal replay projections",
                },
            )?,
            result: receipt.result().clone(),
            response_context: receipt.response_context().cloned().ok_or(
                StoreCodecErrorV1::InvalidValue {
                    field: "terminal response context",
                },
            )?,
            graph_session_projection: graph_session_projection.clone(),
            public_terminal_envelope: receipt.public_terminal_envelope().cloned(),
            start_identity: receipt.start_identity().cloned(),
        });
    }

    if receipt.execution_flavor() == crate::DurableExecutionFlavorV1::ProcedureV2 {
        if receipt.public_terminal_envelope().is_some() {
            receipt.validate_v4_projection()?;
        } else if receipt.response_context().is_some() {
            receipt.validate_v3_projection()?;
        } else {
            receipt.validate_v2_projection()?;
        }
        return canonical_json(&TerminalEnvelopeV5 {
            schema: STORE_TERMINAL_SCHEMA_V5.to_owned(),
            execution_flavor: "procedure_v2".to_owned(),
            command: receipt
                .lookup_command()
                .cloned()
                .ok_or(StoreCodecErrorV1::InvalidValue {
                    field: "terminal lookup command",
                })?,
            job: receipt.job().into(),
            job_projection: receipt.job_projection().cloned().ok_or(
                StoreCodecErrorV1::InvalidValue {
                    field: "terminal replay projections",
                },
            )?,
            result: receipt.result().clone(),
            response_context: receipt.response_context().cloned(),
            public_terminal_envelope: receipt.public_terminal_envelope().cloned(),
            session_projection: receipt.session_projection().cloned(),
            start_identity: receipt.start_identity().cloned(),
        });
    }

    match (
        receipt.job_projection(),
        receipt.lookup_command(),
        receipt.response_context(),
        receipt.public_terminal_envelope(),
    ) {
        (
            Some(job_projection),
            Some(lookup_command),
            Some(response_context),
            Some(public_terminal_envelope),
        ) => {
            receipt.validate_v4_projection()?;
            canonical_json(&TerminalEnvelopeV4 {
                schema: STORE_TERMINAL_SCHEMA_V4.to_owned(),
                command: lookup_command.clone(),
                job: receipt.job().into(),
                job_projection: job_projection.clone(),
                result: receipt.result().clone(),
                response_context: response_context.clone(),
                public_terminal_envelope: public_terminal_envelope.clone(),
                session_projection: receipt.session_projection().cloned(),
                start_identity: receipt.start_identity().cloned(),
            })
        }
        (Some(job_projection), Some(lookup_command), Some(response_context), None) => {
            receipt.validate_v3_projection()?;
            canonical_json(&TerminalEnvelopeV3 {
                schema: STORE_TERMINAL_SCHEMA_V3.to_owned(),
                command: lookup_command.clone(),
                job: receipt.job().into(),
                job_projection: job_projection.clone(),
                result: receipt.result().clone(),
                response_context: response_context.clone(),
                session_projection: receipt.session_projection().cloned(),
                start_identity: receipt.start_identity().cloned(),
            })
        }
        (Some(job_projection), Some(lookup_command), None, None) => {
            receipt.validate_v2_projection()?;
            canonical_json(&TerminalEnvelopeV2 {
                schema: STORE_TERMINAL_SCHEMA_V2.to_owned(),
                command: lookup_command.clone(),
                job: receipt.job().into(),
                job_projection: job_projection.clone(),
                result: receipt.result().clone(),
                session_projection: receipt.session_projection().cloned(),
                start_identity: receipt.start_identity().cloned(),
            })
        }
        (Some(job_projection), None, None, None) => {
            receipt.validate_v1_projections()?;
            canonical_json(&TerminalEnvelopeV1 {
                schema: STORE_TERMINAL_SCHEMA_V1.to_owned(),
                job: receipt.job().into(),
                job_projection: job_projection.clone(),
                result: receipt.result().clone(),
                session_projection: receipt.session_projection().cloned(),
                start_identity: receipt.start_identity().cloned(),
            })
        }
        (None, None, None, None) => {
            receipt.validate_legacy_projections()?;
            canonical_json(&TerminalEnvelopeV0 {
                schema: STORE_TERMINAL_SCHEMA_V0.to_owned(),
                job: receipt.job().into(),
                result: receipt.result().clone(),
                start_identity: receipt.start_identity().cloned(),
            })
        }
        (None, Some(_), _, _)
        | (None, None, Some(_), _)
        | (None, None, None, Some(_))
        | (Some(_), None, Some(_), _)
        | (Some(_), None, None, Some(_))
        | (Some(_), Some(_), None, Some(_)) => Err(StoreCodecErrorV1::InvalidValue {
            field: "terminal lookup command",
        }),
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
    let legacy_goal_defined_absent = matches!(
        schema.as_str(),
        STORE_GRAPH_TERMINAL_SCHEMA_V1 | STORE_GRAPH_TERMINAL_SCHEMA_V2
    ) && document
        .get("graph_session_projection")
        .and_then(Value::as_object)
        .is_some_and(|projection| !projection.contains_key("goal_defined"));
    let legacy_goal_defined_explicit = matches!(
        schema.as_str(),
        STORE_GRAPH_TERMINAL_SCHEMA_V1 | STORE_GRAPH_TERMINAL_SCHEMA_V2
    ) && document
        .get("graph_session_projection")
        .and_then(Value::as_object)
        .and_then(|projection| projection.get("goal_defined"))
        .and_then(Value::as_bool)
        == Some(true);
    let receipt = match schema.as_str() {
        STORE_TERMINAL_SCHEMA_V0 => {
            let envelope: TerminalEnvelopeV0 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            if envelope.job.identity_sequence == 0 {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "identity sequence",
                });
            }
            let receipt = PersistedTerminalReceiptV1::new(envelope.job.into(), envelope.result);
            match envelope.start_identity {
                Some(start_identity) => receipt.with_start_identity(start_identity),
                None => receipt,
            }
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
                envelope.start_identity,
            )?
        }
        STORE_TERMINAL_SCHEMA_V2 => {
            let envelope: TerminalEnvelopeV2 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            if envelope.job.identity_sequence == 0 {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "identity sequence",
                });
            }
            PersistedTerminalReceiptV1::from_v2_envelope(
                envelope.job.into(),
                envelope.result,
                envelope.job_projection,
                envelope.session_projection,
                envelope.start_identity,
                envelope.command,
            )?
        }
        STORE_TERMINAL_SCHEMA_V3 => {
            let envelope: TerminalEnvelopeV3 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            if envelope.job.identity_sequence == 0 {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "identity sequence",
                });
            }
            PersistedTerminalReceiptV1::from_v3_envelope(
                envelope.job.into(),
                envelope.result,
                envelope.job_projection,
                envelope.session_projection,
                envelope.start_identity,
                envelope.command,
                envelope.response_context,
            )?
        }
        STORE_TERMINAL_SCHEMA_V4 => {
            let envelope: TerminalEnvelopeV4 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            if envelope.job.identity_sequence == 0 {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "identity sequence",
                });
            }
            PersistedTerminalReceiptV1::from_v4_envelope(envelope)?
        }
        STORE_TERMINAL_SCHEMA_V5 => {
            let envelope: TerminalEnvelopeV5 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            if envelope.execution_flavor != "procedure_v2" || envelope.job.identity_sequence == 0 {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "Procedure v2 terminal execution flavor",
                });
            }
            let mut receipt = PersistedTerminalReceiptV1::new_with_projections(
                envelope.job.into(),
                envelope.result,
                envelope.job_projection,
                envelope.session_projection,
            )?
            .with_lookup_command(envelope.command)?;
            if let Some(start_identity) = envelope.start_identity {
                receipt = receipt.with_start_identity(start_identity);
            }
            if let Some(response_context) = envelope.response_context {
                receipt = receipt.with_response_context(response_context)?;
            }
            if let Some(public_terminal_envelope) = envelope.public_terminal_envelope {
                receipt = receipt.with_public_terminal_envelope(public_terminal_envelope)?;
            }
            receipt.with_procedure_v2_execution()?
        }
        STORE_GRAPH_TERMINAL_SCHEMA_V1 => {
            let envelope: GraphTerminalEnvelopeV1 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            if envelope.job.identity_sequence == 0 {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "identity sequence",
                });
            }
            if envelope.graph_session_projection.operation().is_some() {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "Procedure v2 terminal operation projection",
                });
            }
            let mut receipt = PersistedTerminalReceiptV1::new_with_graph_projection(
                envelope.job.into(),
                envelope.result,
                envelope.job_projection,
                envelope.graph_session_projection,
            )?
            .with_lookup_command(envelope.command)?
            .with_response_context(envelope.response_context)?;
            if let Some(start_identity) = envelope.start_identity {
                receipt = receipt.with_start_identity(start_identity);
            }
            if let Some(public_terminal_envelope) = envelope.public_terminal_envelope {
                receipt = receipt.with_public_terminal_envelope(public_terminal_envelope)?;
            }
            receipt
        }
        STORE_GRAPH_TERMINAL_SCHEMA_V2 => {
            let envelope: GraphTerminalEnvelopeV2 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            if envelope.job.identity_sequence == 0
                || envelope.graph_session_projection.operation().is_none()
            {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "Procedure v2 terminal operation projection",
                });
            }
            let mut receipt = PersistedTerminalReceiptV1::new_with_graph_projection(
                envelope.job.into(),
                envelope.result,
                envelope.job_projection,
                envelope.graph_session_projection,
            )?
            .with_lookup_command(envelope.command)?
            .with_response_context(envelope.response_context)?;
            if let Some(start_identity) = envelope.start_identity {
                receipt = receipt.with_start_identity(start_identity);
            }
            if let Some(public_terminal_envelope) = envelope.public_terminal_envelope {
                receipt = receipt.with_public_terminal_envelope(public_terminal_envelope)?;
            }
            receipt
        }
        STORE_GRAPH_TERMINAL_SCHEMA_V3 => {
            let envelope: GraphTerminalEnvelopeV3 =
                serde_json::from_value(document).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
            if envelope.job.identity_sequence == 0
                || !envelope.graph_session_projection.goal_defined()
            {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "Procedure v2 goal terminal projection",
                });
            }
            let mut receipt = PersistedTerminalReceiptV1::new_with_graph_projection(
                envelope.job.into(),
                envelope.result,
                envelope.job_projection,
                envelope.graph_session_projection,
            )?
            .with_lookup_command(envelope.command)?
            .with_response_context(envelope.response_context)?;
            if let Some(start_identity) = envelope.start_identity {
                receipt = receipt.with_start_identity(start_identity);
            }
            if let Some(public_terminal_envelope) = envelope.public_terminal_envelope {
                receipt = receipt.with_public_terminal_envelope(public_terminal_envelope)?;
            }
            receipt
        }
        found => {
            return Err(StoreCodecErrorV1::UnsupportedSchema {
                expected: STORE_TERMINAL_SCHEMA_V5,
                found: found.to_owned(),
            });
        }
    };
    let goal_operation = receipt
        .graph_session_projection()
        .and_then(PersistedGraphTerminalSessionProjectionV2::operation)
        .is_some_and(|operation| {
            matches!(
                operation,
                PersistedGraphTerminalOperationV2::GoalDefine { .. }
                    | PersistedGraphTerminalOperationV2::GoalRevise { .. }
                    | PersistedGraphTerminalOperationV2::GoalAssessCriterion { .. }
            )
        });
    let goal_bearing_start = matches!(
        receipt.lookup_command(),
        Some(
            PersistedDomainCommandV1::SessionStart | PersistedDomainCommandV1::SessionStartReplace
        )
    ) && receipt
        .public_terminal_envelope()
        .and_then(|envelope| envelope.get("result"))
        .and_then(|result| result.get("goal_defined"))
        .and_then(Value::as_bool)
        == Some(true);
    let legacy_goal_defined_allowed =
        legacy_goal_defined_absent && !goal_operation && !goal_bearing_start;
    let encoded = encode_persisted_terminal_receipt_v1(&receipt)?;
    let legacy_canonical = if legacy_goal_defined_allowed {
        let mut document: Value =
            serde_json::from_str(&encoded).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
        if let Some(projection) = document
            .get_mut("graph_session_projection")
            .and_then(Value::as_object_mut)
        {
            projection.remove("goal_defined");
        }
        Some(
            serde_json::to_string(&canonicalize_json(document))
                .map_err(|_| StoreCodecErrorV1::InvalidJson)?,
        )
    } else {
        None
    };
    let legacy_explicit_canonical = if legacy_goal_defined_explicit {
        let mut document: Value =
            serde_json::from_str(&encoded).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
        document
            .as_object_mut()
            .ok_or(StoreCodecErrorV1::InvalidJson)?
            .insert("schema".to_owned(), Value::String(schema));
        Some(
            serde_json::to_string(&canonicalize_json(document))
                .map_err(|_| StoreCodecErrorV1::InvalidJson)?,
        )
    } else {
        None
    };
    if encoded != value
        && legacy_canonical.as_deref() != Some(value)
        && legacy_explicit_canonical.as_deref() != Some(value)
    {
        return Err(StoreCodecErrorV1::InvalidValue {
            field: "canonical terminal receipt",
        });
    }
    Ok(receipt)
}

/// Promotes a predecessor terminal receipt's frozen public success envelope to the
/// current v3 schema before the v2-only schema migration verifies the receipt.
///
/// This is deliberately migration-only. Normal decoding continues to reject every
/// public success schema except `podway.output/v3`.
pub(crate) fn normalize_terminal_receipt_for_schema_v4_v1(
    value: &str,
    authoritative_execution_flavor: Option<crate::DurableExecutionFlavorV1>,
) -> Result<(PersistedTerminalReceiptV1, String), StoreCodecErrorV1> {
    let mut document: Value =
        serde_json::from_str(value).map_err(|_| StoreCodecErrorV1::InvalidJson)?;
    let envelope = document
        .get_mut("public_terminal_envelope")
        .and_then(Value::as_object_mut);
    if let Some(envelope) = envelope {
        match envelope.get("schema").and_then(Value::as_str) {
            Some("podway.output/v1" | "podway.output/v2") => {
                envelope.insert(
                    "schema".to_owned(),
                    Value::String("podway.output/v3".to_owned()),
                );
            }
            Some("podway.output/v3" | "podway.error/v1") => {}
            _ => {
                return Err(StoreCodecErrorV1::InvalidValue {
                    field: "public terminal envelope",
                });
            }
        }
    }
    let normalized = serde_json::to_string(&canonicalize_json(document))
        .map_err(|_| StoreCodecErrorV1::InvalidJson)?;
    let mut receipt = decode_terminal_receipt_v1(&normalized)?;
    let normalized = if receipt.execution_flavor() == crate::DurableExecutionFlavorV1::LegacyV1
        && authoritative_execution_flavor == Some(crate::DurableExecutionFlavorV1::ProcedureV2)
    {
        receipt = receipt.with_procedure_v2_execution()?;
        encode_persisted_terminal_receipt_v1(&receipt)?
    } else {
        normalized
    };

    if let (Some(context), Some(envelope)) = (
        receipt.response_context(),
        receipt.public_terminal_envelope(),
    ) && (envelope.get("request_id").and_then(Value::as_str) != Some(context.request_id())
        || envelope.get("command").and_then(Value::as_str) != Some(context.command()))
    {
        return Err(StoreCodecErrorV1::InvalidValue {
            field: "public terminal envelope correlation",
        });
    }
    Ok((receipt, normalized))
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
            | CommandV1::SessionDecide
            | CommandV1::SessionRework
            | CommandV1::GoalDefine
            | CommandV1::GoalRevise
            | CommandV1::GoalAssessCriterion
            | CommandV1::SessionSkip
            | CommandV1::SessionRetry
            | CommandV1::SessionBlock
            | CommandV1::SessionUnblock
            | CommandV1::SessionCancel,
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

#[cfg(test)]
mod tests {
    use super::*;
    use podway_core::{GraphNodeId, JobId, SessionId, WorkspaceId};
    use serde_json::json;

    fn digest(nibble: char) -> Sha256Digest {
        Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64))).unwrap()
    }

    #[test]
    fn schema_v4_migration_promotes_frozen_v2_graph_success_to_output_v3() {
        let session_id = SessionId::new("00000000-0000-4000-8000-000000000101").unwrap();
        let receipt = PersistedTerminalReceiptV1::new_with_graph_projection(
            JobReceiptV1::new(
                1,
                JobId::new("00000000-0000-4000-8000-000000000102").unwrap(),
                digest('a'),
            ),
            PersistedTerminalResultV1::Success(PersistedDomainResultV1::SessionChanged {
                session_id: session_id.clone(),
                revision_before: RevisionV1::ZERO,
                revision_after: RevisionV1::new(1),
                changed: true,
            }),
            PersistedTerminalJobProjectionV1::new(
                PersistedTerminalJobStateV1::Succeeded,
                EpochMillisV1::new(1),
                Some(EpochMillisV1::new(1)),
                EpochMillisV1::new(2),
            )
            .unwrap(),
            PersistedGraphTerminalSessionProjectionV2::new(
                session_id,
                "Graph start".to_owned(),
                PersistedSessionLifecycleV1::Running,
                RevisionV1::ZERO,
                RevisionV1::new(1),
                digest('b'),
                GraphNodeId::new("work").unwrap(),
                false,
                false,
            )
            .unwrap(),
        )
        .unwrap()
        .with_lookup_command(PersistedDomainCommandV1::SessionStart)
        .unwrap()
        .with_response_context(
            PersistedResponseContextV1::new(
                "00000000-0000-4000-8000-000000000103",
                "session.start",
                WorkspaceId::new("00000000-0000-4000-8000-000000000104").unwrap(),
                "/fixture/worktree",
                1,
            )
            .unwrap()
            .with_frozen_public_terminal_envelope(),
        )
        .unwrap()
        .with_public_terminal_envelope(json!({
            "schema": "podway.output/v3",
            "request_id": "00000000-0000-4000-8000-000000000103",
            "command": "session.start"
        }))
        .unwrap();
        let encoded = encode_persisted_terminal_receipt_v1(&receipt).unwrap();
        let mut predecessor: Value = serde_json::from_str(&encoded).unwrap();
        predecessor["public_terminal_envelope"]["schema"] = json!("podway.output/v2");
        let predecessor = serde_json::to_string(&canonicalize_json(predecessor)).unwrap();

        assert!(decode_terminal_receipt_v1(&predecessor).is_err());
        let (promoted, normalized) =
            normalize_terminal_receipt_for_schema_v4_v1(&predecessor, None).unwrap();
        assert_eq!(
            promoted.public_terminal_envelope().unwrap()["schema"],
            "podway.output/v3"
        );
        assert_eq!(decode_terminal_receipt_v1(&normalized).unwrap(), promoted);
    }

    #[test]
    fn schema_v4_migration_marks_start_cancellation_only_with_authoritative_v2_execution() {
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            JobReceiptV1::new(
                1,
                JobId::new("00000000-0000-4000-8000-000000000201").unwrap(),
                digest('c'),
            ),
            PersistedTerminalResultV1::Cancelled,
            PersistedTerminalJobProjectionV1::new(
                PersistedTerminalJobStateV1::Cancelled,
                EpochMillisV1::new(1),
                None,
                EpochMillisV1::new(2),
            )
            .unwrap(),
            None,
        )
        .unwrap()
        .with_lookup_command(PersistedDomainCommandV1::SessionStart)
        .unwrap();
        let predecessor = encode_persisted_terminal_receipt_v1(&receipt).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&predecessor).unwrap()["schema"],
            STORE_TERMINAL_SCHEMA_V2
        );

        let (ambiguous, unchanged) =
            normalize_terminal_receipt_for_schema_v4_v1(&predecessor, None).unwrap();
        assert_eq!(
            ambiguous.execution_flavor(),
            crate::DurableExecutionFlavorV1::LegacyV1
        );
        assert_eq!(unchanged, predecessor);

        let (promoted, normalized) = normalize_terminal_receipt_for_schema_v4_v1(
            &predecessor,
            Some(crate::DurableExecutionFlavorV1::ProcedureV2),
        )
        .unwrap();
        assert_eq!(
            promoted.execution_flavor(),
            crate::DurableExecutionFlavorV1::ProcedureV2
        );
        assert_eq!(
            serde_json::from_str::<Value>(&normalized).unwrap()["schema"],
            STORE_TERMINAL_SCHEMA_V5
        );
        assert_eq!(decode_terminal_receipt_v1(&normalized).unwrap(), promoted);
    }

    #[test]
    fn schema_v4_migration_keeps_v1_start_cancellation_legacy() {
        let receipt = PersistedTerminalReceiptV1::new_with_projections(
            JobReceiptV1::new(
                1,
                JobId::new("00000000-0000-4000-8000-000000000202").unwrap(),
                digest('d'),
            ),
            PersistedTerminalResultV1::Cancelled,
            PersistedTerminalJobProjectionV1::new(
                PersistedTerminalJobStateV1::Cancelled,
                EpochMillisV1::new(1),
                None,
                EpochMillisV1::new(2),
            )
            .unwrap(),
            None,
        )
        .unwrap()
        .with_lookup_command(PersistedDomainCommandV1::SessionStart)
        .unwrap()
        .with_start_identity(PersistedStartIdentityV1::new(4, digest('e')).unwrap());
        let predecessor = encode_persisted_terminal_receipt_v1(&receipt).unwrap();

        let (decoded, normalized) = normalize_terminal_receipt_for_schema_v4_v1(
            &predecessor,
            Some(crate::DurableExecutionFlavorV1::LegacyV1),
        )
        .unwrap();
        assert_eq!(
            decoded.execution_flavor(),
            crate::DurableExecutionFlavorV1::LegacyV1
        );
        assert_eq!(normalized, predecessor);
    }
}
