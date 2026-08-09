use std::fmt;

use podway_core::{
    ActorAttributionV2, AttemptId, BlockerId, CriterionAssessmentReasonV2, CriterionId,
    CriterionStatusV2, GoalCriterionV2, GoalDefinitionV2, GoalRevisionNumberV2,
    GoalRevisionReasonV2, GoalStatementV2, GraphNodeId, ItemId, JobId,
    MAX_OPEN_BLOCKERS_PER_ATTEMPT_V1, MAX_STAGE_ITEMS, OptionId, ReasonV2, Revision, SessionId,
    Sha256Digest, StageId, WorkspaceId, canonicalize_json_v1,
};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};

use crate::{
    ErrorCodeV1, ExitCodeV1, IdempotencyKeyV1, OperationV1, PreconditionsV1, RequestEnvelopeV1,
    Rfc3339MillisV1, SessionLifecycleV1, SessionOutputV1,
};

/// The only selector representation accepted by the G006 daemon boundary.
pub const WORKTREE_SELECTOR_WIRE_V1_VERSION: u8 = 1;
/// Maximum decoded path bytes and diagnostic display bytes for one selector.
pub const MAX_WORKTREE_SELECTOR_COMPONENT_BYTES_V1: usize = 16 * 1024;
/// Core-compatible maximums for command text admitted at the wire boundary.
pub const MAX_SLICE_TASK_TITLE_SCALARS_V1: usize = 500;
pub const MAX_SLICE_REASON_SCALARS_V1: usize = 4_000;
pub const MAX_SLICE_ITEM_TEXT_SCALARS_V1: usize = 65_536;
pub const MAX_SLICE_LIST_VALUE_SCALARS_V1: usize = 4_000;
pub const MAX_SLICE_ARTIFACT_PATH_SCALARS_V1: usize = 4_000;
pub const MAX_SLICE_MEDIA_TYPE_BYTES_V1: usize = 255;

/// Validation failures for the G006 protocol slice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SliceErrorV1 {
    InvalidSelector {
        reason: &'static str,
    },
    InvalidBase64Url,
    ValueTooLong {
        field: &'static str,
        maximum: usize,
        actual: usize,
    },
    EmptyValue {
        field: &'static str,
    },
    InvalidValue {
        field: &'static str,
    },
    InvalidCommand {
        received: String,
    },
    OperationMismatch {
        command: &'static str,
        expected: OperationV1,
        received: OperationV1,
    },
    MissingWorkspace {
        command: &'static str,
    },
    MissingIdempotencyKey {
        command: &'static str,
    },
    UnexpectedIdempotencyKey {
        command: &'static str,
    },
    MissingPrecondition {
        field: &'static str,
    },
    UnexpectedPrecondition {
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
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSelector { reason } => {
                write!(formatter, "invalid worktree selector: {reason}")
            }
            Self::InvalidBase64Url => {
                formatter.write_str("path_bytes_base64url must be canonical unpadded base64url")
            }
            Self::ValueTooLong {
                field,
                maximum,
                actual,
            } => write!(
                formatter,
                "{field} exceeds its maximum of {maximum} (received {actual})"
            ),
            Self::EmptyValue { field } => write!(formatter, "{field} must not be empty"),
            Self::InvalidValue { field } => write!(formatter, "{field} is invalid"),
            Self::InvalidCommand { received } => {
                write!(formatter, "unsupported G006 command {received:?}")
            }
            Self::OperationMismatch {
                command,
                expected,
                received,
            } => write!(
                formatter,
                "{command} requires {expected:?}, not {received:?}"
            ),
            Self::MissingWorkspace { command } => {
                write!(formatter, "{command} requires workspace context")
            }
            Self::MissingIdempotencyKey { command } => {
                write!(formatter, "{command} requires an idempotency key")
            }
            Self::UnexpectedIdempotencyKey { command } => {
                write!(formatter, "{command} must not include an idempotency key")
            }
            Self::MissingPrecondition { field } => {
                write!(formatter, "missing required precondition {field}")
            }
            Self::UnexpectedPrecondition { field } => {
                write!(formatter, "unexpected precondition {field}")
            }
            Self::InvalidPayload { message } => {
                write!(formatter, "invalid G006 command payload: {message}")
            }
            Self::NotAMutation { command } => write!(formatter, "{command} is not a mutation"),
            Self::Canonicalization { message } => write!(
                formatter,
                "cannot canonicalize mutation identity: {message}"
            ),
        }
    }
}

impl std::error::Error for SliceErrorV1 {}

/// Encodes bytes as canonical unpadded RFC 4648 base64url.
pub fn encode_base64url_unpadded_v1(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    let mut encoded = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        encoded.push(ALPHABET[(first >> 2) as usize] as char);
        match chunk.len() {
            1 => encoded.push(ALPHABET[((first & 0x03) << 4) as usize] as char),
            2 => {
                let second = chunk[1];
                encoded.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
                encoded.push(ALPHABET[((second & 0x0f) << 2) as usize] as char);
            }
            3 => {
                let second = chunk[1];
                let third = chunk[2];
                encoded.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
                encoded.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
                encoded.push(ALPHABET[(third & 0x3f) as usize] as char);
            }
            _ => unreachable!("chunks(3) never yields more than three bytes"),
        }
    }
    encoded
}

/// Decodes only canonical unpadded RFC 4648 base64url.
pub fn decode_base64url_unpadded_v1(value: &str) -> Result<Vec<u8>, SliceErrorV1> {
    let maximum_encoded = (MAX_WORKTREE_SELECTOR_COMPONENT_BYTES_V1 * 4).div_ceil(3);
    if value.len() > maximum_encoded {
        return Err(SliceErrorV1::ValueTooLong {
            field: "selector.path_bytes_base64url",
            maximum: maximum_encoded,
            actual: value.len(),
        });
    }
    if value.len() % 4 == 1 {
        return Err(SliceErrorV1::InvalidBase64Url);
    }

    let mut decoded = Vec::with_capacity(value.len() / 4 * 3 + 2);
    let input = value.as_bytes();
    for chunk in input.chunks(4) {
        let mut sextets = [0_u8; 4];
        for (index, byte) in chunk.iter().copied().enumerate() {
            sextets[index] = base64url_sextet(byte).ok_or(SliceErrorV1::InvalidBase64Url)?;
        }
        match chunk.len() {
            2 => {
                if sextets[1] & 0x0f != 0 {
                    return Err(SliceErrorV1::InvalidBase64Url);
                }
                decoded.push((sextets[0] << 2) | (sextets[1] >> 4));
            }
            3 => {
                if sextets[2] & 0x03 != 0 {
                    return Err(SliceErrorV1::InvalidBase64Url);
                }
                decoded.push((sextets[0] << 2) | (sextets[1] >> 4));
                decoded.push((sextets[1] << 4) | (sextets[2] >> 2));
            }
            4 => {
                decoded.push((sextets[0] << 2) | (sextets[1] >> 4));
                decoded.push((sextets[1] << 4) | (sextets[2] >> 2));
                decoded.push((sextets[2] << 6) | sextets[3]);
            }
            _ => return Err(SliceErrorV1::InvalidBase64Url),
        }
    }
    if decoded.len() > MAX_WORKTREE_SELECTOR_COMPONENT_BYTES_V1 {
        return Err(SliceErrorV1::ValueTooLong {
            field: "selector.path_bytes",
            maximum: MAX_WORKTREE_SELECTOR_COMPONENT_BYTES_V1,
            actual: decoded.len(),
        });
    }
    Ok(decoded)
}

fn base64url_sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

/// A lossless, diagnostic-only worktree selector. The daemon independently resolves this hint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeSelectorWireV1 {
    version: u8,
    path_bytes_base64url: String,
    display: String,
    expected_uuid: Option<WorkspaceId>,
}

impl WorktreeSelectorWireV1 {
    pub fn new(
        path_bytes: &[u8],
        display: impl Into<String>,
        expected_uuid: Option<WorkspaceId>,
    ) -> Result<Self, SliceErrorV1> {
        let selector = Self {
            version: WORKTREE_SELECTOR_WIRE_V1_VERSION,
            path_bytes_base64url: encode_base64url_unpadded_v1(path_bytes),
            display: display.into(),
            expected_uuid,
        };
        selector.validate()?;
        Ok(selector)
    }

    pub const fn version(&self) -> u8 {
        self.version
    }

    pub fn path_bytes_base64url(&self) -> &str {
        &self.path_bytes_base64url
    }

    pub fn path_bytes(&self) -> Result<Vec<u8>, SliceErrorV1> {
        decode_base64url_unpadded_v1(&self.path_bytes_base64url)
    }

    /// Diagnostic text only. It is never used for workspace identity or mutation identity.
    pub fn display(&self) -> &str {
        &self.display
    }

    pub fn expected_uuid(&self) -> Option<&WorkspaceId> {
        self.expected_uuid.as_ref()
    }

    fn validate(&self) -> Result<(), SliceErrorV1> {
        if self.version != WORKTREE_SELECTOR_WIRE_V1_VERSION {
            return Err(SliceErrorV1::InvalidSelector {
                reason: "unsupported version",
            });
        }
        if self.display.is_empty() {
            return Err(SliceErrorV1::EmptyValue {
                field: "selector.display",
            });
        }
        if self.display.len() > MAX_WORKTREE_SELECTOR_COMPONENT_BYTES_V1 {
            return Err(SliceErrorV1::ValueTooLong {
                field: "selector.display",
                maximum: MAX_WORKTREE_SELECTOR_COMPONENT_BYTES_V1,
                actual: self.display.len(),
            });
        }
        if self.display.contains('\0') {
            return Err(SliceErrorV1::InvalidSelector {
                reason: "display contains NUL",
            });
        }

        let path = decode_base64url_unpadded_v1(&self.path_bytes_base64url)?;
        if path.is_empty() {
            return Err(SliceErrorV1::InvalidSelector {
                reason: "path bytes are empty",
            });
        }
        if path.contains(&0) {
            return Err(SliceErrorV1::InvalidSelector {
                reason: "path bytes contain NUL",
            });
        }
        if path[0] != b'/' {
            return Err(SliceErrorV1::InvalidSelector {
                reason: "path must be absolute and local",
            });
        }
        if path.starts_with(b"//") || path.starts_with(b"\\\\") {
            return Err(SliceErrorV1::InvalidSelector {
                reason: "network path forms are not accepted",
            });
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for WorktreeSelectorWireV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawWorktreeSelectorWireV1 {
            version: u8,
            path_bytes_base64url: String,
            display: String,
            #[serde(deserialize_with = "deserialize_required_option")]
            expected_uuid: Option<WorkspaceId>,
        }

        let raw = RawWorktreeSelectorWireV1::deserialize(deserializer)?;
        let selector = Self {
            version: raw.version,
            path_bytes_base64url: raw.path_bytes_base64url,
            display: raw.display,
            expected_uuid: raw.expected_uuid,
        };
        selector.validate().map_err(serde::de::Error::custom)?;
        Ok(selector)
    }
}

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

/// Optimistic-concurrency facts required when a completed session is reopened.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRevisionPreconditionsWireV1 {
    pub expected_session_id: SessionId,
    pub expected_session_revision: Revision,
}

/// Optional identity guard accepted by session-bearing reads.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionReadPreconditionsWireV1 {
    pub expected_session_id: Option<SessionId>,
}

/// Optimistic-concurrency facts required to cancel a queued job.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobMutationPreconditionsWireV1 {
    pub expected_job_state: crate::JobStateV1,
}

