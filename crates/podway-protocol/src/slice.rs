use crate::IdempotencyKeyV1;
use podway_core::{
    AttemptId, BlockerId, CheckResultExecutorV2, CheckResultInputBasisV2, CheckResultOutcomeV2,
    CheckResultValueV2, CriterionId, GraphNodeId, ItemId, OperationId, OptionId, Revision,
    SessionId, Sha256Digest, WorkspaceId, canonicalize_json_v1,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::str::FromStr;
pub const MAX_SLICE_TASK_TITLE_SCALARS_V1: usize = 500;
pub const MAX_SLICE_REASON_SCALARS_V1: usize = 4_000;
/// Maximum Unicode scalars for a bounded lifecycle assertion such as a progress summary,
/// disposition summary, or disposition reason.
pub const MAX_SLICE_LIFECYCLE_TEXT_SCALARS_V1: usize = 4_000;
pub const MAX_SLICE_ITEM_TEXT_SCALARS_V1: usize = podway_core::MAX_TEXT_SCALARS_V2 as usize;
pub const MAX_SLICE_ARTIFACT_PATH_SCALARS_V1: usize = 4_000;
pub const MAX_SLICE_MEDIA_TYPE_BYTES_V1: usize = 255;
pub const ITEM_RECORD_MANY_INPUT_SCHEMA_V1: &str = "podway.item-record-many-input/v1";
pub const MAX_ITEM_RECORD_MANY_OPERATIONS_V1: usize = podway_core::MAX_ITEMS_PER_DEFINITION_V2;
pub const MAX_ITEM_RECORD_MANY_LIST_ENTRIES_V1: usize = podway_core::MAX_LIST_ENTRIES_V2 as usize;
pub const MAX_ITEM_RECORD_MANY_LIST_ENTRY_SCALARS_V1: usize =
    podway_core::MAX_LIST_ENTRY_SCALARS_V2 as usize;
pub const MAX_ITEM_RECORD_MANY_CHOICE_SCALARS_V1: usize = 120;

/// Validation failures for direct command input and semantic identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SliceErrorV1 {
    ValueTooLong {
        field: &'static str,
        maximum: usize,
        actual: usize,
    },
    BoundExceeded {
        field: &'static str,
        maximum: usize,
        actual: usize,
        unit: &'static str,
    },
    EmptyValue {
        field: &'static str,
    },
    InvalidValue {
        field: &'static str,
    },
    InvalidPayload {
        message: String,
    },
    NotAMutation {
        command: &'static str,
    },
    Canonicalization {
        message: String,
    },
}
impl fmt::Display for SliceErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ValueTooLong {
                field,
                maximum,
                actual,
            } => write!(
                f,
                "{field} exceeds its maximum of {maximum} (received {actual})"
            ),
            Self::BoundExceeded {
                field,
                maximum,
                actual,
                unit,
            } => write!(
                f,
                "{field} exceeds its maximum of {maximum} {unit} (received {actual})"
            ),
            Self::EmptyValue { field } => write!(f, "{field} must not be empty"),
            Self::InvalidValue { field } => write!(f, "invalid {field}"),
            Self::InvalidPayload { message } => write!(f, "invalid command payload: {message}"),
            Self::NotAMutation { command } => write!(f, "{command} is not a mutation"),
            Self::Canonicalization { message } => {
                write!(f, "cannot canonicalize mutation identity: {message}")
            }
        }
    }
}
impl std::error::Error for SliceErrorV1 {}

/// Optimistic-concurrency facts required for an item mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemMutationPreconditionsWireV1 {
    pub expected_session_id: SessionId,
    pub expected_attempt_id: AttemptId,
    pub expected_item_revision: Revision,
}

/// Optimistic-concurrency facts required for a session mutation that changes an attempt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMutationPreconditionsWireV1 {
    pub expected_session_id: SessionId,
    pub expected_session_revision: Revision,
    pub expected_attempt_id: AttemptId,
}

/// Optimistic-concurrency facts required when a command targets a particular session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionIdentityPreconditionsWireV1 {
    pub expected_session_id: SessionId,
    pub expected_session_revision: Revision,
}

/// Current workspace facts required by `workspace.reset_all`.
///
/// Selector UUID hints and this semantic assertion must agree when either is supplied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceResetAllPreconditionsWireV1 {
    pub expected_workspace_id: Option<WorkspaceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceInitV1 {
    pub repair: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRepairV1 {}

/// The exclusive source of a new session procedure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStartSourceV1 {
    Preset { preset: String },
    Procedure { procedure: String },
}

/// Validated inputs shared by `session.start` and `session.start_replace`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionStartV1 {
    pub source: SessionStartSourceV1,
    pub expected_procedure_digest: Option<Sha256Digest>,
    pub task_title: String,
    pub dry_run: bool,
}

/// The plain-start command variant carrying its optional paired session fences.
///
/// `SessionStartV1` stays fence-free because `SessionStartReplaceV1` embeds it
/// with its own required preconditions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionStartCommandV1 {
    pub start: SessionStartV1,
    pub preconditions: Option<SessionIdentityPreconditionsWireV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionStartReplaceV1 {
    pub start: SessionStartV1,
    pub mode: SessionDeletionModeV1,
    pub preconditions: SessionIdentityPreconditionsWireV1,
}

