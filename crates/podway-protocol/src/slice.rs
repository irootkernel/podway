use std::fmt;

use podway_core::{
    AttemptId, BlockerId, ItemId, JobId, Revision, SessionId, Sha256Digest, StageId, WorkspaceId,
    canonicalize_json_v1,
};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};

use crate::{OperationV1, PreconditionsV1, RequestEnvelopeV1, Rfc3339MillisV1, SessionLifecycleV1};

/// The only selector representation accepted by the G005 daemon boundary.
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

/// Validation failures for the G005 protocol slice.
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
                write!(formatter, "unsupported G005 command {received:?}")
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
                write!(formatter, "invalid G005 command payload: {message}")
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
    pub expected_attempt_id: AttemptId,
    pub expected_item_revision: Revision,
}

/// Optimistic-concurrency facts required for a session mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMutationPreconditionsWireV1 {
    pub expected_session_revision: Revision,
    pub expected_attempt_id: AttemptId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PresetStartV1 {
    pub preset: String,
    pub task_title: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemCheckV1 {
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
pub struct ItemAttachPathV1 {
    pub item_id: ItemId,
    pub path: String,
    pub media_type: Option<String>,
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
pub struct SessionRetryV1 {
    pub reason: String,
    pub preconditions: SessionMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionReturnV1 {
    pub destination_stage_id: StageId,
    pub reason: String,
    pub preconditions: SessionMutationPreconditionsWireV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCompleteV1 {
    pub preconditions: SessionMutationPreconditionsWireV1,
}

/// The complete, deliberately small G005 command set. No other envelope command is admitted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceCommandV1 {
    WorkspaceInit,
    PresetStart(PresetStartV1),
    Status,
    Next,
    ItemCheck(ItemCheckV1),
    ItemSet(ItemSetV1),
    ItemAdd(ItemAddV1),
    ItemAttachPath(ItemAttachPathV1),
    SessionBlock(SessionBlockV1),
    SessionUnblock(SessionUnblockV1),
    SessionRetry(SessionRetryV1),
    SessionReturn(SessionReturnV1),
    SessionComplete(SessionCompleteV1),
}

impl SliceCommandV1 {
    pub const fn command_name(&self) -> &'static str {
        match self {
            Self::WorkspaceInit => "workspace.init",
            Self::PresetStart(_) => "preset.start",
            Self::Status => "session.status",
            Self::Next => "session.next",
            Self::ItemCheck(_) => "item.check",
            Self::ItemSet(_) => "item.set",
            Self::ItemAdd(_) => "item.add",
            Self::ItemAttachPath(_) => "item.attach_path",
            Self::SessionBlock(_) => "session.block",
            Self::SessionUnblock(_) => "session.unblock",
            Self::SessionRetry(_) => "session.retry",
            Self::SessionReturn(_) => "session.return",
            Self::SessionComplete(_) => "session.complete",
        }
    }

    pub const fn is_mutation(&self) -> bool {
        !matches!(self, Self::Status | Self::Next)
    }
}

/// Parsed G005 data from a generic v1 request envelope. Envelope metadata remains transport-owned.
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
                (payload.selector, SliceCommandV1::WorkspaceInit)
            }
            "preset.start" => {
                require_envelope(envelope, "preset.start", OperationV1::Mutate, true)?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: PresetStartPayloadV1 = parse_payload(envelope)?;
                validate_preset(&payload.preset)?;
                validate_title(&payload.task_title)?;
                (
                    payload.selector,
                    SliceCommandV1::PresetStart(PresetStartV1 {
                        preset: payload.preset,
                        task_title: payload.task_title,
                    }),
                )
            }
            "session.status" => {
                require_envelope(envelope, "session.status", OperationV1::Query, false)?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: SelectorOnlyPayloadV1 = parse_payload(envelope)?;
                (payload.selector, SliceCommandV1::Status)
            }
            "session.next" => {
                require_envelope(envelope, "session.next", OperationV1::Query, false)?;
                require_no_preconditions(envelope.preconditions())?;
                let payload: SelectorOnlyPayloadV1 = parse_payload(envelope)?;
                (payload.selector, SliceCommandV1::Next)
            }
            "item.check" => {
                require_envelope(envelope, "item.check", OperationV1::Mutate, true)?;
                let preconditions = require_item_preconditions(envelope.preconditions())?;
                let payload: ItemCheckPayloadV1 = parse_payload(envelope)?;
                (
                    payload.selector,
                    SliceCommandV1::ItemCheck(ItemCheckV1 {
                        item_id: payload.item_id,
                        preconditions,
                    }),
                )
            }
            "item.set" => {
                require_envelope(envelope, "item.set", OperationV1::Mutate, true)?;
                let preconditions = require_item_preconditions(envelope.preconditions())?;
                let payload: ItemSetPayloadV1 = parse_payload(envelope)?;
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
                let payload: ItemAddPayloadV1 = parse_payload(envelope)?;
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
            "item.attach_path" => {
                require_envelope(envelope, "item.attach_path", OperationV1::Mutate, true)?;
                let preconditions = require_item_preconditions(envelope.preconditions())?;
                let payload: ItemAttachPathPayloadV1 = parse_payload(envelope)?;
                validate_safe_artifact_path(&payload.path)?;
                if let Some(media_type) = &payload.media_type {
                    validate_media_type(media_type)?;
                }
                (
                    payload.selector,
                    SliceCommandV1::ItemAttachPath(ItemAttachPathV1 {
                        item_id: payload.item_id,
                        path: payload.path,
                        media_type: payload.media_type,
                        preconditions,
                    }),
                )
            }
            "session.block" => {
                require_envelope(envelope, "session.block", OperationV1::Mutate, true)?;
                let preconditions = require_session_preconditions(envelope.preconditions())?;
                let payload: SessionBlockPayloadV1 = parse_payload(envelope)?;
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
            "session.retry" => {
                require_envelope(envelope, "session.retry", OperationV1::Mutate, true)?;
                let preconditions = require_session_preconditions(envelope.preconditions())?;
                let payload: SessionRetryPayloadV1 = parse_payload(envelope)?;
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
                require_envelope(envelope, "session.return", OperationV1::Mutate, true)?;
                let preconditions = require_session_preconditions(envelope.preconditions())?;
                let payload: SessionReturnPayloadV1 = parse_payload(envelope)?;
                validate_reason(&payload.reason)?;
                (
                    payload.selector,
                    SliceCommandV1::SessionReturn(SessionReturnV1 {
                        destination_stage_id: payload.destination_stage_id,
                        reason: payload.reason,
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
            _ => {
                return Err(SliceErrorV1::InvalidCommand {
                    received: command_name.to_owned(),
                });
            }
        };
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectorOnlyPayloadV1 {
    selector: WorktreeSelectorWireV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceInitPayloadV1 {
    selector: WorktreeSelectorWireV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetStartPayloadV1 {
    selector: WorktreeSelectorWireV1,
    preset: String,
    task_title: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemCheckPayloadV1 {
    selector: WorktreeSelectorWireV1,
    item_id: ItemId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemSetPayloadV1 {
    selector: WorktreeSelectorWireV1,
    item_id: ItemId,
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemAddPayloadV1 {
    selector: WorktreeSelectorWireV1,
    item_id: ItemId,
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemAttachPathPayloadV1 {
    selector: WorktreeSelectorWireV1,
    item_id: ItemId,
    path: String,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    media_type: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionBlockPayloadV1 {
    selector: WorktreeSelectorWireV1,
    reason: String,
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
struct SessionRetryPayloadV1 {
    selector: WorktreeSelectorWireV1,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionReturnPayloadV1 {
    selector: WorktreeSelectorWireV1,
    destination_stage_id: StageId,
    reason: String,
}

fn parse_payload<T: DeserializeOwned>(envelope: &RequestEnvelopeV1) -> Result<T, SliceErrorV1> {
    serde_json::from_value(Value::Object(envelope.payload().clone())).map_err(|error| {
        SliceErrorV1::InvalidPayload {
            message: error.to_string(),
        }
    })
}

fn require_envelope(
    envelope: &RequestEnvelopeV1,
    command: &'static str,
    operation: OperationV1,
    mutation: bool,
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
    match (mutation, envelope.idempotency_key().is_some()) {
        (true, false) => return Err(SliceErrorV1::MissingIdempotencyKey { command }),
        (false, true) => return Err(SliceErrorV1::UnexpectedIdempotencyKey { command }),
        _ => {}
    }
    Ok(())
}

fn require_no_preconditions(preconditions: &PreconditionsV1) -> Result<(), SliceErrorV1> {
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
        expected_attempt_id,
        expected_item_revision,
    })
}

fn require_session_preconditions(
    preconditions: &PreconditionsV1,
) -> Result<SessionMutationPreconditionsWireV1, SliceErrorV1> {
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
    if preconditions.session_id().is_some() {
        return Err(SliceErrorV1::UnexpectedPrecondition {
            field: "preconditions.session_id",
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
    Ok(SessionMutationPreconditionsWireV1 {
        expected_session_revision,
        expected_attempt_id,
    })
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

/// Canonical semantic mutation identity. Its UTF-8 bytes are the SHA-256 input.
///
/// Request metadata, transport wait preferences, and the selector's path/display/expected UUID are
/// deliberately excluded. The resolved daemon workspace identity is the only workspace identity.
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
        SliceCommandV1::WorkspaceInit => (json!({}), json!({})),
        SliceCommandV1::PresetStart(start) => (
            json!({}),
            json!({"preset": &start.preset, "task_title": &start.task_title}),
        ),
        SliceCommandV1::ItemCheck(item) => (
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
        SliceCommandV1::ItemAttachPath(item) => (
            item_preconditions_json(&item.preconditions),
            json!({
                "item_id": &item.item_id,
                "path": &item.path,
                "media_type": &item.media_type,
            }),
        ),
        SliceCommandV1::SessionBlock(session) => (
            session_preconditions_json(&session.preconditions),
            json!({"reason": &session.reason}),
        ),
        SliceCommandV1::SessionUnblock(session) => (
            session_preconditions_json(&session.preconditions),
            json!({"blocker_id": &session.blocker_id, "all": session.all}),
        ),
        SliceCommandV1::SessionRetry(session) => (
            session_preconditions_json(&session.preconditions),
            json!({"reason": &session.reason}),
        ),
        SliceCommandV1::SessionReturn(session) => (
            session_preconditions_json(&session.preconditions),
            json!({"destination_stage_id": &session.destination_stage_id, "reason": &session.reason}),
        ),
        SliceCommandV1::SessionComplete(session) => (
            session_preconditions_json(&session.preconditions),
            json!({}),
        ),
        SliceCommandV1::Status | SliceCommandV1::Next => {
            unreachable!("queries were rejected above")
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

/// Convenience form for hash functions that accept a byte slice.
pub fn canonical_mutation_identity_bytes_v1(
    request: &SliceRequestV1,
    resolved_workspace_id: &WorkspaceId,
) -> Result<Vec<u8>, SliceErrorV1> {
    Ok(canonical_mutation_identity_v1(request, resolved_workspace_id)?.into_bytes())
}

fn item_preconditions_json(preconditions: &ItemMutationPreconditionsWireV1) -> Value {
    json!({
        "attempt_id": &preconditions.expected_attempt_id,
        "item_revision": preconditions.expected_item_revision,
    })
}

fn session_preconditions_json(preconditions: &SessionMutationPreconditionsWireV1) -> Value {
    json!({
        "session_revision": preconditions.expected_session_revision,
        "attempt_id": &preconditions.expected_attempt_id,
    })
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
    pub value: Value,
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

/// Strict typed projection of `status` output `result`, matching `status-result-v1.schema.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusResultV1 {
    pub task: StatusTaskV1,
    pub session: StatusSessionV1,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub current: Option<CurrentAttemptResultV1>,
    pub stages: Vec<StatusStageResultV1>,
    pub items: Vec<StatusItemResultV1>,
    pub blockers: Vec<BlockerResultV1>,
    pub queue: QueueResultV1,
}

impl StatusResultV1 {
    pub fn from_result_map(result: &Map<String, Value>) -> Result<Self, serde_json::Error> {
        serde_json::from_value(Value::Object(result.clone()))
    }

    pub fn to_result_map(&self) -> Map<String, Value> {
        result_map_from_serializable(self)
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

/// Strict typed projection of `next` output `result`, matching `next-result-v1.schema.json`.
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
        serde_json::from_value(Value::Object(result.clone()))
    }

    pub fn to_result_map(&self) -> Map<String, Value> {
        result_map_from_serializable(self)
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