/// Daemon-observed workspace facts required by `workspace.reset_all`.
///
/// Selector UUID hints and this semantic assertion must agree when either is supplied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceResetAllPreconditionsWireV1 {
    pub expected_workspace_id: Option<WorkspaceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDoctorV1 {
    pub deep: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceShowV1 {}

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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionStartReplaceV1 {
    pub start: SessionStartV1,
    pub confirmed: bool,
    pub preconditions: SessionIdentityPreconditionsWireV1,
}

/// A query wait is deliberately exclusive: callers may wait for queue idleness or one job, not both.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryWaitV1 {
    Immediate,
    Idle,
    AfterJob { job_id: JobId },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionStatusV1 {
    pub wait: QueryWaitV1,
    pub verbose: bool,
    pub compact: bool,
    pub preconditions: SessionReadPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionNextV1 {
    pub wait: QueryWaitV1,
    pub preconditions: SessionReadPreconditionsWireV1,
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
pub struct SessionReturnV1 {
    pub destination_stage_id: StageId,
    pub reason: String,
    pub dry_run: bool,
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
pub struct SessionReopenV1 {
    pub destination_stage_id: StageId,
    pub reason: String,
    pub dry_run: bool,
    pub preconditions: SessionRevisionPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionResetV1 {
    pub confirmed: bool,
    pub dry_run: bool,
    pub preconditions: SessionIdentityPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceResetAllV1 {
    pub confirmed: bool,
    pub preconditions: WorkspaceResetAllPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobListV1 {
    pub state: Option<crate::JobStateV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobStatusV1 {
    pub job_id: JobId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobLookupV1 {
    pub idempotency_key: IdempotencyKeyV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobWaitV1 {
    pub job_id: JobId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobCancelV1 {
    pub job_id: JobId,
    pub preconditions: JobMutationPreconditionsWireV1,
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

/// A closed Procedure v2 start payload decoded without making the route executable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcedureV2SessionStartV1 {
    pub start: SessionStartV1,
    pub initial_goal: Option<InitialGoalWireV2>,
}

/// A closed Procedure v2 replacement-start payload decoded without dispatching it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcedureV2SessionStartReplaceV1 {
    pub start: ProcedureV2SessionStartV1,
    pub confirmed: bool,
    pub preconditions: SessionIdentityPreconditionsWireV1,
}

/// Typed protocol-only Procedure v2 start routes reserved for daemon integration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureV2StartCommandV1 {
    SessionStart(ProcedureV2SessionStartV1),
    SessionStartReplace(ProcedureV2SessionStartReplaceV1),
}

impl ProcedureV2StartCommandV1 {
    pub const fn command_name(&self) -> &'static str {
        match self {
            Self::SessionStart(_) => "session.start",
            Self::SessionStartReplace(_) => "session.start_replace",
        }
    }

    pub const fn is_mutation(&self) -> bool {
        match self {
            Self::SessionStart(start) => !start.start.dry_run,
            Self::SessionStartReplace(start) => !start.start.start.dry_run,
        }
    }
}

/// One decoded Procedure v2 start plus its lossless worktree selector.
///
/// This protocol boundary is intentionally separate from [`SliceRequestV1`]. The daemon keeps
/// rejecting goal-bearing starts until V2PLT-007 wires the complete Procedure v2 admission path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcedureV2StartRequestV1 {
    selector: WorktreeSelectorWireV1,
    command: ProcedureV2StartCommandV1,
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

/// The five daemon mutation routes reserved by the Procedure v2 contract.
pub const RESERVED_V2_MUTATION_COMMAND_NAMES_V1: [&str; 5] = [
    "session.decide",
    "session.rework",
    "goal.define",
    "goal.revise",
    "goal.assess_criterion",
];

/// Typed request data for the reserved Procedure v2 daemon mutation boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureV2MutationCommandV1 {
    SessionDecide(SessionDecideV2),
    SessionRework(SessionReworkV2),
    GoalDefine(GoalDefineV2),
    GoalRevise(GoalReviseV2),
    GoalAssessCriterion(GoalAssessCriterionV2),
}

impl ProcedureV2MutationCommandV1 {
    pub const fn command_name(&self) -> &'static str {
        match self {
            Self::SessionDecide(_) => "session.decide",
            Self::SessionRework(_) => "session.rework",
            Self::GoalDefine(_) => "goal.define",
            Self::GoalRevise(_) => "goal.revise",
            Self::GoalAssessCriterion(_) => "goal.assess_criterion",
        }
    }
}

/// One decoded reserved Procedure v2 mutation plus its lossless worktree selector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcedureV2MutationRequestV1 {
    selector: WorktreeSelectorWireV1,
    command: ProcedureV2MutationCommandV1,
}

/// The authoritative G006 daemon route set. No aliases are admitted at the protocol boundary.
pub const DAEMON_COMMAND_NAMES_V1: [&str; 30] = [
    "workspace.init",
    "workspace.doctor",
    "workspace.show",
    "workspace.repair",
    "session.start",
    "session.start_replace",
    "session.status",
    "session.next",
    "session.complete",
    "session.skip",
    "session.retry",
    "session.return",
    "session.block",
    "session.unblock",
    "session.cancel",
    "session.reopen",
    "session.reset",
    "workspace.reset_all",
    "item.check",
    "item.uncheck",
    "item.set",
    "item.add",
    "item.remove",
    "item.attach",
    "item.clear",
    "job.list",
    "job.lookup",
    "job.status",
    "job.wait",
    "job.cancel",
];

/// The complete G006 daemon command set. Each variant maps to exactly one canonical route.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceCommandV1 {
    WorkspaceInit(WorkspaceInitV1),
    WorkspaceDoctor(WorkspaceDoctorV1),
    WorkspaceShow(WorkspaceShowV1),
    WorkspaceRepair(WorkspaceRepairV1),
    SessionStart(SessionStartV1),
    SessionStartReplace(SessionStartReplaceV1),
    SessionStatus(SessionStatusV1),
    SessionNext(SessionNextV1),
    SessionComplete(SessionCompleteV1),
    SessionSkip(SessionSkipV1),
    SessionRetry(SessionRetryV1),
    SessionReturn(SessionReturnV1),
    SessionBlock(SessionBlockV1),
    SessionUnblock(SessionUnblockV1),
    SessionCancel(SessionCancelV1),
    SessionReopen(SessionReopenV1),
    SessionReset(SessionResetV1),
    WorkspaceResetAll(WorkspaceResetAllV1),
    ItemCheck(ItemCheckV1),
    ItemUncheck(ItemUncheckV1),
    ItemSet(ItemSetV1),
    ItemAdd(ItemAddV1),
    ItemRemove(ItemRemoveV1),
    ItemAttach(ItemAttachV1),
    ItemClear(ItemClearV1),
    JobList(JobListV1),
    JobLookup(JobLookupV1),
    JobStatus(JobStatusV1),
    JobWait(JobWaitV1),
    JobCancel(JobCancelV1),
}

impl SliceCommandV1 {
    pub const fn command_name(&self) -> &'static str {
        match self {
            Self::WorkspaceInit(_) => "workspace.init",
            Self::WorkspaceDoctor(_) => "workspace.doctor",
            Self::WorkspaceShow(_) => "workspace.show",
            Self::WorkspaceRepair(_) => "workspace.repair",
            Self::SessionStart(_) => "session.start",
            Self::SessionStartReplace(_) => "session.start_replace",
            Self::SessionStatus(_) => "session.status",
            Self::SessionNext(_) => "session.next",
            Self::SessionComplete(_) => "session.complete",
            Self::SessionSkip(_) => "session.skip",
            Self::SessionRetry(_) => "session.retry",
            Self::SessionReturn(_) => "session.return",
            Self::SessionBlock(_) => "session.block",
            Self::SessionUnblock(_) => "session.unblock",
            Self::SessionCancel(_) => "session.cancel",
            Self::SessionReopen(_) => "session.reopen",
            Self::SessionReset(_) => "session.reset",
            Self::WorkspaceResetAll(_) => "workspace.reset_all",
            Self::ItemCheck(_) => "item.check",
            Self::ItemUncheck(_) => "item.uncheck",
            Self::ItemSet(_) => "item.set",
            Self::ItemAdd(_) => "item.add",
            Self::ItemRemove(_) => "item.remove",
            Self::ItemAttach(_) => "item.attach",
            Self::ItemClear(_) => "item.clear",
            Self::JobList(_) => "job.list",
            Self::JobLookup(_) => "job.lookup",
            Self::JobStatus(_) => "job.status",
            Self::JobWait(_) => "job.wait",
            Self::JobCancel(_) => "job.cancel",
        }
    }

    pub const fn operation(&self) -> OperationV1 {
        match self {
            Self::SessionStart(command) => {
                if command.dry_run {
                    OperationV1::Query
                } else {
                    OperationV1::Mutate
                }
            }
            Self::SessionStartReplace(command) => {
                if command.start.dry_run {
                    OperationV1::Query
                } else {
                    OperationV1::Mutate
                }
            }
            Self::SessionReturn(command) => {
                if command.dry_run {
                    OperationV1::Query
                } else {
                    OperationV1::Mutate
                }
            }
            Self::SessionReopen(command) => {
                if command.dry_run {
                    OperationV1::Query
                } else {
                    OperationV1::Mutate
                }
            }
            Self::SessionReset(command) => {
                if command.dry_run {
                    OperationV1::Query
                } else {
                    OperationV1::Mutate
                }
            }
            Self::WorkspaceInit(_) | Self::WorkspaceResetAll(_) => OperationV1::Bootstrap,
            Self::WorkspaceRepair(_) | Self::JobCancel(_) => OperationV1::Control,
            Self::WorkspaceDoctor(_)
            | Self::WorkspaceShow(_)
            | Self::SessionStatus(_)
            | Self::SessionNext(_)
            | Self::JobList(_)
            | Self::JobLookup(_)
            | Self::JobStatus(_)
            | Self::JobWait(_) => OperationV1::Query,
            Self::SessionComplete(_)
            | Self::SessionSkip(_)
            | Self::SessionRetry(_)
            | Self::SessionBlock(_)
            | Self::SessionUnblock(_)
            | Self::SessionCancel(_)
            | Self::ItemCheck(_)
            | Self::ItemUncheck(_)
            | Self::ItemSet(_)
            | Self::ItemAdd(_)
            | Self::ItemRemove(_)
            | Self::ItemAttach(_)
            | Self::ItemClear(_) => OperationV1::Mutate,
        }
    }

    /// Whether this route admits a durable daemon job.
    pub const fn is_durable_job(&self) -> bool {
        match self {
            Self::SessionStart(command) => !command.dry_run,
            Self::SessionStartReplace(command) => !command.start.dry_run,
            Self::SessionReturn(command) => !command.dry_run,
            Self::SessionReopen(command) => !command.dry_run,
            Self::SessionReset(command) => !command.dry_run,
            _ => matches!(
                self,
                Self::WorkspaceInit(_)
                    | Self::WorkspaceResetAll(_)
                    | Self::SessionComplete(_)
                    | Self::SessionSkip(_)
                    | Self::SessionRetry(_)
                    | Self::SessionBlock(_)
                    | Self::SessionUnblock(_)
                    | Self::SessionCancel(_)
                    | Self::ItemCheck(_)
                    | Self::ItemUncheck(_)
                    | Self::ItemSet(_)
                    | Self::ItemAdd(_)
                    | Self::ItemRemove(_)
                    | Self::ItemAttach(_)
                    | Self::ItemClear(_)
            ),
        }
    }

    pub const fn is_mutation(&self) -> bool {
        self.is_durable_job()
    }
}

/// Parsed G006 data from a generic v1 request envelope. Envelope metadata remains transport-owned.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SliceRequestV1 {
    selector: WorktreeSelectorWireV1,
    command: SliceCommandV1,
}

impl SliceRequestV1 {
    pub fn from_envelope(envelope: &RequestEnvelopeV1) -> Result<Self, SliceErrorV1> {
        let command_name = envelope.command().as_str();
        let (selector, command) = match command_name {
            "workspace.init" => {
                require_envelope(envelope, "workspace.init", OperationV1::Bootstrap, true)?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: WorkspaceInitPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::WorkspaceInit(WorkspaceInitV1 {
                        repair: payload.repair,
                    }),
                )
            }
            "workspace.doctor" => {
                require_envelope(envelope, "workspace.doctor", OperationV1::Query, false)?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: WorkspaceDoctorPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::WorkspaceDoctor(WorkspaceDoctorV1 { deep: payload.deep }),
                )
            }
            "workspace.show" => {
                require_envelope(envelope, "workspace.show", OperationV1::Query, false)?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: SelectorOnlyPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::WorkspaceShow(WorkspaceShowV1 {}),
                )
            }
            "workspace.repair" => {
                require_envelope(envelope, "workspace.repair", OperationV1::Control, false)?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: SelectorOnlyPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::WorkspaceRepair(WorkspaceRepairV1 {}),
                )
            }
            "session.start" => {
                let payload: SessionStartPayloadV1 = parse_payload(envelope)?;
                require_dry_run_envelope(envelope, "session.start", payload.dry_run)?;
                require_no_preconditions(envelope.preconditions())?;
                let start = validated_start(
                    payload.preset,
                    payload.procedure,
                    payload.expected_procedure_digest,
                    payload.task_title,
                    payload.dry_run,
                )?;
                (payload.selector, SliceCommandV1::SessionStart(start))
            }
            "session.start_replace" => {
                let payload: SessionStartReplacePayloadV1 = parse_payload(envelope)?;
                require_dry_run_envelope(envelope, "session.start_replace", payload.dry_run)?;
                let preconditions =
                    require_session_identity_preconditions(envelope.preconditions())?;
                let confirmed = match (payload.dry_run, payload.confirmed) {
                    (true, None) => false,
                    (true, Some(_)) => {
                        return Err(SliceErrorV1::InvalidValue { field: "confirmed" });
                    }
                    (false, confirmed) => {
                        let confirmed = confirmed.unwrap_or(false);
                        require_confirmation(confirmed)?;
                        confirmed
                    }
                };
                let start = validated_start(
                    payload.preset,
                    payload.procedure,
                    payload.expected_procedure_digest,
                    payload.task_title,
                    payload.dry_run,
                )?;
                (
                    payload.selector,
                    SliceCommandV1::SessionStartReplace(SessionStartReplaceV1 {
                        start,
                        confirmed,
                        preconditions,
                    }),
                )
            }
            "session.status" => {
                require_envelope(envelope, "session.status", OperationV1::Query, false)?;
                let preconditions = require_session_read_preconditions(envelope.preconditions())?;
                let payload: SessionStatusPayloadV1 = parse_payload(envelope)?;
                let wait = validated_query_wait(payload.wait_for_idle, payload.after_job_id)?;
                if payload.compact && (!matches!(wait, QueryWaitV1::Idle) || payload.verbose) {
                    return Err(SliceErrorV1::InvalidValue { field: "compact" });
                }
                (
                    payload.selector,
                    SliceCommandV1::SessionStatus(SessionStatusV1 {
                        wait,
                        verbose: payload.verbose,
                        compact: payload.compact,
                        preconditions,
                    }),
                )
            }
            "session.next" => {
                require_envelope(envelope, "session.next", OperationV1::Query, false)?;
                let preconditions = require_session_read_preconditions(envelope.preconditions())?;
                let payload: SessionNextPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::SessionNext(SessionNextV1 {
                        wait: validated_query_wait(payload.wait_for_idle, payload.after_job_id)?,
                        preconditions,
                    }),
                )
            }
            "session.complete" => {
                require_envelope(envelope, "session.complete", OperationV1::Mutate, true)?;
                let preconditions = require_session_preconditions(envelope.preconditions())?;
                let payload: SelectorOnlyPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::SessionComplete(SessionCompleteV1 { preconditions }),
                )
            }
            "session.skip" => {
                require_envelope(envelope, "session.skip", OperationV1::Mutate, true)?;
                let preconditions = require_session_preconditions(envelope.preconditions())?;
                let payload: SessionSkipPayloadV1 = parse_payload(envelope)?;
                if let Some(reason) = &payload.reason {
                    validate_reason(reason)?;
                }
                (
                    payload.selector,
                    SliceCommandV1::SessionSkip(SessionSkipV1 {
                        reason: payload.reason,
                        preconditions,
                    }),
                )
            }
            "session.retry" => {
                require_envelope(envelope, "session.retry", OperationV1::Mutate, true)?;
                let preconditions = require_session_preconditions(envelope.preconditions())?;
                let payload: SessionReasonPayloadV1 = parse_payload(envelope)?;
                validate_reason(&payload.reason)?;
                (
                    payload.selector,
                    SliceCommandV1::SessionRetry(SessionRetryV1 {
                        reason: payload.reason,
                        preconditions,
                    }),
                )
            }
            "session.return" => {
                let payload: SessionStageReasonPayloadV1 = parse_payload(envelope)?;
                require_dry_run_envelope(envelope, "session.return", payload.dry_run)?;
                let preconditions = require_session_preconditions(envelope.preconditions())?;
                validate_reason(&payload.reason)?;
                (
                    payload.selector,
                    SliceCommandV1::SessionReturn(SessionReturnV1 {
                        destination_stage_id: payload.destination_stage_id,
                        reason: payload.reason,
                        dry_run: payload.dry_run,
                        preconditions,
                    }),
                )
            }
            "session.block" => {
                require_envelope(envelope, "session.block", OperationV1::Mutate, true)?;
                let preconditions = require_session_preconditions(envelope.preconditions())?;
                let payload: SessionReasonPayloadV1 = parse_payload(envelope)?;
                validate_reason(&payload.reason)?;
                (
                    payload.selector,
                    SliceCommandV1::SessionBlock(SessionBlockV1 {
                        reason: payload.reason,
                        preconditions,
                    }),
                )
            }
            "session.unblock" => {
                require_envelope(envelope, "session.unblock", OperationV1::Mutate, true)?;
                let preconditions = require_session_preconditions(envelope.preconditions())?;
                let payload: SessionUnblockPayloadV1 = parse_payload(envelope)?;
                if payload.all == payload.blocker_id.is_some() {
                    return Err(SliceErrorV1::InvalidValue {
                        field: "unblock blocker_id/all",
                    });
                }
                (
                    payload.selector,
                    SliceCommandV1::SessionUnblock(SessionUnblockV1 {
                        blocker_id: payload.blocker_id,
                        all: payload.all,
                        preconditions,
                    }),
                )
            }
            "session.cancel" => {
                require_envelope(envelope, "session.cancel", OperationV1::Mutate, true)?;
                let preconditions = require_session_preconditions(envelope.preconditions())?;
                let payload: SessionReasonPayloadV1 = parse_payload(envelope)?;
                validate_reason(&payload.reason)?;
                (
                    payload.selector,
                    SliceCommandV1::SessionCancel(SessionCancelV1 {
                        reason: payload.reason,
                        preconditions,
                    }),
                )
            }
            "session.reopen" => {
                let payload: SessionStageReasonPayloadV1 = parse_payload(envelope)?;
                require_dry_run_envelope(envelope, "session.reopen", payload.dry_run)?;
                let preconditions =
                    require_session_revision_preconditions(envelope.preconditions())?;
                validate_reason(&payload.reason)?;
                (
                    payload.selector,
                    SliceCommandV1::SessionReopen(SessionReopenV1 {
                        destination_stage_id: payload.destination_stage_id,
                        reason: payload.reason,
                        dry_run: payload.dry_run,
                        preconditions,
                    }),
                )
            }
            "session.reset" => {
                let payload: SessionResetPayloadV1 = parse_payload(envelope)?;
                require_dry_run_envelope(envelope, "session.reset", payload.dry_run)?;
                let preconditions =
                    require_session_identity_preconditions(envelope.preconditions())?;
                let confirmed = match (payload.dry_run, payload.confirmed) {
                    (true, None) => false,
                    (true, Some(_)) => {
                        return Err(SliceErrorV1::InvalidValue { field: "confirmed" });
                    }
                    (false, confirmed) => {
                        let confirmed = confirmed.unwrap_or(false);
                        require_confirmation(confirmed)?;
                        confirmed
                    }
                };
                (
                    payload.selector,
                    SliceCommandV1::SessionReset(SessionResetV1 {
                        confirmed,
                        dry_run: payload.dry_run,
                        preconditions,
                    }),
                )
            }
            "workspace.reset_all" => {
                require_envelope(
                    envelope,
                    "workspace.reset_all",
                    OperationV1::Bootstrap,
                    true,
                )?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: WorkspaceResetAllPayloadV1 = parse_payload(envelope)?;
                require_confirmation(payload.confirmed)?;
                require_selector_workspace_consistency(
                    &payload.selector,
                    payload.expected_workspace_uuid.as_ref(),
                )?;
                let preconditions = WorkspaceResetAllPreconditionsWireV1 {
                    expected_workspace_id: payload.expected_workspace_uuid,
                };
                (
                    payload.selector,
                    SliceCommandV1::WorkspaceResetAll(WorkspaceResetAllV1 {
                        confirmed: payload.confirmed,
                        preconditions,
                    }),
                )
            }
            "item.check" => {
                require_envelope(envelope, "item.check", OperationV1::Mutate, true)?;
                let preconditions = require_item_preconditions(envelope.preconditions())?;
                let payload: ItemIdPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::ItemCheck(ItemCheckV1 {
                        item_id: payload.item_id,
                        preconditions,
                    }),
                )
            }
            "item.uncheck" => {
                require_envelope(envelope, "item.uncheck", OperationV1::Mutate, true)?;
                let preconditions = require_item_preconditions(envelope.preconditions())?;
                let payload: ItemIdPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::ItemUncheck(ItemUncheckV1 {
                        item_id: payload.item_id,
                        preconditions,
                    }),
                )
            }
            "item.set" => {
                require_envelope(envelope, "item.set", OperationV1::Mutate, true)?;
                let preconditions = require_item_preconditions(envelope.preconditions())?;
                let payload: ItemValuePayloadV1 = parse_payload(envelope)?;
                validate_item_text(&payload.value)?;
                (
                    payload.selector,
                    SliceCommandV1::ItemSet(ItemSetV1 {
                        item_id: payload.item_id,
                        value: payload.value,
                        preconditions,
                    }),
                )
            }
            "item.add" => {
                require_envelope(envelope, "item.add", OperationV1::Mutate, true)?;
                let preconditions = require_item_preconditions(envelope.preconditions())?;
                let payload: ItemValuePayloadV1 = parse_payload(envelope)?;
                validate_list_value(&payload.value)?;
                (
                    payload.selector,
                    SliceCommandV1::ItemAdd(ItemAddV1 {
                        item_id: payload.item_id,
                        value: payload.value,
                        preconditions,
                    }),
                )
            }
            "item.remove" => {
                require_envelope(envelope, "item.remove", OperationV1::Mutate, true)?;
                let preconditions = require_item_preconditions(envelope.preconditions())?;
                let payload: ItemRemovePayloadV1 = parse_payload(envelope)?;
                validate_list_value(&payload.value)?;
                (
                    payload.selector,
                    SliceCommandV1::ItemRemove(ItemRemoveV1 {
                        item_id: payload.item_id,
                        value: payload.value,
                        ignore_missing: payload.ignore_missing,
                        preconditions,
                    }),
                )
            }
            "item.attach" => {
                require_envelope(envelope, "item.attach", OperationV1::Mutate, true)?;
                let preconditions = require_item_preconditions(envelope.preconditions())?;
                let payload: ItemAttachPayloadV1 = parse_payload(envelope)?;
                let source = validated_attach_source(
                    payload.path,
                    payload.reference,
                    payload.digest,
                    payload.size_bytes,
                    payload.media_type,
                )?;
                (
                    payload.selector,
                    SliceCommandV1::ItemAttach(ItemAttachV1 {
                        item_id: payload.item_id,
                        source,
                        preconditions,
                    }),
                )
            }
            "item.clear" => {
                require_envelope(envelope, "item.clear", OperationV1::Mutate, true)?;
                let preconditions = require_item_preconditions(envelope.preconditions())?;
                let payload: ItemIdPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::ItemClear(ItemClearV1 {
                        item_id: payload.item_id,
                        preconditions,
                    }),
                )
            }
            "job.list" => {
                require_envelope(envelope, "job.list", OperationV1::Query, false)?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: JobListPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::JobList(JobListV1 {
                        state: payload.state,
                    }),
                )
            }
            "job.lookup" => {
                require_envelope(envelope, "job.lookup", OperationV1::Query, false)?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: JobLookupPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::JobLookup(JobLookupV1 {
                        idempotency_key: payload.idempotency_key,
                    }),
                )
            }
            "job.status" => {
                require_envelope(envelope, "job.status", OperationV1::Query, false)?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: JobIdPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::JobStatus(JobStatusV1 {
                        job_id: payload.job_id,
                    }),
                )
            }
            "job.wait" => {
                require_envelope(envelope, "job.wait", OperationV1::Query, false)?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: JobIdPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::JobWait(JobWaitV1 {
                        job_id: payload.job_id,
                    }),
                )
            }
            "job.cancel" => {
                require_envelope(envelope, "job.cancel", OperationV1::Control, false)?;
                let preconditions = require_job_preconditions(envelope.preconditions())?;
                let payload: JobIdPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::JobCancel(JobCancelV1 {
                        job_id: payload.job_id,
                        preconditions,
                    }),
                )
            }
            _ => {
                return Err(SliceErrorV1::InvalidCommand {
                    received: command_name.to_owned(),
                });
            }
        };
        let workspace = envelope.workspace().ok_or(SliceErrorV1::MissingWorkspace {
            command: command.command_name(),
        })?;
        if workspace.expected_uuid() != selector.expected_uuid() {
            return Err(SliceErrorV1::InvalidValue {
                field: "workspace.expected_uuid/selector.expected_uuid",
            });
        }
        Ok(Self { selector, command })
    }

    pub fn selector(&self) -> &WorktreeSelectorWireV1 {
        &self.selector
    }

    pub fn command(&self) -> &SliceCommandV1 {
        &self.command
    }
}