/// The only two deletion authorities accepted for reset and replacement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionDeletionModeV1 {
    Eligible,
    Force {
        progress_summary: String,
    },
    Delete {
        confirmed: bool,
        progress_summary: Option<String>,
    },
    Supersede {
        reason: String,
        actor: Option<String>,
    },
    DisposeAndArchive {
        disposition: TerminalDispositionInputV1,
        actor: Option<String>,
    },
    #[doc(hidden)]
    LegacyConfirmed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DelegationAcceptV1 {
    pub input: Value,
    pub preconditions: SessionMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemCheckV1 {
    pub item_id: ItemId,
    pub preconditions: ItemMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemUncheckV1 {
    pub item_id: ItemId,
    pub preconditions: ItemMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemSetV1 {
    pub item_id: ItemId,
    pub value: String,
    pub preconditions: ItemMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemAddV1 {
    pub item_id: ItemId,
    pub value: String,
    pub preconditions: ItemMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemRemoveV1 {
    pub item_id: ItemId,
    pub value: String,
    pub ignore_missing: bool,
    pub preconditions: ItemMutationPreconditionsWireV1,
}

/// A local artifact is verified by the daemon after resolving its worktree-relative path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemAttachSourceV1 {
    Path {
        path: String,
        media_type: Option<String>,
    },
    OpaqueReference {
        reference: String,
        digest: Sha256Digest,
        size_bytes: u64,
        media_type: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemAttachV1 {
    pub item_id: ItemId,
    pub source: ItemAttachSourceV1,
    pub preconditions: ItemMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemClearV1 {
    pub item_id: ItemId,
    pub preconditions: ItemMutationPreconditionsWireV1,
}

/// One complete typed value accepted by atomic multi-item recording.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ItemRecordValueV1 {
    Confirm {
        value: bool,
    },
    Text {
        value: String,
    },
    Choice {
        value: String,
    },
    Integer {
        value: i64,
    },
    List {
        value: Vec<String>,
    },
    Artifact {
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        digest: Option<Sha256Digest>,
        #[serde(skip_serializing_if = "Option::is_none")]
        size_bytes: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
    CheckResult {
        #[serde(flatten)]
        value: CheckResultValueV2,
    },
}

/// Exactly one record value or clear disposition for an atomic item operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ItemRecordManyDispositionV1 {
    Record { record: ItemRecordValueV1 },
    Clear { clear: bool },
}

/// One item-local fence and mutation within a canonicalized atomic batch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemRecordManyOperationV1 {
    pub item_id: ItemId,
    pub expected_item_revision: Revision,
    #[serde(flatten)]
    pub disposition: ItemRecordManyDispositionV1,
}

/// Validated public stdin document consumed by `podway record --stdin`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemRecordManyInputV1 {
    pub schema: String,
    pub workspace_uuid: WorkspaceId,
    pub session_id: SessionId,
    pub session_revision: Revision,
    pub attempt_id: AttemptId,
    pub idempotency_key: IdempotencyKeyV1,
    pub operations: Vec<ItemRecordManyOperationV1>,
}

/// Typed direct input for one atomic multi-item mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemRecordManyV1 {
    pub operations: Vec<ItemRecordManyOperationV1>,
    pub preconditions: SessionMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionBlockV1 {
    pub reason: String,
    pub preconditions: SessionMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionUnblockV1 {
    pub blocker_id: Option<BlockerId>,
    pub all: bool,
    pub preconditions: SessionMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSkipV1 {
    pub reason: Option<String>,
    pub preconditions: SessionMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRetryV1 {
    pub reason: String,
    pub preconditions: SessionMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCompleteV1 {
    pub preconditions: SessionMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCancelV1 {
    pub reason: String,
    pub preconditions: SessionMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionResetV1 {
    pub mode: SessionDeletionModeV1,
    pub dry_run: bool,
    pub preconditions: SessionIdentityPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionArchiveV1 {
    pub preconditions: SessionIdentityPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionArchivePurgeV1 {
    pub session_id: SessionId,
    pub expected_session_revision: Revision,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceResetAllV1 {
    pub confirmed: bool,
    pub preconditions: WorkspaceResetAllPreconditionsWireV1,
}

/// One ordered criterion supplied while defining a Procedure v2 session goal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GoalCriterionWireV2 {
    pub criterion_id: CriterionId,
    pub statement: String,
}

/// Optional immutable revision 1 supplied with a Procedure v2 session start.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InitialGoalWireV2 {
    pub goal: String,
    pub criteria: Vec<GoalCriterionWireV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

/// Session and goal-revision fences for a Procedure v2 goal mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GoalRevisionPreconditionsWireV2 {
    pub expected_session_id: SessionId,
    pub expected_session_revision: Revision,
    pub expected_attempt_id: Option<AttemptId>,
    pub expected_goal_revision: u64,
}

/// Session fences for manual rework, whose active attempt is absent after completion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReworkPreconditionsWireV2 {
    pub expected_session_id: SessionId,
    pub expected_session_revision: Revision,
    pub expected_attempt_id: Option<AttemptId>,
}

/// A closed, bounded `session.decide` request payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionDecideV2 {
    pub option_id: OptionId,
    pub reason: String,
    pub actor: Option<String>,
    pub preconditions: SessionMutationPreconditionsWireV1,
    pub expected_goal_revision: Option<u64>,
}

/// A closed, bounded `session.rework` request payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionReworkV2 {
    pub target_graph_node_id: GraphNodeId,
    pub reason: String,
    pub actor: Option<String>,
    pub preconditions: ReworkPreconditionsWireV2,
}

/// A closed, bounded `goal.define` request payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GoalDefineV2 {
    pub goal: String,
    pub criteria: Vec<GoalCriterionWireV2>,
    pub actor: Option<String>,
    pub preconditions: SessionIdentityPreconditionsWireV1,
}

/// A closed, bounded `goal.revise` request payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GoalReviseV2 {
    pub goal: String,
    pub criteria: Vec<GoalCriterionWireV2>,
    pub target_graph_node_id: GraphNodeId,
    pub reason: String,
    pub actor: Option<String>,
    pub reactivate: bool,
    pub preconditions: GoalRevisionPreconditionsWireV2,
}

/// A closed, bounded `goal.assess_criterion` request payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GoalAssessCriterionV2 {
    pub criterion_id: CriterionId,
    pub status: String,
    pub reason: String,
    pub evidence: Vec<GraphNodeId>,
    pub items: Vec<ItemId>,
    pub actor: Option<String>,
    pub preconditions: SessionMutationPreconditionsWireV1,
    pub expected_goal_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionBeginV1 {
    pub initial_goal: Option<InitialGoalWireV2>,
    pub preconditions: SessionIdentityPreconditionsWireV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TerminalDispositionInputV1 {
    HandedOff { summary: String, reference: String },
    NotRequired { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionTerminalDispositionV1 {
    pub disposition: TerminalDispositionInputV1,
    pub actor: Option<String>,
    pub preconditions: SessionIdentityPreconditionsWireV1,
}

/// Typed request data for the Procedure v2 direct mutation boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureV2MutationCommandV1 {
    SessionBegin(SessionBeginV1),
    SessionTerminalDisposition(SessionTerminalDispositionV1),
    SessionDecide(SessionDecideV2),
    SessionRework(SessionReworkV2),
    GoalDefine(GoalDefineV2),
    GoalRevise(GoalReviseV2),
    GoalAssessCriterion(GoalAssessCriterionV2),
}
impl ItemRecordValueV1 {
    pub fn artifact_source(&self) -> Option<Result<ItemAttachSourceV1, SliceErrorV1>> {
        match self {
            Self::Artifact {
                path,
                reference,
                digest,
                size_bytes,
                media_type,
            } => Some(validated_attach_source(
                path.clone(),
                reference.clone(),
                digest.clone(),
                *size_bytes,
                media_type.clone(),
            )),
            _ => None,
        }
    }
}

impl ProcedureV2MutationCommandV1 {
    pub const fn command_name(&self) -> &'static str {
        match self {
            Self::SessionBegin(_) => "session.begin",
            Self::SessionTerminalDisposition(_) => "session.terminal_disposition",
            Self::SessionDecide(_) => "session.decide",
            Self::SessionRework(_) => "session.rework",
            Self::GoalDefine(_) => "goal.define",
            Self::GoalRevise(_) => "goal.revise",
            Self::GoalAssessCriterion(_) => "goal.assess_criterion",
        }
    }
}

/// Typed direct mutation data with canonical public route names.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceCommandV1 {
    WorkspaceInit(WorkspaceInitV1),
    WorkspaceRepair(WorkspaceRepairV1),
    SessionStart(SessionStartCommandV1),
    SessionStartReplace(SessionStartReplaceV1),
    DelegationAccept(DelegationAcceptV1),
    SessionComplete(SessionCompleteV1),
    SessionSkip(SessionSkipV1),
    SessionRetry(SessionRetryV1),
    SessionBlock(SessionBlockV1),
    SessionUnblock(SessionUnblockV1),
    SessionCancel(SessionCancelV1),
    SessionReset(SessionResetV1),
    SessionArchive(SessionArchiveV1),
    SessionArchivePurge(SessionArchivePurgeV1),
    WorkspaceResetAll(WorkspaceResetAllV1),
    ItemCheck(ItemCheckV1),
    ItemUncheck(ItemUncheckV1),
    ItemSet(ItemSetV1),
    ItemAdd(ItemAddV1),
    ItemRemove(ItemRemoveV1),
    ItemAttach(ItemAttachV1),
    ItemClear(ItemClearV1),
    ItemRecordMany(ItemRecordManyV1),
}
impl SliceCommandV1 {
    pub const fn command_name(&self) -> &'static str {
        match self {
            Self::WorkspaceInit(_) => "workspace.init",
            Self::WorkspaceRepair(_) => "workspace.repair",
            Self::SessionStart(_) => "session.start",
            Self::SessionStartReplace(_) => "session.start_replace",
            Self::DelegationAccept(_) => "delegation.accept",
            Self::SessionComplete(_) => "session.complete",
            Self::SessionSkip(_) => "session.skip",
            Self::SessionRetry(_) => "session.retry",
            Self::SessionBlock(_) => "session.block",
            Self::SessionUnblock(_) => "session.unblock",
            Self::SessionCancel(_) => "session.cancel",
            Self::SessionReset(_) => "session.reset",
            Self::SessionArchive(_) => "session.archive",
            Self::SessionArchivePurge(_) => "session.archive_purge",
            Self::WorkspaceResetAll(_) => "workspace.reset_all",
            Self::ItemCheck(_) => "item.check",
            Self::ItemUncheck(_) => "item.uncheck",
            Self::ItemSet(_) => "item.set",
            Self::ItemAdd(_) => "item.add",
            Self::ItemRemove(_) => "item.remove",
            Self::ItemAttach(_) => "item.attach",
            Self::ItemClear(_) => "item.clear",
            Self::ItemRecordMany(_) => "item.record_many",
        }
    }
    pub const fn is_mutation(&self) -> bool {
        is_direct_slice_mutation_v1(self)
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RawItemRecordValueV1 {
    Confirm {
        value: bool,
    },
    Text {
        value: String,
    },
    Choice {
        value: String,
    },
    Integer {
        value: i64,
    },
    List {
        value: Vec<String>,
    },
    Artifact {
        #[serde(default, deserialize_with = "deserialize_optional_non_null")]
        path: Option<String>,
        #[serde(default, deserialize_with = "deserialize_optional_non_null")]
        reference: Option<String>,
        #[serde(default, deserialize_with = "deserialize_optional_non_null")]
        digest: Option<Sha256Digest>,
        #[serde(default, deserialize_with = "deserialize_optional_non_null")]
        size_bytes: Option<u64>,
        #[serde(default, deserialize_with = "deserialize_optional_non_null")]
        media_type: Option<String>,
    },
    CheckResult {
        operation_id: OperationId,
        operation_digest: Sha256Digest,
        input_basis: RawCheckResultInputBasisV1,
        executor: RawCheckResultExecutorV1,
        outcome: String,
        summary: String,
        output_digest: Sha256Digest,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCheckResultInputBasisV1 {
    descriptor: String,
    digest: Sha256Digest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCheckResultExecutorV1 {
    name: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawItemRecordManyOperationV1 {
    item_id: ItemId,
    expected_item_revision: Revision,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    record: Option<RawItemRecordValueV1>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    clear: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawItemRecordManyInputV1 {
    schema: String,
    workspace_uuid: WorkspaceId,
    session_id: SessionId,
    session_revision: Revision,
    attempt_id: AttemptId,
    idempotency_key: IdempotencyKeyV1,
    operations: Vec<RawItemRecordManyOperationV1>,
}

/// Decodes only the closed, bounded stdin contract for atomic item recording.
pub fn decode_item_record_many_input_v1(
    bytes: &[u8],
) -> Result<ItemRecordManyInputV1, SliceErrorV1> {
    if bytes.len() > crate::MAX_DIRECT_INPUT_BYTES_V1 {
        return Err(SliceErrorV1::ValueTooLong {
            field: "item record input",
            maximum: crate::MAX_DIRECT_INPUT_BYTES_V1,
            actual: bytes.len(),
        });
    }
    let raw: RawItemRecordManyInputV1 =
        serde_json::from_slice(bytes).map_err(|error| SliceErrorV1::InvalidPayload {
            message: error.to_string(),
        })?;
    if raw.schema != ITEM_RECORD_MANY_INPUT_SCHEMA_V1 {
        return Err(SliceErrorV1::InvalidValue {
            field: "item record input schema",
        });
    }
    Ok(ItemRecordManyInputV1 {
        schema: raw.schema,
        workspace_uuid: raw.workspace_uuid,
        session_id: raw.session_id,
        session_revision: raw.session_revision,
        attempt_id: raw.attempt_id,
        idempotency_key: raw.idempotency_key,
        operations: validate_item_record_many_operations_v1(raw.operations)?,
    })
}

/// Validates and canonicalizes the operation array embedded in a atomic recording document.
pub fn decode_item_record_many_operations_v1(
    value: serde_json::Value,
) -> Result<Vec<ItemRecordManyOperationV1>, SliceErrorV1> {
    let operations: Vec<RawItemRecordManyOperationV1> =
        serde_json::from_value(value).map_err(|error| SliceErrorV1::InvalidPayload {
            message: error.to_string(),
        })?;
    validate_item_record_many_operations_v1(operations)
}

fn validate_item_record_many_operations_v1(
    operations: Vec<RawItemRecordManyOperationV1>,
) -> Result<Vec<ItemRecordManyOperationV1>, SliceErrorV1> {
    if operations.is_empty() {
        return Err(SliceErrorV1::InvalidValue {
            field: "item record operations",
        });
    }
    if operations.len() > MAX_ITEM_RECORD_MANY_OPERATIONS_V1 {
        return Err(SliceErrorV1::BoundExceeded {
            field: "item record operations",
            maximum: MAX_ITEM_RECORD_MANY_OPERATIONS_V1,
            actual: operations.len(),
            unit: "operations",
        });
    }
    let mut operations = operations
        .into_iter()
        .map(|operation| {
            let disposition = match (operation.record, operation.clear) {
                (Some(record), None) => ItemRecordManyDispositionV1::Record {
                    record: validate_item_record_value_v1(record)?,
                },
                (None, Some(true)) => ItemRecordManyDispositionV1::Clear { clear: true },
                _ => {
                    return Err(SliceErrorV1::InvalidValue {
                        field: "item record disposition",
                    });
                }
            };
            Ok(ItemRecordManyOperationV1 {
                item_id: operation.item_id,
                expected_item_revision: operation.expected_item_revision,
                disposition,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    operations.sort_by(|left, right| left.item_id.cmp(&right.item_id));
    if operations
        .windows(2)
        .any(|pair| pair[0].item_id == pair[1].item_id)
    {
        return Err(SliceErrorV1::InvalidValue {
            field: "item record item_id",
        });
    }
    Ok(operations)
}

fn validate_item_record_value_v1(
    record: RawItemRecordValueV1,
) -> Result<ItemRecordValueV1, SliceErrorV1> {
    Ok(match record {
        RawItemRecordValueV1::Confirm { value } => ItemRecordValueV1::Confirm { value },
        RawItemRecordValueV1::Text { value } => {
            validate_record_scalar_bound(
                &value,
                MAX_SLICE_ITEM_TEXT_SCALARS_V1,
                "item text value",
            )?;
            ItemRecordValueV1::Text { value }
        }
        RawItemRecordValueV1::Choice { value } => {
            validate_record_scalar_bound(
                &value,
                MAX_ITEM_RECORD_MANY_CHOICE_SCALARS_V1,
                "item choice value",
            )?;
            if value.is_empty() {
                return Err(SliceErrorV1::EmptyValue { field: "choice" });
            }
            ItemRecordValueV1::Choice { value }
        }
        RawItemRecordValueV1::Integer { value } => ItemRecordValueV1::Integer { value },
        RawItemRecordValueV1::List { value } => {
            if value.len() > MAX_ITEM_RECORD_MANY_LIST_ENTRIES_V1 {
                return Err(SliceErrorV1::BoundExceeded {
                    field: "item list entries",
                    maximum: MAX_ITEM_RECORD_MANY_LIST_ENTRIES_V1,
                    actual: value.len(),
                    unit: "entries",
                });
            }
            for entry in &value {
                validate_record_scalar_bound(
                    entry,
                    MAX_ITEM_RECORD_MANY_LIST_ENTRY_SCALARS_V1,
                    "item list entry",
                )?;
                if entry.is_empty() {
                    return Err(SliceErrorV1::EmptyValue {
                        field: "list entry",
                    });
                }
            }
            ItemRecordValueV1::List { value }
        }
        RawItemRecordValueV1::Artifact {
            path,
            reference,
            digest,
            size_bytes,
            media_type,
        } => match validated_attach_source(path, reference, digest, size_bytes, media_type)? {
            ItemAttachSourceV1::Path { path, media_type } => ItemRecordValueV1::Artifact {
                path: Some(path),
                reference: None,
                digest: None,
                size_bytes: None,
                media_type,
            },
            ItemAttachSourceV1::OpaqueReference {
                reference,
                digest,
                size_bytes,
                media_type,
            } => ItemRecordValueV1::Artifact {
                path: None,
                reference: Some(reference),
                digest: Some(digest),
                size_bytes: Some(size_bytes),
                media_type: Some(media_type),
            },
        },
        RawItemRecordValueV1::CheckResult {
            operation_id,
            operation_digest,
            input_basis,
            executor,
            outcome,
            summary,
            output_digest,
        } => {
            validate_record_scalar_bound(
                &input_basis.descriptor,
                podway_core::MAX_CHECK_RESULT_INPUT_DESCRIPTOR_SCALARS_V2,
                "check result input descriptor",
            )?;
            validate_record_scalar_bound(
                &executor.name,
                podway_core::MAX_CHECK_RESULT_EXECUTOR_NAME_SCALARS_V2,
                "check result executor name",
            )?;
            validate_record_scalar_bound(
                &executor.version,
                podway_core::MAX_CHECK_RESULT_EXECUTOR_VERSION_SCALARS_V2,
                "check result executor version",
            )?;
            validate_record_scalar_bound(
                &summary,
                podway_core::MAX_CHECK_RESULT_SUMMARY_SCALARS_V2,
                "check result summary",
            )?;
            let input_basis =
                CheckResultInputBasisV2::new(input_basis.descriptor, input_basis.digest).map_err(
                    |_| SliceErrorV1::InvalidValue {
                        field: "check result input_basis",
                    },
                )?;
            let executor =
                CheckResultExecutorV2::new(executor.name, executor.version).map_err(|_| {
                    SliceErrorV1::InvalidValue {
                        field: "check result executor",
                    }
                })?;
            let outcome = CheckResultOutcomeV2::from_str(&outcome).map_err(|_| {
                SliceErrorV1::InvalidValue {
                    field: "check result outcome",
                }
            })?;
            let value = CheckResultValueV2::new(
                operation_id,
                operation_digest,
                input_basis,
                executor,
                outcome,
                summary,
                output_digest,
            )
            .map_err(|_| SliceErrorV1::InvalidValue {
                field: "check result summary",
            })?;
            let canonical =
                canonicalize_json_v1(&serde_json::to_value(&value).map_err(|error| {
                    SliceErrorV1::InvalidPayload {
                        message: error.to_string(),
                    }
                })?)
                .map_err(|error| SliceErrorV1::InvalidPayload {
                    message: error.to_string(),
                })?;
            if canonical.len() > podway_core::MAX_CHECK_RESULT_VALUE_BYTES_V2 {
                return Err(SliceErrorV1::BoundExceeded {
                    field: "check result value",
                    maximum: podway_core::MAX_CHECK_RESULT_VALUE_BYTES_V2,
                    actual: canonical.len(),
                    unit: "bytes",
                });
            }
            ItemRecordValueV1::CheckResult { value }
        }
    })
}

fn validate_safe_artifact_path(path: &str) -> Result<(), SliceErrorV1> {
    validate_scalar_bound(path, MAX_SLICE_ARTIFACT_PATH_SCALARS_V1, "path")?;
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains('\0')
        || path.ends_with('/')
        || path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':'
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(SliceErrorV1::InvalidValue { field: "path" });
    }
    Ok(())
}

fn validate_opaque_reference(value: &str) -> Result<(), SliceErrorV1> {
    if value.trim().is_empty() {
        return Err(SliceErrorV1::EmptyValue { field: "reference" });
    }
    validate_scalar_bound(value, MAX_SLICE_ARTIFACT_PATH_SCALARS_V1, "reference")?;
    if value.contains('\0') {
        return Err(SliceErrorV1::InvalidValue { field: "reference" });
    }
    Ok(())
}

fn validate_media_type(value: &str) -> Result<(), SliceErrorV1> {
    validate_non_empty_bytes(value, MAX_SLICE_MEDIA_TYPE_BYTES_V1, "media_type")?;
    let Some((kind, subtype)) = value.split_once('/') else {
        return Err(SliceErrorV1::InvalidValue {
            field: "media_type",
        });
    };
    if kind.is_empty()
        || subtype.is_empty()
        || subtype.contains('/')
        || !kind
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !subtype
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !kind.bytes().all(is_media_token)
        || !subtype.bytes().all(is_media_token)
    {
        return Err(SliceErrorV1::InvalidValue {
            field: "media_type",
        });
    }
    Ok(())
}

fn is_media_token(byte: u8) -> bool {
    byte.is_ascii_lowercase()
        || byte.is_ascii_digit()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
        )
}

fn validate_non_empty_bytes(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), SliceErrorV1> {
    if value.is_empty() {
        return Err(SliceErrorV1::EmptyValue { field });
    }
    if value.len() > maximum {
        return Err(SliceErrorV1::ValueTooLong {
            field,
            maximum,
            actual: value.len(),
        });
    }
    Ok(())
}

fn validate_scalar_bound(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), SliceErrorV1> {
    let actual = value.chars().count();
    if actual > maximum {
        return Err(SliceErrorV1::ValueTooLong {
            field,
            maximum,
            actual,
        });
    }
    Ok(())
}

fn validate_record_scalar_bound(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), SliceErrorV1> {
    let actual = value.chars().count();
    if actual > maximum {
        return Err(SliceErrorV1::BoundExceeded {
            field,
            maximum,
            actual,
            unit: "scalars",
        });
    }
    Ok(())
}

/// The same semantic identity is available to an in-process typed caller without IPC metadata.
pub fn canonical_procedure_v2_command_identity_v1(
    command: &ProcedureV2MutationCommandV1,
    resolved_workspace_id: &WorkspaceId,
) -> Result<String, SliceErrorV1> {
    let (preconditions, payload) = match command {
        ProcedureV2MutationCommandV1::SessionBegin(command) => (
            session_identity_preconditions_json(&command.preconditions),
            json!({ "initial_goal": &command.initial_goal }),
        ),
        ProcedureV2MutationCommandV1::SessionTerminalDisposition(command) => (
            session_identity_preconditions_json(&command.preconditions),
            json!({
                "disposition": &command.disposition,
                "actor": &command.actor,
            }),
        ),
        ProcedureV2MutationCommandV1::SessionDecide(command) => (
            {
                let mut preconditions = session_preconditions_json(&command.preconditions);
                if let Some(goal_revision) = command.expected_goal_revision {
                    insert_canonical_field_v1(
                        &mut preconditions,
                        "goal_revision",
                        json!(goal_revision),
                    )?;
                }
                preconditions
            },
            json!({
                "option_id": &command.option_id,
                "reason": &command.reason,
                "actor": &command.actor,
            }),
        ),
        ProcedureV2MutationCommandV1::SessionRework(command) => (
            json!({
                "session_id": &command.preconditions.expected_session_id,
                "session_revision": command.preconditions.expected_session_revision,
                "attempt_id": &command.preconditions.expected_attempt_id,
            }),
            json!({
                "target_graph_node_id": &command.target_graph_node_id,
                "reason": &command.reason,
                "actor": &command.actor,
            }),
        ),
        ProcedureV2MutationCommandV1::GoalDefine(command) => (
            session_identity_preconditions_json(&command.preconditions),
            json!({
                "goal": &command.goal,
                "criteria": &command.criteria,
                "actor": &command.actor,
            }),
        ),
        ProcedureV2MutationCommandV1::GoalRevise(command) => (
            json!({
                "session_id": &command.preconditions.expected_session_id,
                "session_revision": command.preconditions.expected_session_revision,
                "attempt_id": &command.preconditions.expected_attempt_id,
                "goal_revision": command.preconditions.expected_goal_revision,
            }),
            json!({
                "goal": &command.goal,
                "criteria": &command.criteria,
                "target_graph_node_id": &command.target_graph_node_id,
                "reason": &command.reason,
                "actor": &command.actor,
                "reactivate": command.reactivate,
            }),
        ),
        ProcedureV2MutationCommandV1::GoalAssessCriterion(command) => {
            let mut preconditions = session_preconditions_json(&command.preconditions);
            insert_canonical_field_v1(
                &mut preconditions,
                "goal_revision",
                json!(command.expected_goal_revision),
            )?;
            (
                preconditions,
                json!({
                    "criterion_id": &command.criterion_id,
                    "status": &command.status,
                    "reason": &command.reason,
                    "evidence": &command.evidence,
                    "items": &command.items,
                    "actor": &command.actor,
                }),
            )
        }
    };
    canonicalize_json_v1(&json!({
        "protocol_major": 1,
        "command": command.command_name(),
        "workspace_id": resolved_workspace_id,
        "preconditions": preconditions,
        "payload": payload,
    }))
    .map_err(|error| SliceErrorV1::Canonicalization {
        message: error.to_string(),
    })
}

pub fn canonical_direct_start_identity_v1(
    command: &SliceCommandV1,
    resolved_workspace_id: &WorkspaceId,
    resolved_procedure_digest: &Sha256Digest,
) -> Result<String, SliceErrorV1> {
    let expected_procedure_digest = match command {
        SliceCommandV1::SessionStart(start) => start.start.expected_procedure_digest.as_ref(),
        SliceCommandV1::SessionStartReplace(start) => {
            start.start.expected_procedure_digest.as_ref()
        }
        _ => {
            return Err(SliceErrorV1::InvalidValue {
                field: "resolved_procedure_digest",
            });
        }
    };
    let canonical = canonical_direct_slice_mutation_identity_v1(command, resolved_workspace_id)?;
    let mut identity: Value =
        serde_json::from_str(&canonical).map_err(|error| SliceErrorV1::Canonicalization {
            message: error.to_string(),
        })?;
    let preconditions = identity
        .get_mut("preconditions")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| SliceErrorV1::Canonicalization {
            message: "canonical mutation identity has no preconditions object".to_owned(),
        })?;
    preconditions.insert(
        "procedure_digest".to_owned(),
        json!(resolved_procedure_digest),
    );
    preconditions.insert(
        "expected_procedure_digest".to_owned(),
        json!(expected_procedure_digest),
    );
    canonicalize_json_v1(&identity).map_err(|error| SliceErrorV1::Canonicalization {
        message: error.to_string(),
    })
}
fn start_source_json(source: &SessionStartSourceV1) -> Value {
    match source {
        SessionStartSourceV1::Preset { preset } => json!({"preset": preset}),
        SessionStartSourceV1::Procedure { procedure } => json!({"procedure": procedure}),
    }
}

fn start_payload_json_v1(start: &SessionStartV1) -> Value {
    json!({
        "source": start_source_json(&start.source),
        "task_title": &start.task_title,
    })
}

fn deletion_mode_json(mode: &SessionDeletionModeV1) -> Value {
    match mode {
        SessionDeletionModeV1::Eligible => json!({ "mode": "eligible" }),
        SessionDeletionModeV1::Force { progress_summary } => json!({
            "mode": "force",
            "progress_summary": progress_summary,
        }),
        SessionDeletionModeV1::Delete {
            confirmed,
            progress_summary,
        } => json!({
            "mode": "delete",
            "confirmed": confirmed,
            "progress_summary": progress_summary,
        }),
        SessionDeletionModeV1::Supersede { reason, actor } => json!({
            "mode": "supersede",
            "reason": reason,
            "actor": actor,
        }),
        SessionDeletionModeV1::DisposeAndArchive { disposition, actor } => json!({
            "mode": "dispose_and_archive",
            "disposition": disposition,
            "actor": actor,
        }),
        SessionDeletionModeV1::LegacyConfirmed => json!({ "mode": "legacy_confirmed" }),
    }
}

fn insert_canonical_field_v1(
    value: &mut Value,
    field: &'static str,
    field_value: Value,
) -> Result<(), SliceErrorV1> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| SliceErrorV1::Canonicalization {
            message: "canonical identity component is not an object".to_owned(),
        })?;
    object.insert(field.to_owned(), field_value);
    Ok(())
}

fn item_attach_source_json(source: &ItemAttachSourceV1) -> Value {
    match source {
        ItemAttachSourceV1::Path { path, media_type } => {
            json!({"path": path, "media_type": media_type})
        }
        ItemAttachSourceV1::OpaqueReference {
            reference,
            digest,
            size_bytes,
            media_type,
        } => json!({
            "reference": reference,
            "digest": digest,
            "size_bytes": size_bytes,
            "media_type": media_type,
        }),
    }
}

fn item_preconditions_json(preconditions: &ItemMutationPreconditionsWireV1) -> Value {
    json!({
        "session_id": &preconditions.expected_session_id,
        "attempt_id": &preconditions.expected_attempt_id,
        "item_revision": preconditions.expected_item_revision,
    })
}

fn session_preconditions_json(preconditions: &SessionMutationPreconditionsWireV1) -> Value {
    json!({
        "session_id": &preconditions.expected_session_id,
        "session_revision": preconditions.expected_session_revision,
        "attempt_id": &preconditions.expected_attempt_id,
    })
}

fn session_identity_preconditions_json(
    preconditions: &SessionIdentityPreconditionsWireV1,
) -> Value {
    json!({
        "session_id": &preconditions.expected_session_id,
        "session_revision": preconditions.expected_session_revision,
    })
}

fn workspace_reset_all_preconditions_json(
    preconditions: &WorkspaceResetAllPreconditionsWireV1,
) -> Value {
    json!({"workspace_id": &preconditions.expected_workspace_id})
}

fn deserialize_optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let value = Option::<T>::deserialize(deserializer)?;
    if value.is_none() {
        return Err(serde::de::Error::custom("explicit null is not permitted"));
    }
    Ok(value)
}

/// Direct writes include archive purging and local repair, which previously bypassed jobs.
/// Retired jobs and queries never acquire a mutation identity through this boundary.
pub fn canonical_direct_slice_mutation_identity_v1(
    command: &SliceCommandV1,
    resolved_workspace_id: &WorkspaceId,
) -> Result<String, SliceErrorV1> {
    if !is_direct_slice_mutation_v1(command) {
        return Err(SliceErrorV1::NotAMutation {
            command: command.command_name(),
        });
    }

    let (preconditions, payload) = match command {
        SliceCommandV1::WorkspaceInit(init) => (json!({}), json!({"repair": init.repair})),
        SliceCommandV1::WorkspaceRepair(_) => (json!({}), json!({})),
        SliceCommandV1::SessionArchivePurge(purge) => (
            json!({"session_id":purge.session_id,"session_revision":purge.expected_session_revision}),
            json!({"confirmed":purge.confirmed}),
        ),
        SliceCommandV1::SessionStart(start) => (
            start
                .preconditions
                .as_ref()
                .map_or_else(|| json!({}), session_identity_preconditions_json),
            start_payload_json_v1(&start.start),
        ),
        SliceCommandV1::SessionStartReplace(start) => {
            (session_identity_preconditions_json(&start.preconditions), {
                let mut payload = start_payload_json_v1(&start.start);
                insert_canonical_field_v1(
                    &mut payload,
                    "deletion",
                    deletion_mode_json(&start.mode),
                )?;
                payload
            })
        }
        SliceCommandV1::SessionComplete(session) => (
            session_preconditions_json(&session.preconditions),
            json!({}),
        ),
        SliceCommandV1::SessionSkip(session) => (
            session_preconditions_json(&session.preconditions),
            json!({"reason": &session.reason}),
        ),
        SliceCommandV1::SessionRetry(session) => (
            session_preconditions_json(&session.preconditions),
            json!({"reason": &session.reason}),
        ),
        SliceCommandV1::SessionBlock(session) => (
            session_preconditions_json(&session.preconditions),
            json!({"reason": &session.reason}),
        ),
        SliceCommandV1::SessionUnblock(session) => (
            session_preconditions_json(&session.preconditions),
            json!({"blocker_id": &session.blocker_id, "all": session.all}),
        ),
        SliceCommandV1::SessionCancel(session) => (
            session_preconditions_json(&session.preconditions),
            json!({"reason": &session.reason}),
        ),
        SliceCommandV1::SessionReset(session) => (
            session_identity_preconditions_json(&session.preconditions),
            json!({"deletion": deletion_mode_json(&session.mode)}),
        ),
        SliceCommandV1::SessionArchive(session) => (
            session_identity_preconditions_json(&session.preconditions),
            json!({}),
        ),
        SliceCommandV1::WorkspaceResetAll(workspace) => (
            workspace_reset_all_preconditions_json(&workspace.preconditions),
            json!({"confirmed": workspace.confirmed}),
        ),
        SliceCommandV1::ItemCheck(item) => (
            item_preconditions_json(&item.preconditions),
            json!({"item_id": &item.item_id}),
        ),
        SliceCommandV1::ItemUncheck(item) => (
            item_preconditions_json(&item.preconditions),
            json!({"item_id": &item.item_id}),
        ),
        SliceCommandV1::ItemSet(item) => (
            item_preconditions_json(&item.preconditions),
            json!({"item_id": &item.item_id, "value": &item.value}),
        ),
        SliceCommandV1::ItemAdd(item) => (
            item_preconditions_json(&item.preconditions),
            json!({"item_id": &item.item_id, "value": &item.value}),
        ),
        SliceCommandV1::ItemRemove(item) => (
            item_preconditions_json(&item.preconditions),
            json!({
                "item_id": &item.item_id,
                "value": &item.value,
                "ignore_missing": item.ignore_missing,
            }),
        ),
        SliceCommandV1::ItemAttach(item) => (
            item_preconditions_json(&item.preconditions),
            json!({"item_id": &item.item_id, "source": item_attach_source_json(&item.source)}),
        ),
        SliceCommandV1::ItemClear(item) => (
            item_preconditions_json(&item.preconditions),
            json!({"item_id": &item.item_id}),
        ),
        SliceCommandV1::ItemRecordMany(item) => (
            session_preconditions_json(&item.preconditions),
            json!({"operations": &item.operations}),
        ),
        SliceCommandV1::DelegationAccept(input) => (
            session_preconditions_json(&input.preconditions),
            input.input.clone(),
        ),
    };

    canonicalize_json_v1(&json!({
        "protocol_major": 1,
        "command": command.command_name(),
        "workspace_id": resolved_workspace_id,
        "preconditions": preconditions,
        "payload": payload,
    }))
    .map_err(|error| SliceErrorV1::Canonicalization {
        message: error.to_string(),
    })
}

pub const fn is_direct_slice_mutation_v1(command: &SliceCommandV1) -> bool {
    match command {
        SliceCommandV1::SessionStart(input) => !input.start.dry_run,
        SliceCommandV1::SessionStartReplace(input) => !input.start.dry_run,
        SliceCommandV1::SessionReset(input) => !input.dry_run,
        SliceCommandV1::WorkspaceInit(_)
        | SliceCommandV1::WorkspaceRepair(_)
        | SliceCommandV1::WorkspaceResetAll(_)
        | SliceCommandV1::SessionArchive(_)
        | SliceCommandV1::SessionArchivePurge(_)
        | SliceCommandV1::SessionComplete(_)
        | SliceCommandV1::SessionSkip(_)
        | SliceCommandV1::SessionRetry(_)
        | SliceCommandV1::SessionBlock(_)
        | SliceCommandV1::SessionUnblock(_)
        | SliceCommandV1::SessionCancel(_)
        | SliceCommandV1::DelegationAccept(_)
        | SliceCommandV1::ItemCheck(_)
        | SliceCommandV1::ItemUncheck(_)
        | SliceCommandV1::ItemSet(_)
        | SliceCommandV1::ItemAdd(_)
        | SliceCommandV1::ItemRemove(_)
        | SliceCommandV1::ItemAttach(_)
        | SliceCommandV1::ItemClear(_)
        | SliceCommandV1::ItemRecordMany(_) => true,
    }
}

fn validated_attach_source(
    path: Option<String>,
    reference: Option<String>,
    digest: Option<Sha256Digest>,
    size_bytes: Option<u64>,
    media_type: Option<String>,
) -> Result<ItemAttachSourceV1, SliceErrorV1> {
    match (path, reference, digest, size_bytes, media_type) {
        (Some(path), None, None, None, media_type) => {
            validate_safe_artifact_path(&path)?;
            if let Some(media_type) = &media_type {
                validate_media_type(media_type)?;
            }
            Ok(ItemAttachSourceV1::Path { path, media_type })
        }
        (None, Some(reference), Some(digest), Some(size_bytes), Some(media_type)) => {
            validate_opaque_reference(&reference)?;
            validate_media_type(&media_type)?;
            Ok(ItemAttachSourceV1::OpaqueReference {
                reference,
                digest,
                size_bytes,
                media_type,
            })
        }
        _ => Err(SliceErrorV1::InvalidValue {
            field: "item.attach source",
        }),
    }
}