impl TryFrom<&RequestEnvelopeV1> for SliceRequestV1 {
    type Error = SliceErrorV1;

    fn try_from(envelope: &RequestEnvelopeV1) -> Result<Self, Self::Error> {
        Self::from_envelope(envelope)
    }
}

impl ProcedureV2MutationRequestV1 {
    pub fn from_envelope(envelope: &RequestEnvelopeV1) -> Result<Self, SliceErrorV1> {
        let (selector, command) = match envelope.command().as_str() {
            "session.decide" => {
                require_envelope(envelope, "session.decide", OperationV1::Mutate, true)?;
                let preconditions = require_session_preconditions(envelope.preconditions())?;
                let payload: SessionDecidePayloadV2 = parse_payload(envelope)?;
                validate_reason_v2(&payload.reason)?;
                validate_actor_v2(payload.actor.as_deref())?;
                (
                    payload.selector,
                    ProcedureV2MutationCommandV1::SessionDecide(SessionDecideV2 {
                        option_id: payload.option_id,
                        reason: payload.reason,
                        actor: payload.actor,
                        preconditions,
                    }),
                )
            }
            "session.rework" => {
                require_envelope(envelope, "session.rework", OperationV1::Mutate, true)?;
                let preconditions = require_rework_preconditions_v2(envelope.preconditions())?;
                let payload: SessionReworkPayloadV2 = parse_payload(envelope)?;
                validate_reason_v2(&payload.reason)?;
                validate_actor_v2(payload.actor.as_deref())?;
                (
                    payload.selector,
                    ProcedureV2MutationCommandV1::SessionRework(SessionReworkV2 {
                        target_graph_node_id: payload.target_graph_node_id,
                        reason: payload.reason,
                        actor: payload.actor,
                        preconditions,
                    }),
                )
            }
            "goal.define" => {
                require_envelope(envelope, "goal.define", OperationV1::Mutate, true)?;
                let preconditions =
                    require_session_identity_preconditions(envelope.preconditions())?;
                let payload: GoalDefinePayloadV2 = parse_payload(envelope)?;
                validate_goal_definition_v2(&payload.goal, &payload.criteria)?;
                validate_actor_v2(payload.actor.as_deref())?;
                (
                    payload.selector,
                    ProcedureV2MutationCommandV1::GoalDefine(GoalDefineV2 {
                        goal: payload.goal,
                        criteria: payload.criteria,
                        actor: payload.actor,
                        preconditions,
                    }),
                )
            }
            "goal.revise" => {
                require_envelope(envelope, "goal.revise", OperationV1::Mutate, true)?;
                let preconditions =
                    require_goal_revision_preconditions_v2(envelope.preconditions())?;
                let payload: GoalRevisePayloadV2 = parse_payload(envelope)?;
                validate_goal_definition_v2(&payload.goal, &payload.criteria)?;
                GoalRevisionReasonV2::new(payload.reason.clone()).map_err(domain_value_error_v2)?;
                validate_actor_v2(payload.actor.as_deref())?;
                (
                    payload.selector,
                    ProcedureV2MutationCommandV1::GoalRevise(GoalReviseV2 {
                        goal: payload.goal,
                        criteria: payload.criteria,
                        target_graph_node_id: payload.target_graph_node_id,
                        reason: payload.reason,
                        actor: payload.actor,
                        reactivate: payload.reactivate,
                        preconditions,
                    }),
                )
            }
            "goal.assess_criterion" => {
                require_envelope(envelope, "goal.assess_criterion", OperationV1::Mutate, true)?;
                let expected_goal_revision = require_goal_revision_v2(envelope.preconditions())?;
                let preconditions = require_session_preconditions_v2(envelope.preconditions())?;
                let payload: GoalAssessCriterionPayloadV2 = parse_payload(envelope)?;
                let status = payload
                    .status
                    .parse::<CriterionStatusV2>()
                    .map_err(domain_value_error_v2)?;
                CriterionAssessmentReasonV2::new(payload.reason.clone())
                    .map_err(domain_value_error_v2)?;
                validate_actor_v2(payload.actor.as_deref())?;
                validate_citations_v2(status, &payload.evidence, &payload.items)?;
                (
                    payload.selector,
                    ProcedureV2MutationCommandV1::GoalAssessCriterion(GoalAssessCriterionV2 {
                        criterion_id: payload.criterion_id,
                        status: status.as_str().to_owned(),
                        reason: payload.reason,
                        evidence: payload.evidence,
                        items: payload.items,
                        actor: payload.actor,
                        preconditions,
                        expected_goal_revision: expected_goal_revision.get(),
                    }),
                )
            }
            command => {
                return Err(SliceErrorV1::InvalidCommand {
                    received: command.to_owned(),
                });
            }
        };
        let workspace = envelope.workspace().ok_or(SliceErrorV1::MissingWorkspace {
            command: command.command_name(),
        })?;
        if workspace.expected_uuid() != selector.expected_uuid() {
            return Err(SliceErrorV1::InvalidValue {
                field: "workspace.expected_uuid/selector.expected_uuid",
            });
        }
        Ok(Self { selector, command })
    }

    pub fn selector(&self) -> &WorktreeSelectorWireV1 {
        &self.selector
    }

    pub fn command(&self) -> &ProcedureV2MutationCommandV1 {
        &self.command
    }
}

impl TryFrom<&RequestEnvelopeV1> for ProcedureV2MutationRequestV1 {
    type Error = SliceErrorV1;

    fn try_from(envelope: &RequestEnvelopeV1) -> Result<Self, Self::Error> {
        Self::from_envelope(envelope)
    }
}

impl ProcedureV2StartRequestV1 {
    /// Decodes the complete closed Procedure v2 start payload without registering it for dispatch.
    pub fn from_envelope(envelope: &RequestEnvelopeV1) -> Result<Self, SliceErrorV1> {
        let (selector, command) = match envelope.command().as_str() {
            "session.start" => {
                let payload: ProcedureV2SessionStartPayloadV1 = parse_payload(envelope)?;
                require_dry_run_envelope(envelope, "session.start", payload.dry_run)?;
                require_no_preconditions(envelope.preconditions())?;
                let start = validated_start(
                    payload.preset,
                    payload.procedure,
                    payload.expected_procedure_digest,
                    payload.task_title,
                    payload.dry_run,
                )?;
                let initial_goal =
                    validated_initial_goal_v2(payload.goal, payload.criteria, payload.actor)?;
                (
                    payload.selector,
                    ProcedureV2StartCommandV1::SessionStart(ProcedureV2SessionStartV1 {
                        start,
                        initial_goal,
                    }),
                )
            }
            "session.start_replace" => {
                let payload: ProcedureV2SessionStartReplacePayloadV1 = parse_payload(envelope)?;
                require_dry_run_envelope(envelope, "session.start_replace", payload.dry_run)?;
                let preconditions =
                    require_session_identity_preconditions(envelope.preconditions())?;
                let confirmed = match (payload.dry_run, payload.confirmed) {
                    (true, None) => false,
                    (true, Some(_)) => {
                        return Err(SliceErrorV1::InvalidValue { field: "confirmed" });
                    }
                    (false, confirmed) => {
                        let confirmed = confirmed.unwrap_or(false);
                        require_confirmation(confirmed)?;
                        confirmed
                    }
                };
                let start = validated_start(
                    payload.preset,
                    payload.procedure,
                    payload.expected_procedure_digest,
                    payload.task_title,
                    payload.dry_run,
                )?;
                let initial_goal =
                    validated_initial_goal_v2(payload.goal, payload.criteria, payload.actor)?;
                (
                    payload.selector,
                    ProcedureV2StartCommandV1::SessionStartReplace(
                        ProcedureV2SessionStartReplaceV1 {
                            start: ProcedureV2SessionStartV1 {
                                start,
                                initial_goal,
                            },
                            confirmed,
                            preconditions,
                        },
                    ),
                )
            }
            command => {
                return Err(SliceErrorV1::InvalidCommand {
                    received: command.to_owned(),
                });
            }
        };
        let workspace = envelope.workspace().ok_or(SliceErrorV1::MissingWorkspace {
            command: command.command_name(),
        })?;
        if workspace.expected_uuid() != selector.expected_uuid() {
            return Err(SliceErrorV1::InvalidValue {
                field: "workspace.expected_uuid/selector.expected_uuid",
            });
        }
        Ok(Self { selector, command })
    }

    pub fn selector(&self) -> &WorktreeSelectorWireV1 {
        &self.selector
    }

    pub fn command(&self) -> &ProcedureV2StartCommandV1 {
        &self.command
    }
}

impl TryFrom<&RequestEnvelopeV1> for ProcedureV2StartRequestV1 {
    type Error = SliceErrorV1;

    fn try_from(envelope: &RequestEnvelopeV1) -> Result<Self, Self::Error> {
        Self::from_envelope(envelope)
    }
}

/// Decodes the optional Procedure v2 revision 1 fields from a start payload.
///
/// This extension-only helper intentionally ignores base start fields. Use
/// [`ProcedureV2StartRequestV1::from_envelope`] at the complete request boundary.
pub fn decode_initial_goal_v2(
    payload: &Map<String, Value>,
) -> Result<Option<InitialGoalWireV2>, SliceErrorV1> {
    let decode = |field: &'static str| -> Result<Option<Value>, SliceErrorV1> {
        match payload.get(field) {
            None => Ok(None),
            Some(Value::Null) => Err(SliceErrorV1::InvalidValue { field }),
            Some(value) => Ok(Some(value.clone())),
        }
    };
    let goal = decode("goal")?
        .map(serde_json::from_value::<String>)
        .transpose()
        .map_err(|error| SliceErrorV1::InvalidPayload {
            message: error.to_string(),
        })?;
    let criteria = decode("criteria")?
        .map(serde_json::from_value::<Vec<GoalCriterionWireV2>>)
        .transpose()
        .map_err(|error| SliceErrorV1::InvalidPayload {
            message: error.to_string(),
        })?;
    let actor = decode("actor")?
        .map(serde_json::from_value::<String>)
        .transpose()
        .map_err(|error| SliceErrorV1::InvalidPayload {
            message: error.to_string(),
        })?;
    validated_initial_goal_v2(goal, criteria, actor)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectorOnlyPayloadV1 {
    selector: WorktreeSelectorWireV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceInitPayloadV1 {
    selector: WorktreeSelectorWireV1,
    #[serde(default)]
    repair: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceDoctorPayloadV1 {
    selector: WorktreeSelectorWireV1,
    deep: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionStartPayloadV1 {
    selector: WorktreeSelectorWireV1,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    preset: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    procedure: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    expected_procedure_digest: Option<Sha256Digest>,
    task_title: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionStartReplacePayloadV1 {
    selector: WorktreeSelectorWireV1,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    preset: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    procedure: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    expected_procedure_digest: Option<Sha256Digest>,
    task_title: String,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    confirmed: Option<bool>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcedureV2SessionStartPayloadV1 {
    selector: WorktreeSelectorWireV1,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    preset: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    procedure: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    expected_procedure_digest: Option<Sha256Digest>,
    task_title: String,
    #[serde(default)]
    dry_run: bool,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    goal: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    criteria: Option<Vec<GoalCriterionWireV2>>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    actor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcedureV2SessionStartReplacePayloadV1 {
    selector: WorktreeSelectorWireV1,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    preset: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    procedure: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    expected_procedure_digest: Option<Sha256Digest>,
    task_title: String,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    confirmed: Option<bool>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    goal: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    criteria: Option<Vec<GoalCriterionWireV2>>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    actor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionStatusPayloadV1 {
    selector: WorktreeSelectorWireV1,
    #[serde(default)]
    wait_for_idle: bool,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    after_job_id: Option<JobId>,
    #[serde(default)]
    verbose: bool,
    #[serde(default)]
    compact: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionNextPayloadV1 {
    selector: WorktreeSelectorWireV1,
    #[serde(default)]
    wait_for_idle: bool,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    after_job_id: Option<JobId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionSkipPayloadV1 {
    selector: WorktreeSelectorWireV1,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionReasonPayloadV1 {
    selector: WorktreeSelectorWireV1,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionStageReasonPayloadV1 {
    selector: WorktreeSelectorWireV1,
    destination_stage_id: StageId,
    reason: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionUnblockPayloadV1 {
    selector: WorktreeSelectorWireV1,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    blocker_id: Option<BlockerId>,
    all: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionResetPayloadV1 {
    selector: WorktreeSelectorWireV1,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    confirmed: Option<bool>,
    #[serde(default)]
    dry_run: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceResetAllPayloadV1 {
    selector: WorktreeSelectorWireV1,
    confirmed: bool,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    expected_workspace_uuid: Option<WorkspaceId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemIdPayloadV1 {
    selector: WorktreeSelectorWireV1,
    item_id: ItemId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemValuePayloadV1 {
    selector: WorktreeSelectorWireV1,
    item_id: ItemId,
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemRemovePayloadV1 {
    selector: WorktreeSelectorWireV1,
    item_id: ItemId,
    value: String,
    #[serde(default)]
    ignore_missing: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemAttachPayloadV1 {
    selector: WorktreeSelectorWireV1,
    item_id: ItemId,
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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobListPayloadV1 {
    selector: WorktreeSelectorWireV1,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    state: Option<crate::JobStateV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobLookupPayloadV1 {
    selector: WorktreeSelectorWireV1,
    idempotency_key: IdempotencyKeyV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobIdPayloadV1 {
    selector: WorktreeSelectorWireV1,
    job_id: JobId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionDecidePayloadV2 {
    selector: WorktreeSelectorWireV1,
    option_id: OptionId,
    reason: String,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    actor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionReworkPayloadV2 {
    selector: WorktreeSelectorWireV1,
    target_graph_node_id: GraphNodeId,
    reason: String,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    actor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoalDefinePayloadV2 {
    selector: WorktreeSelectorWireV1,
    goal: String,
    criteria: Vec<GoalCriterionWireV2>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    actor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoalRevisePayloadV2 {
    selector: WorktreeSelectorWireV1,
    goal: String,
    criteria: Vec<GoalCriterionWireV2>,
    target_graph_node_id: GraphNodeId,
    reason: String,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    actor: Option<String>,
    #[serde(default)]
    reactivate: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoalAssessCriterionPayloadV2 {
    selector: WorktreeSelectorWireV1,
    criterion_id: CriterionId,
    status: String,
    reason: String,
    #[serde(default)]
    evidence: Vec<GraphNodeId>,
    #[serde(default)]
    items: Vec<ItemId>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    actor: Option<String>,
}

fn parse_payload<T: DeserializeOwned>(envelope: &RequestEnvelopeV1) -> Result<T, SliceErrorV1> {
    serde_json::from_value(Value::Object(envelope.payload().clone())).map_err(|error| {
        SliceErrorV1::InvalidPayload {
            message: error.to_string(),
        }
    })
}

fn domain_value_error_v2(error: podway_core::DomainError) -> SliceErrorV1 {
    SliceErrorV1::InvalidPayload {
        message: error.to_string(),
    }
}

fn validate_reason_v2(value: &str) -> Result<(), SliceErrorV1> {
    ReasonV2::new(value.to_owned())
        .map(drop)
        .map_err(domain_value_error_v2)
}

fn validate_actor_v2(value: Option<&str>) -> Result<(), SliceErrorV1> {
    value
        .map(|value| ActorAttributionV2::new(value.to_owned()))
        .transpose()
        .map(drop)
        .map_err(domain_value_error_v2)
}

fn validate_goal_definition_v2(
    goal: &str,
    criteria: &[GoalCriterionWireV2],
) -> Result<(), SliceErrorV1> {
    GoalStatementV2::new(goal.to_owned()).map_err(domain_value_error_v2)?;
    let criteria = criteria
        .iter()
        .map(|criterion| {
            GoalCriterionV2::new(criterion.criterion_id.clone(), criterion.statement.clone())
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(domain_value_error_v2)?;
    GoalDefinitionV2::new(criteria)
        .map(drop)
        .map_err(domain_value_error_v2)
}

fn validated_initial_goal_v2(
    goal: Option<String>,
    criteria: Option<Vec<GoalCriterionWireV2>>,
    actor: Option<String>,
) -> Result<Option<InitialGoalWireV2>, SliceErrorV1> {
    match (goal, criteria, actor) {
        (None, None, None) => Ok(None),
        (Some(goal), Some(criteria), actor) => {
            validate_goal_definition_v2(&goal, &criteria)?;
            validate_actor_v2(actor.as_deref())?;
            Ok(Some(InitialGoalWireV2 {
                goal,
                criteria,
                actor,
            }))
        }
        _ => Err(SliceErrorV1::InvalidValue {
            field: "goal/criteria/actor",
        }),
    }
}

fn require_goal_revision_v2(
    preconditions: &PreconditionsV1,
) -> Result<GoalRevisionNumberV2, SliceErrorV1> {
    preconditions
        .goal_revision()
        .ok_or(SliceErrorV1::MissingPrecondition {
            field: "preconditions.goal_revision",
        })
}

fn require_goal_revision_preconditions_v2(
    preconditions: &PreconditionsV1,
) -> Result<GoalRevisionPreconditionsWireV2, SliceErrorV1> {
    let expected_session_id =
        preconditions
            .session_id()
            .cloned()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_id",
            })?;
    let expected_session_revision =
        preconditions
            .session_revision()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_revision",
            })?;
    let expected_goal_revision = require_goal_revision_v2(preconditions)?.get();
    reject_non_session_preconditions_v2(preconditions)?;
    Ok(GoalRevisionPreconditionsWireV2 {
        expected_session_id,
        expected_session_revision,
        expected_attempt_id: preconditions.attempt_id().cloned(),
        expected_goal_revision,
    })
}

fn require_rework_preconditions_v2(
    preconditions: &PreconditionsV1,
) -> Result<ReworkPreconditionsWireV2, SliceErrorV1> {
    let expected_session_id =
        preconditions
            .session_id()
            .cloned()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_id",
            })?;
    let expected_session_revision =
        preconditions
            .session_revision()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_revision",
            })?;
    reject_non_session_preconditions_v2(preconditions)?;
    if preconditions.goal_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.goal_revision",
        });
    }
    Ok(ReworkPreconditionsWireV2 {
        expected_session_id,
        expected_session_revision,
        expected_attempt_id: preconditions.attempt_id().cloned(),
    })
}

fn reject_non_session_preconditions_v2(
    preconditions: &PreconditionsV1,
) -> Result<(), SliceErrorV1> {
    for (present, field) in [
        (
            preconditions.item_revision().is_some(),
            "preconditions.item_revision",
        ),
        (
            preconditions.blocker_id().is_some(),
            "preconditions.blocker_id",
        ),
        (
            preconditions.job_state().is_some(),
            "preconditions.job_state",
        ),
    ] {
        if present {
            return Err(SliceErrorV1::UnexpectedPrecondition { field });
        }
    }
    Ok(())
}

fn validate_citations_v2(
    status: CriterionStatusV2,
    evidence: &[GraphNodeId],
    items: &[ItemId],
) -> Result<(), SliceErrorV1> {
    let count = evidence.len().saturating_add(items.len());
    if count > 4
        || evidence
            .iter()
            .enumerate()
            .any(|(index, value)| evidence[..index].contains(value))
        || items
            .iter()
            .enumerate()
            .any(|(index, value)| items[..index].contains(value))
        || (status == CriterionStatusV2::NotApplicable && count != 0)
    {
        return Err(SliceErrorV1::InvalidValue {
            field: "evidence/items",
        });
    }
    Ok(())
}

fn require_envelope(
    envelope: &RequestEnvelopeV1,
    command: &'static str,
    operation: OperationV1,
    durable_job: bool,
) -> Result<(), SliceErrorV1> {
    if envelope.operation() != operation {
        return Err(SliceErrorV1::OperationMismatch {
            command,
            expected: operation,
            received: envelope.operation(),
        });
    }
    if envelope.workspace().is_none() {
        return Err(SliceErrorV1::MissingWorkspace { command });
    }
    match (durable_job, envelope.idempotency_key().is_some()) {
        (true, false) => return Err(SliceErrorV1::MissingIdempotencyKey { command }),
        (false, true) => return Err(SliceErrorV1::UnexpectedIdempotencyKey { command }),
        _ => {}
    }
    Ok(())
}
fn require_dry_run_envelope(
    envelope: &RequestEnvelopeV1,
    command: &'static str,
    dry_run: bool,
) -> Result<(), SliceErrorV1> {
    require_envelope(
        envelope,
        command,
        if dry_run {
            OperationV1::Query
        } else {
            OperationV1::Mutate
        },
        !dry_run,
    )
}

fn require_no_preconditions(preconditions: &PreconditionsV1) -> Result<(), SliceErrorV1> {
    reject_goal_revision_precondition(preconditions)?;
    if preconditions.session_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.session_id",
        });
    }
    if preconditions.session_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.session_revision",
        });
    }
    if preconditions.attempt_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.attempt_id",
        });
    }
    if preconditions.item_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.item_revision",
        });
    }
    if preconditions.blocker_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.blocker_id",
        });
    }
    if preconditions.job_state().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.job_state",
        });
    }
    Ok(())
}

fn require_item_preconditions(
    preconditions: &PreconditionsV1,
) -> Result<ItemMutationPreconditionsWireV1, SliceErrorV1> {
    reject_goal_revision_precondition(preconditions)?;
    let expected_session_id =
        preconditions
            .session_id()
            .cloned()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_id",
            })?;
    let expected_attempt_id =
        preconditions
            .attempt_id()
            .cloned()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.attempt_id",
            })?;
    let expected_item_revision =
        preconditions
            .item_revision()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.item_revision",
            })?;
    if preconditions.session_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.session_revision",
        });
    }
    if preconditions.blocker_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.blocker_id",
        });
    }
    if preconditions.job_state().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.job_state",
        });
    }
    Ok(ItemMutationPreconditionsWireV1 {
        expected_session_id,
        expected_attempt_id,
        expected_item_revision,
    })
}

fn require_session_preconditions(
    preconditions: &PreconditionsV1,
) -> Result<SessionMutationPreconditionsWireV1, SliceErrorV1> {
    reject_goal_revision_precondition(preconditions)?;
    let expected_session_id =
        preconditions
            .session_id()
            .cloned()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_id",
            })?;
    let expected_session_revision =
        preconditions
            .session_revision()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_revision",
            })?;
    let expected_attempt_id =
        preconditions
            .attempt_id()
            .cloned()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.attempt_id",
            })?;
    if preconditions.item_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.item_revision",
        });
    }
    if preconditions.blocker_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.blocker_id",
        });
    }
    if preconditions.job_state().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.job_state",
        });
    }
    Ok(SessionMutationPreconditionsWireV1 {
        expected_session_id,
        expected_session_revision,
        expected_attempt_id,
    })
}

fn require_session_preconditions_v2(
    preconditions: &PreconditionsV1,
) -> Result<SessionMutationPreconditionsWireV1, SliceErrorV1> {
    let expected_session_id =
        preconditions
            .session_id()
            .cloned()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_id",
            })?;
    let expected_session_revision =
        preconditions
            .session_revision()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_revision",
            })?;
    let expected_attempt_id =
        preconditions
            .attempt_id()
            .cloned()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.attempt_id",
            })?;
    reject_non_session_preconditions_v2(preconditions)?;
    Ok(SessionMutationPreconditionsWireV1 {
        expected_session_id,
        expected_session_revision,
        expected_attempt_id,
    })
}

fn require_session_identity_preconditions(
    preconditions: &PreconditionsV1,
) -> Result<SessionIdentityPreconditionsWireV1, SliceErrorV1> {
    reject_goal_revision_precondition(preconditions)?;
    let expected_session_id =
        preconditions
            .session_id()
            .cloned()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_id",
            })?;
    let expected_session_revision =
        preconditions
            .session_revision()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_revision",
            })?;
    if preconditions.attempt_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.attempt_id",
        });
    }
    if preconditions.item_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.item_revision",
        });
    }
    if preconditions.blocker_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.blocker_id",
        });
    }
    if preconditions.job_state().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.job_state",
        });
    }
    Ok(SessionIdentityPreconditionsWireV1 {
        expected_session_id,
        expected_session_revision,
    })
}

fn require_session_revision_preconditions(
    preconditions: &PreconditionsV1,
) -> Result<SessionRevisionPreconditionsWireV1, SliceErrorV1> {
    reject_goal_revision_precondition(preconditions)?;
    let expected_session_id =
        preconditions
            .session_id()
            .cloned()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_id",
            })?;
    let expected_session_revision =
        preconditions
            .session_revision()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.session_revision",
            })?;
    if preconditions.attempt_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.attempt_id",
        });
    }
    if preconditions.item_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.item_revision",
        });
    }
    if preconditions.blocker_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.blocker_id",
        });
    }
    if preconditions.job_state().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.job_state",
        });
    }
    Ok(SessionRevisionPreconditionsWireV1 {
        expected_session_id,
        expected_session_revision,
    })
}

fn require_session_read_preconditions(
    preconditions: &PreconditionsV1,
) -> Result<SessionReadPreconditionsWireV1, SliceErrorV1> {
    reject_goal_revision_precondition(preconditions)?;
    if preconditions.session_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.session_revision",
        });
    }
    if preconditions.attempt_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.attempt_id",
        });
    }
    if preconditions.item_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.item_revision",
        });
    }
    if preconditions.blocker_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.blocker_id",
        });
    }
    if preconditions.job_state().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.job_state",
        });
    }
    Ok(SessionReadPreconditionsWireV1 {
        expected_session_id: preconditions.session_id().cloned(),
    })
}

fn require_job_preconditions(
    preconditions: &PreconditionsV1,
) -> Result<JobMutationPreconditionsWireV1, SliceErrorV1> {
    reject_goal_revision_precondition(preconditions)?;
    let expected_job_state =
        preconditions
            .job_state()
            .ok_or(SliceErrorV1::MissingPrecondition {
                field: "preconditions.job_state",
            })?;
    if preconditions.session_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.session_id",
        });
    }
    if preconditions.session_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.session_revision",
        });
    }
    if preconditions.attempt_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.attempt_id",
        });
    }
    if preconditions.item_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.item_revision",
        });
    }
    if preconditions.blocker_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.blocker_id",
        });
    }
    Ok(JobMutationPreconditionsWireV1 { expected_job_state })
}

fn reject_goal_revision_precondition(preconditions: &PreconditionsV1) -> Result<(), SliceErrorV1> {
    if preconditions.goal_revision().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.goal_revision",
        });
    }
    Ok(())
}

fn validated_start(
    preset: Option<String>,
    procedure: Option<String>,
    expected_procedure_digest: Option<Sha256Digest>,
    task_title: String,
    dry_run: bool,
) -> Result<SessionStartV1, SliceErrorV1> {
    validate_title(&task_title)?;
    let source = match (preset, procedure) {
        (Some(preset), None) if expected_procedure_digest.is_none() => {
            validate_preset(&preset)?;
            SessionStartSourceV1::Preset { preset }
        }
        (None, Some(procedure)) => {
            validate_safe_artifact_path(&procedure)?;
            SessionStartSourceV1::Procedure { procedure }
        }
        _ => {
            return Err(SliceErrorV1::InvalidValue {
                field: "preset/procedure",
            });
        }
    };
    Ok(SessionStartV1 {
        source,
        expected_procedure_digest,
        task_title,
        dry_run,
    })
}

fn validated_query_wait(
    wait_for_idle: bool,
    after_job_id: Option<JobId>,
) -> Result<QueryWaitV1, SliceErrorV1> {
    match (wait_for_idle, after_job_id) {
        (false, None) => Ok(QueryWaitV1::Immediate),
        (true, None) => Ok(QueryWaitV1::Idle),
        (false, Some(job_id)) => Ok(QueryWaitV1::AfterJob { job_id }),
        (true, Some(_)) => Err(SliceErrorV1::InvalidValue {
            field: "wait_for_idle/after_job_id",
        }),
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

fn require_confirmation(confirmed: bool) -> Result<(), SliceErrorV1> {
    if !confirmed {
        return Err(SliceErrorV1::InvalidValue { field: "confirmed" });
    }
    Ok(())
}

fn require_selector_workspace_consistency(
    selector: &WorktreeSelectorWireV1,
    expected_workspace_id: Option<&WorkspaceId>,
) -> Result<(), SliceErrorV1> {
    if selector.expected_uuid() != expected_workspace_id {
        return Err(SliceErrorV1::InvalidValue {
            field: "expected_workspace_uuid",
        });
    }
    Ok(())
}

fn validate_preset(value: &str) -> Result<(), SliceErrorV1> {
    validate_non_empty_bytes(value, 64, "preset")?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || value.starts_with('-')
        || value.ends_with('-')
    {
        return Err(SliceErrorV1::InvalidValue { field: "preset" });
    }
    Ok(())
}

fn validate_title(value: &str) -> Result<(), SliceErrorV1> {
    if value.trim().is_empty() {
        return Err(SliceErrorV1::EmptyValue {
            field: "task_title",
        });
    }
    validate_scalar_bound(value, MAX_SLICE_TASK_TITLE_SCALARS_V1, "task_title")
}

fn validate_reason(value: &str) -> Result<(), SliceErrorV1> {
    if value.trim().is_empty() {
        return Err(SliceErrorV1::EmptyValue { field: "reason" });
    }
    validate_scalar_bound(value, MAX_SLICE_REASON_SCALARS_V1, "reason")
}

fn validate_item_text(value: &str) -> Result<(), SliceErrorV1> {
    validate_scalar_bound(value, MAX_SLICE_ITEM_TEXT_SCALARS_V1, "value")
}

fn validate_list_value(value: &str) -> Result<(), SliceErrorV1> {
    if value.trim().is_empty() {
        return Err(SliceErrorV1::EmptyValue { field: "value" });
    }
    validate_scalar_bound(value, MAX_SLICE_LIST_VALUE_SCALARS_V1, "value")
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

/// Canonical semantic identity for one reserved Procedure v2 mutation.
///
/// It excludes request correlation, client metadata, selector hints, detach, and wait timeout in
/// the same way as the released mutation identity. Every semantic payload field and concurrency
/// fence is retained so an idempotency key cannot be rebound to different v2 work.
pub fn canonical_procedure_v2_mutation_identity_v1(
    request: &ProcedureV2MutationRequestV1,
    resolved_workspace_id: &WorkspaceId,
) -> Result<String, SliceErrorV1> {
    let (preconditions, payload) = match request.command() {
        ProcedureV2MutationCommandV1::SessionDecide(command) => (
            session_preconditions_json(&command.preconditions),
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
        "command": request.command().command_name(),
        "workspace_id": resolved_workspace_id,
        "preconditions": preconditions,
        "payload": payload,
    }))
    .map_err(|error| SliceErrorV1::Canonicalization {
        message: error.to_string(),
    })
}

/// Canonical semantic mutation identity. Its UTF-8 bytes are the SHA-256 input.
///
/// Request metadata, transport wait preferences, dry-run mode, and the selector's
/// path/display/expected UUID are deliberately excluded. Dry-run requests are not mutations;
/// the resolved daemon workspace identity is the only workspace identity.
pub fn canonical_mutation_identity_v1(
    request: &SliceRequestV1,
    resolved_workspace_id: &WorkspaceId,
) -> Result<String, SliceErrorV1> {
    let command = request.command();
    if !command.is_mutation() {
        return Err(SliceErrorV1::NotAMutation {
            command: command.command_name(),
        });
    }

    let (preconditions, payload) = match command {
        SliceCommandV1::WorkspaceInit(init) => (json!({}), json!({"repair": init.repair})),
        SliceCommandV1::SessionStart(start) => (json!({}), start_payload_json_v1(start)),
        SliceCommandV1::SessionStartReplace(start) => {
            (session_identity_preconditions_json(&start.preconditions), {
                let mut payload = start_payload_json_v1(&start.start);
                insert_canonical_field_v1(&mut payload, "confirmed", json!(start.confirmed))?;
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
        SliceCommandV1::SessionReturn(session) => (
            session_preconditions_json(&session.preconditions),
            json!({"destination_stage_id": &session.destination_stage_id, "reason": &session.reason}),
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
        SliceCommandV1::SessionReopen(session) => (
            session_revision_preconditions_json(&session.preconditions),
            json!({"destination_stage_id": &session.destination_stage_id, "reason": &session.reason}),
        ),
        SliceCommandV1::SessionReset(session) => (
            session_identity_preconditions_json(&session.preconditions),
            json!({"confirmed": session.confirmed}),
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
        SliceCommandV1::WorkspaceDoctor(_)
        | SliceCommandV1::WorkspaceShow(_)
        | SliceCommandV1::WorkspaceRepair(_)
        | SliceCommandV1::SessionStatus(_)
        | SliceCommandV1::SessionNext(_)
        | SliceCommandV1::JobList(_)
        | SliceCommandV1::JobLookup(_)
        | SliceCommandV1::JobStatus(_)
        | SliceCommandV1::JobWait(_)
        | SliceCommandV1::JobCancel(_) => {
            unreachable!("non-durable commands were rejected above")
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

/// Canonical semantic identity for a Procedure-backed start after daemon resolution.
///
/// The resolved digest is kept separate from the wire request because it is an admission fact,
/// not client-controlled payload. The optional digest guard and replacement identity fences are
/// retained as preconditions, while the validated, defaulted Procedure digest binds retries to
/// the exact Procedure that the daemon observed.
pub fn canonical_start_mutation_identity_v1(
    request: &SliceRequestV1,
    resolved_workspace_id: &WorkspaceId,
    resolved_procedure_digest: &Sha256Digest,
) -> Result<String, SliceErrorV1> {
    let expected_procedure_digest = match request.command() {
        SliceCommandV1::SessionStart(start) => start.expected_procedure_digest.as_ref(),
        SliceCommandV1::SessionStartReplace(start) => {
            start.start.expected_procedure_digest.as_ref()
        }
        _ => {
            return Err(SliceErrorV1::InvalidValue {
                field: "resolved_procedure_digest",
            });
        }
    };
    let canonical = canonical_mutation_identity_v1(request, resolved_workspace_id)?;
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

/// Canonical semantic identity for a protocol-only Procedure v2 start.
///
/// The identity is byte-identical to the retained start identity when no initial goal is present.
/// A supplied revision 1 definition is encoded in authored criterion order and therefore changes
/// the identity without exposing the request through the executable v1 slice.
pub fn canonical_procedure_v2_start_identity_v1(
    request: &ProcedureV2StartRequestV1,
    resolved_workspace_id: &WorkspaceId,
    resolved_procedure_digest: &Sha256Digest,
) -> Result<String, SliceErrorV1> {
    if !request.command.is_mutation() {
        return Err(SliceErrorV1::NotAMutation {
            command: request.command.command_name(),
        });
    }

    let (start, initial_goal, preconditions, confirmed) = match &request.command {
        ProcedureV2StartCommandV1::SessionStart(start) => {
            (&start.start, &start.initial_goal, json!({}), None)
        }
        ProcedureV2StartCommandV1::SessionStartReplace(replace) => (
            &replace.start.start,
            &replace.start.initial_goal,
            session_identity_preconditions_json(&replace.preconditions),
            Some(replace.confirmed),
        ),
    };

    let mut payload = start_payload_json_v1(start);
    if let Some(initial_goal) = initial_goal {
        insert_canonical_field_v1(&mut payload, "goal", json!(&initial_goal.goal))?;
        insert_canonical_field_v1(&mut payload, "criteria", json!(&initial_goal.criteria))?;
        insert_canonical_field_v1(&mut payload, "actor", json!(&initial_goal.actor))?;
    }
    if let Some(confirmed) = confirmed {
        insert_canonical_field_v1(&mut payload, "confirmed", json!(confirmed))?;
    }

    let mut preconditions = preconditions;
    insert_canonical_field_v1(
        &mut preconditions,
        "procedure_digest",
        json!(resolved_procedure_digest),
    )?;
    insert_canonical_field_v1(
        &mut preconditions,
        "expected_procedure_digest",
        json!(&start.expected_procedure_digest),
    )?;

    canonicalize_json_v1(&json!({
        "protocol_major": 1,
        "command": request.command.command_name(),
        "workspace_id": resolved_workspace_id,
        "preconditions": preconditions,
        "payload": payload,
    }))
    .map_err(|error| SliceErrorV1::Canonicalization {
        message: error.to_string(),
    })
}

/// Convenience form for hash functions that accept a byte slice.
pub fn canonical_mutation_identity_bytes_v1(
    request: &SliceRequestV1,
    resolved_workspace_id: &WorkspaceId,
) -> Result<Vec<u8>, SliceErrorV1> {
    Ok(canonical_mutation_identity_v1(request, resolved_workspace_id)?.into_bytes())
}

/// Canonical reset-all identity. Its UTF-8 bytes are the SHA-256 input.
///
/// Reset-all replaces workspace UUID state, so this identity deliberately binds the stable Git
/// common-directory and worktree-administration fingerprints instead. It excludes all selector,
/// expected, previous, and target workspace UUIDs.
pub fn canonical_reset_all_identity_v1(
    request: &SliceRequestV1,
    common_dir_identity: &Sha256Digest,
    worktree_admin_identity: &Sha256Digest,
) -> Result<String, SliceErrorV1> {
    let command = request.command();
    if !command.is_mutation() {
        return Err(SliceErrorV1::NotAMutation {
            command: command.command_name(),
        });
    }

    let SliceCommandV1::WorkspaceResetAll(reset) = command else {
        return Err(SliceErrorV1::InvalidValue {
            field: "workspace.reset_all command",
        });
    };
    if !reset.confirmed {
        return Err(SliceErrorV1::InvalidValue { field: "confirmed" });
    }

    canonicalize_json_v1(&json!({
        "protocol_major": 1,
        "command": "workspace.reset_all",
        "common_dir_identity": common_dir_identity,
        "worktree_admin_identity": worktree_admin_identity,
        "payload": {
            "confirmed": true,
        },
    }))
    .map_err(|error| SliceErrorV1::Canonicalization {
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

fn session_revision_preconditions_json(
    preconditions: &SessionRevisionPreconditionsWireV1,
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

/// The immutable terminal outcome rendered by `job.list`, `job.status`, and `job.wait`.
///
/// This is deliberately distinct from Store's persisted terminal codec. It contains only public
/// result fields and the immutable session projection needed to replay a terminal job read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum TerminalJobResponseV1 {
    Success(TerminalJobSuccessProjectionV1),
    Error(TerminalJobErrorProjectionV1),
    Cancelled(TerminalJobCancellationProjectionV1),
}

/// Public immutable success facts from one terminal job receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalJobSuccessProjectionV1 {
    pub result: TerminalJobSuccessResultV1,
    pub session: Option<SessionOutputV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub procedure_digest: Option<Sha256Digest>,
}

/// Public result facts from a successful terminal job receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalJobSuccessResultV1 {
    WorkspaceInitialized {
        revision: Revision,
    },
    WorkspaceReset {
        revision: Revision,
    },
    SessionChanged {
        changed: bool,
        revision_before: Revision,
        revision_after: Revision,
    },
    ItemChanged {
        item_id: ItemId,
        changed: bool,
        revision_before: Revision,
        revision_after: Revision,
    },
}

/// Catalog-rendered public error facts from a failed terminal job receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalJobErrorProjectionV1 {
    pub code: ErrorCodeV1,
    pub message: String,
    pub retryable: bool,
    pub exit_code: ExitCodeV1,
    pub details: Map<String, Value>,
}

/// Explicit cancellation facts from a terminal cancelled job receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalJobCancellationProjectionV1 {
    pub cancelled: bool,
}
/// An item type rendered by the status and next result schemas.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemTypeResultV1 {
    Confirm,
    Text,
    Choice,
    Integer,
    List,
    Artifact,
}

/// A stage status rendered by the authoritative status result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatusResultV1 {
    Pending,
    Current,
    Blocked,
    Done,
    Skipped,
    Redo,
    Abandoned,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusTaskV1 {
    pub title: String,
    pub procedure: StatusProcedureV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusProcedureV1 {
    pub id: String,
    pub version: String,
    pub name: String,
    pub digest: Sha256Digest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusSessionV1 {
    pub id: SessionId,
    pub lifecycle: SessionLifecycleV1,
    #[serde(deserialize_with = "deserialize_nonzero_revision")]
    pub revision: Revision,
    pub created_at: Rfc3339MillisV1,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub completed_at: Option<Rfc3339MillisV1>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub cancelled_at: Option<Rfc3339MillisV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentAttemptResultV1 {
    pub stage_id: StageId,
    pub stage_index: u64,
    pub title: String,
    pub attempt_id: AttemptId,
    #[serde(deserialize_with = "deserialize_nonzero_u32")]
    pub attempt_number: u32,
    pub blocked: bool,
    pub ready_to_complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusStageResultV1 {
    pub id: StageId,
    pub index: u64,
    pub title: String,
    pub status: StageStatusResultV1,
    pub latest_attempt_number: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusItemResultV1 {
    pub id: ItemId,
    #[serde(rename = "type")]
    pub item_type: ItemTypeResultV1,
    pub prompt: String,
    pub required: bool,
    pub satisfied: bool,
    pub revision: Revision,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub value: Option<StatusItemValueV1>,
}

/// The closed value family rendered by one status item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StatusItemValueV1 {
    Confirm(bool),
    Text(String),
    Integer(i64),
    List(Vec<String>),
    Artifact(StatusArtifactValueV1),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusArtifactLocationTypeV1 {
    Path,
    Reference,
}

/// Closed artifact metadata rendered as an item value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusArtifactValueV1 {
    pub location_type: StatusArtifactLocationTypeV1,
    pub location: String,
    pub sha256_digest: Sha256Digest,
    pub size_bytes: u64,
    pub media_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockerResultV1 {
    pub id: BlockerId,
    pub attempt_id: AttemptId,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueResultV1 {
    pub pending_mutations: bool,
    pub queued_count: u64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub running_job_id: Option<JobId>,
    pub latest_workspace_sequence: u64,
}

/// Immutable attempt lifecycle rendered by verbose status history.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptLifecycleResultV1 {
    Active,
    Completed,
    Skipped,
    Abandoned,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviousAttemptResultV1 {
    pub stage_id: StageId,
    pub attempt_id: AttemptId,
    #[serde(deserialize_with = "deserialize_nonzero_u32")]
    pub attempt_number: u32,
    pub lifecycle: AttemptLifecycleResultV1,
    pub started_at: Rfc3339MillisV1,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub ended_at: Option<Rfc3339MillisV1>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub reason: Option<String>,
}

/// Typed projection of the closed `status` output `result`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusResultV1 {
    pub task: StatusTaskV1,
    pub session: StatusSessionV1,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub current: Option<CurrentAttemptResultV1>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub previous_attempts: Option<Vec<PreviousAttemptResultV1>>,
    pub stages: Vec<StatusStageResultV1>,
    pub items: Vec<StatusItemResultV1>,
    pub blockers: Vec<BlockerResultV1>,
    pub queue: QueueResultV1,
}

impl StatusResultV1 {
    pub fn from_result_map(result: &Map<String, Value>) -> Result<Self, serde_json::Error> {
        let mut result = result.clone();
        result.remove("schema");
        let status: Self = serde_json::from_value(Value::Object(result))?;
        if status.items.iter().all(status_item_value_matches_type)
            && status
                .blockers
                .iter()
                .all(|blocker| !blocker.reason.is_empty())
        {
            Ok(status)
        } else {
            Err(<serde_json::Error as serde::de::Error>::custom(
                "status item value does not match its item type",
            ))
        }
    }

    pub fn to_result_map(&self) -> Map<String, Value> {
        let mut result = result_map_from_serializable(self);
        result.insert(
            "schema".to_owned(),
            Value::String("podway.status-result/v1".to_owned()),
        );
        result
    }
}

fn status_item_value_matches_type(item: &StatusItemResultV1) -> bool {
    match (item.item_type, item.value.as_ref()) {
        (_, None)
        | (ItemTypeResultV1::Confirm, Some(StatusItemValueV1::Confirm(true)))
        | (ItemTypeResultV1::Text | ItemTypeResultV1::Choice, Some(StatusItemValueV1::Text(_)))
        | (ItemTypeResultV1::Integer, Some(StatusItemValueV1::Integer(_)))
        | (ItemTypeResultV1::List, Some(StatusItemValueV1::List(_))) => true,
        (ItemTypeResultV1::Artifact, Some(StatusItemValueV1::Artifact(artifact))) => {
            !artifact.location.is_empty() && !artifact.media_type.is_empty()
        }
        _ => false,
    }
}

impl TryFrom<Map<String, Value>> for StatusResultV1 {
    type Error = serde_json::Error;

    fn try_from(result: Map<String, Value>) -> Result<Self, Self::Error> {
        Self::from_result_map(&result)
    }
}

impl From<StatusResultV1> for Map<String, Value> {
    fn from(result: StatusResultV1) -> Self {
        result_map_from_serializable(&result)
    }
}

pub const COMPACT_STATUS_RESULT_SCHEMA_V1: &str = "podway.compact-status-result/v1";

/// Closed schema identifier for the bounded quiescent status projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CompactStatusResultSchemaV1 {
    #[serde(rename = "podway.compact-status-result/v1")]
    V1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactStatusProcedureV1 {
    pub id: String,
    pub version: String,
    pub digest: Sha256Digest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactStatusSessionV1 {
    pub id: SessionId,
    pub lifecycle: SessionLifecycleV1,
    #[serde(deserialize_with = "deserialize_nonzero_revision")]
    pub revision: Revision,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactStatusCurrentV1 {
    pub stage_id: StageId,
    pub attempt_id: AttemptId,
    #[serde(deserialize_with = "deserialize_nonzero_u32")]
    pub attempt_number: u32,
    pub ready_to_complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactStatusItemV1 {
    pub id: ItemId,
    #[serde(rename = "type")]
    pub item_type: ItemTypeResultV1,
    pub required: bool,
    pub satisfied: bool,
    pub revision: Revision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactBlockerStateV1 {
    Open,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactStatusBlockerV1 {
    pub id: BlockerId,
    pub attempt_id: AttemptId,
    pub state: CompactBlockerStateV1,
}

/// Closed, value-free status result emitted only after an idle queue barrier.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactStatusResultV1 {
    pub schema: CompactStatusResultSchemaV1,
    pub procedure: CompactStatusProcedureV1,
    pub session: CompactStatusSessionV1,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub current: Option<CompactStatusCurrentV1>,
    pub items: Vec<CompactStatusItemV1>,
    pub blockers: Vec<CompactStatusBlockerV1>,
    pub queue: QueueResultV1,
}

impl CompactStatusResultV1 {
    pub fn from_status(status: &StatusResultV1) -> Self {
        Self {
            schema: CompactStatusResultSchemaV1::V1,
            procedure: CompactStatusProcedureV1 {
                id: status.task.procedure.id.clone(),
                version: status.task.procedure.version.clone(),
                digest: status.task.procedure.digest.clone(),
            },
            session: CompactStatusSessionV1 {
                id: status.session.id.clone(),
                lifecycle: status.session.lifecycle,
                revision: status.session.revision,
            },
            current: status
                .current
                .as_ref()
                .map(|current| CompactStatusCurrentV1 {
                    stage_id: current.stage_id.clone(),
                    attempt_id: current.attempt_id.clone(),
                    attempt_number: current.attempt_number,
                    ready_to_complete: current.ready_to_complete,
                }),
            items: status
                .items
                .iter()
                .map(|item| CompactStatusItemV1 {
                    id: item.id.clone(),
                    item_type: item.item_type,
                    required: item.required,
                    satisfied: item.satisfied,
                    revision: item.revision,
                })
                .collect(),
            blockers: status
                .blockers
                .iter()
                .map(|blocker| CompactStatusBlockerV1 {
                    id: blocker.id.clone(),
                    attempt_id: blocker.attempt_id.clone(),
                    state: CompactBlockerStateV1::Open,
                })
                .collect(),
            queue: status.queue.clone(),
        }
    }

    pub fn from_result_map(result: &Map<String, Value>) -> Result<Self, serde_json::Error> {
        let compact: Self = serde_json::from_value(Value::Object(result.clone()))?;
        let lifecycle_is_consistent = match compact.session.lifecycle {
            SessionLifecycleV1::Running => compact.current.is_some(),
            SessionLifecycleV1::Completed | SessionLifecycleV1::Cancelled => {
                compact.current.is_none() && compact.items.is_empty() && compact.blockers.is_empty()
            }
        };
        let blockers_match_current_attempt = compact.current.as_ref().is_none_or(|current| {
            compact
                .blockers
                .iter()
                .all(|blocker| blocker.attempt_id == current.attempt_id)
        });
        if compact.procedure.id.is_empty()
            || compact.procedure.version.is_empty()
            || compact.items.len() > MAX_STAGE_ITEMS
            || compact.blockers.len() > MAX_OPEN_BLOCKERS_PER_ATTEMPT_V1
            || !lifecycle_is_consistent
            || !blockers_match_current_attempt
            || compact.queue.pending_mutations
            || compact.queue.queued_count != 0
            || compact.queue.running_job_id.is_some()
            || compact.queue.latest_workspace_sequence == 0
        {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "compact status requires an idle queue projection",
            ));
        }
        Ok(compact)
    }

    pub fn to_result_map(&self) -> Map<String, Value> {
        result_map_from_serializable(self)
    }
}

impl From<&StatusResultV1> for CompactStatusResultV1 {
    fn from(status: &StatusResultV1) -> Self {
        Self::from_status(status)
    }
}

impl From<StatusResultV1> for CompactStatusResultV1 {
    fn from(status: StatusResultV1) -> Self {
        Self::from_status(&status)
    }
}

impl TryFrom<Map<String, Value>> for CompactStatusResultV1 {
    type Error = serde_json::Error;

    fn try_from(result: Map<String, Value>) -> Result<Self, Self::Error> {
        Self::from_result_map(&result)
    }
}

impl From<CompactStatusResultV1> for Map<String, Value> {
    fn from(result: CompactStatusResultV1) -> Self {
        result_map_from_serializable(&result)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextStageResultV1 {
    pub id: StageId,
    pub title: String,
    pub attempt_id: AttemptId,
    #[serde(deserialize_with = "deserialize_nonzero_u32")]
    pub attempt_number: u32,
    pub instructions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextItemResultV1 {
    pub id: ItemId,
    #[serde(rename = "type")]
    pub item_type: ItemTypeResultV1,
    pub prompt: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllowedActionsResultV1 {
    pub complete: bool,
    pub skip: bool,
    pub retry: bool,
    pub return_to: Vec<StageId>,
    pub cancel: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextStageAfterCompletionResultV1 {
    pub id: StageId,
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSuggestionResultV1 {
    pub command: String,
    pub argv: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub item_id: Option<ItemId>,
}

/// Typed projection of the closed `next` output `result`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextResultV1 {
    #[serde(deserialize_with = "deserialize_required_option")]
    pub stage: Option<NextStageResultV1>,
    pub missing_required_items: Vec<NextItemResultV1>,
    pub blockers: Vec<BlockerResultV1>,
    pub allowed_actions: AllowedActionsResultV1,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub next_stage_after_completion: Option<NextStageAfterCompletionResultV1>,
    pub suggestions: Vec<CommandSuggestionResultV1>,
}

impl NextResultV1 {
    pub fn from_result_map(result: &Map<String, Value>) -> Result<Self, serde_json::Error> {
        let mut result = result.clone();
        result.remove("schema");
        let next: Self = serde_json::from_value(Value::Object(result))?;
        let stage_titles_are_valid = next
            .stage
            .as_ref()
            .is_none_or(|stage| !stage.title.is_empty())
            && next
                .next_stage_after_completion
                .as_ref()
                .is_none_or(|stage| !stage.title.is_empty());
        if stage_titles_are_valid
            && next
                .blockers
                .iter()
                .all(|blocker| !blocker.reason.is_empty())
        {
            Ok(next)
        } else {
            Err(<serde_json::Error as serde::de::Error>::custom(
                "next result violates non-empty string constraints",
            ))
        }
    }

    pub fn to_result_map(&self) -> Map<String, Value> {
        let mut result = result_map_from_serializable(self);
        result.insert(
            "schema".to_owned(),
            Value::String("podway.next-result/v1".to_owned()),
        );
        result
    }
}

impl TryFrom<Map<String, Value>> for NextResultV1 {
    type Error = serde_json::Error;

    fn try_from(result: Map<String, Value>) -> Result<Self, Self::Error> {
        Self::from_result_map(&result)
    }
}

impl From<NextResultV1> for Map<String, Value> {
    fn from(result: NextResultV1) -> Self {
        result_map_from_serializable(&result)
    }
}

fn result_map_from_serializable<T: Serialize>(value: &T) -> Map<String, Value> {
    match serde_json::to_value(value).expect("all serde_json values serialize") {
        Value::Object(result) => result,
        _ => unreachable!("result record serializes as a JSON object"),
    }
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
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
fn deserialize_nonzero_revision<'de, D>(deserializer: D) -> Result<Revision, D::Error>
where
    D: Deserializer<'de>,
{
    let revision = Revision::deserialize(deserializer)?;
    if revision == Revision::ZERO {
        return Err(serde::de::Error::custom("revision must be at least one"));
    }
    Ok(revision)
}

fn deserialize_nonzero_u32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u32::deserialize(deserializer)?;
    if value == 0 {
        return Err(serde::de::Error::custom("value must be at least one"));
    }
    Ok(value)
}
