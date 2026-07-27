#![forbid(unsafe_code)]

//! Public IPC, JSON wire contracts, and runtime-neutral framing for Podway v1.

use std::fmt;

use podway_core::{AttemptId, JobId, Revision, SessionId, Sha256Digest, WorkspaceId};
use serde::{Deserialize, Deserializer, Serialize, de};
mod codec;
mod framing;
mod identity;
mod slice;

pub use codec::{
    PayloadCodecErrorV1, decode_request_payload_v1, decode_response_payload_v1,
    encode_request_payload_v1, encode_response_payload_v1,
};
pub use framing::{
    FrameErrorV1, FrameIoPhaseV1, decode_single_frame_v1, encode_frame_v1, read_single_frame_v1,
    write_frame_v1,
};
pub use identity::{BuildIdentityV1, build_identity_v1};
pub use slice::*;

use serde_json::{Map, Value};

pub const IPC_PROTOCOL_V1: &str = "podway.ipc/v1";
pub const OUTPUT_SCHEMA_V1: &str = "podway.output/v1";
pub const ERROR_SCHEMA_V1: &str = "podway.error/v1";
pub const SUPPORTED_PROTOCOLS_V1: &[&str] = &[IPC_PROTOCOL_V1];
pub const SUPPORTED_OUTPUT_SCHEMAS_V1: &[&str] = &[OUTPUT_SCHEMA_V1];
pub const SUPPORTED_ERROR_SCHEMAS_V1: &[&str] = &[ERROR_SCHEMA_V1];

pub const FRAME_LENGTH_PREFIX_BYTES_V1: usize = 4;
pub const MIN_FRAME_PAYLOAD_BYTES_V1: usize = 1;
pub const MAX_FRAME_PAYLOAD_BYTES_V1: usize = 1_048_576;
pub const MAX_COMMAND_BYTES_V1: usize = 128;
pub const MAX_IDEMPOTENCY_KEY_BYTES_V1: usize = 256;
pub const MAX_CLIENT_TEXT_SCALARS_V1: usize = 64;
pub const MAX_WORKSPACE_ROOT_SCALARS_V1: usize = 4096;
pub const MAX_WAIT_TIMEOUT_MILLIS_V1: u64 = 3_600_000;
pub const MAX_JSON_DEPTH_V1: usize = 64;

/// A typed protocol-contract validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    UnsupportedProtocol {
        received: String,
        supported: &'static [&'static str],
    },
    InvalidUuid {
        field: &'static str,
    },
    EmptyValue {
        field: &'static str,
    },
    ValueTooLong {
        field: &'static str,
        maximum: usize,
        actual: usize,
    },
    InvalidSha256Digest {
        field: &'static str,
    },
    InvalidTimestamp,
    TerminalJobMissingFinishedAt,
    NonterminalJobHasFinishedAt,
    InvalidErrorCode,
    ErrorCodeMetadataMismatch {
        code: String,
        expected_exit_code: u8,
        expected_retryable: bool,
        actual_exit_code: u8,
        actual_retryable: bool,
    },
    InvalidIdentityConflictDetails,
    InvalidProcedureDigestMismatchDetails,
    InvalidAdmissionMetadata,
    InvalidExitCode {
        value: u8,
    },
    InvalidJobSequence,
    MissingWorkspace,
    MissingIdempotencyKey,
    UnexpectedIdempotencyKey,
    WaitTimeoutTooLarge {
        value: u64,
        maximum: u64,
    },
    ZeroLengthFrame,
    FrameTooLarge {
        length: usize,
        maximum: usize,
    },
    JsonDepthExceeded {
        maximum: usize,
    },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedProtocol {
                received,
                supported,
            } => write!(
                formatter,
                "unsupported protocol {received:?}; supported protocols: {}",
                supported.join(", ")
            ),
            Self::InvalidUuid { field } => write!(formatter, "{field} must be a canonical UUID"),
            Self::EmptyValue { field } => write!(formatter, "{field} must not be empty"),
            Self::ValueTooLong {
                field,
                maximum,
                actual,
            } => write!(
                formatter,
                "{field} exceeds its maximum of {maximum} (received {actual})"
            ),
            Self::InvalidSha256Digest { field } => {
                write!(formatter, "{field} must be a canonical sha256 digest")
            }
            Self::InvalidTimestamp => write!(
                formatter,
                "timestamp must be RFC 3339 UTC with milliseconds"
            ),
            Self::TerminalJobMissingFinishedAt => {
                write!(formatter, "terminal job state requires finished_at")
            }
            Self::NonterminalJobHasFinishedAt => {
                write!(formatter, "queued or running job state forbids finished_at")
            }
            Self::InvalidErrorCode => {
                write!(formatter, "error code is not defined in the v1 catalog")
            }
            Self::ErrorCodeMetadataMismatch {
                code,
                expected_exit_code,
                expected_retryable,
                actual_exit_code,
                actual_retryable,
            } => write!(
                formatter,
                "error code {code} requires exit code {expected_exit_code} and retryable={expected_retryable}; received exit code {actual_exit_code} and retryable={actual_retryable}"
            ),
            Self::InvalidIdentityConflictDetails => {
                write!(
                    formatter,
                    "identity conflict details violate their closed v1 schema"
                )
            }
            Self::InvalidProcedureDigestMismatchDetails => formatter
                .write_str("Procedure digest mismatch details violate their closed v1 schema"),
            Self::InvalidAdmissionMetadata => {
                formatter.write_str("admission metadata violates its closed v1 schema")
            }
            Self::InvalidExitCode { value } => {
                write!(
                    formatter,
                    "exit code {value} is outside the supported range 1..=6"
                )
            }
            Self::InvalidJobSequence => write!(formatter, "job sequence must be at least one"),
            Self::MissingWorkspace => {
                write!(formatter, "this operation requires a workspace context")
            }
            Self::MissingIdempotencyKey => {
                write!(formatter, "this operation requires an idempotency key")
            }
            Self::UnexpectedIdempotencyKey => {
                write!(
                    formatter,
                    "query operations must not include an idempotency key"
                )
            }
            Self::WaitTimeoutTooLarge { value, maximum } => write!(
                formatter,
                "wait timeout {value} exceeds the maximum of {maximum} milliseconds"
            ),
            Self::ZeroLengthFrame => write!(formatter, "zero-length frames are invalid"),
            Self::FrameTooLarge { length, maximum } => {
                write!(
                    formatter,
                    "frame length {length} exceeds the maximum of {maximum}"
                )
            }
            Self::JsonDepthExceeded { maximum } => {
                write!(
                    formatter,
                    "JSON nesting exceeds the maximum depth of {maximum}"
                )
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

/// The one protocol version currently supported by this build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolVersionV1 {
    V1,
}

impl ProtocolVersionV1 {
    pub const fn identifier(self) -> &'static str {
        match self {
            Self::V1 => IPC_PROTOCOL_V1,
        }
    }
}

/// A pure protocol-negotiation result. It performs no I/O and does not inspect commands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolCompatibilityV1 {
    Compatible {
        version: ProtocolVersionV1,
    },
    Unsupported {
        received: String,
        supported: &'static [&'static str],
    },
}

pub fn negotiate_protocol(protocol: &str) -> ProtocolCompatibilityV1 {
    if protocol == IPC_PROTOCOL_V1 {
        ProtocolCompatibilityV1::Compatible {
            version: ProtocolVersionV1::V1,
        }
    } else {
        ProtocolCompatibilityV1::Unsupported {
            received: protocol.to_owned(),
            supported: SUPPORTED_PROTOCOLS_V1,
        }
    }
}

pub fn require_compatible_protocol(protocol: &str) -> Result<ProtocolVersionV1, ProtocolError> {
    match negotiate_protocol(protocol) {
        ProtocolCompatibilityV1::Compatible { version } => Ok(version),
        ProtocolCompatibilityV1::Unsupported {
            received,
            supported,
        } => Err(ProtocolError::UnsupportedProtocol {
            received,
            supported,
        }),
    }
}

/// Validates a declared frame payload length before a transport allocates a payload buffer.
pub fn validate_frame_payload_length(length: usize) -> Result<(), ProtocolError> {
    if length < MIN_FRAME_PAYLOAD_BYTES_V1 {
        return Err(ProtocolError::ZeroLengthFrame);
    }
    if length > MAX_FRAME_PAYLOAD_BYTES_V1 {
        return Err(ProtocolError::FrameTooLarge {
            length,
            maximum: MAX_FRAME_PAYLOAD_BYTES_V1,
        });
    }
    Ok(())
}

/// The IPC operation category that determines workspace and idempotency requirements.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationV1 {
    Query,
    Mutate,
    Control,
    Bootstrap,
}

/// A canonical UUID used for an IPC request identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RequestIdV1(String);

impl RequestIdV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        validate_uuid(&value, "request_id")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        validate_uuid(&self.0, "request_id")
    }
}

impl fmt::Display for RequestIdV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<String> for RequestIdV1 {
    type Error = ProtocolError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RequestIdV1> for String {
    fn from(value: RequestIdV1) -> Self {
        value.into_inner()
    }
}

impl<'de> Deserialize<'de> for RequestIdV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Client metadata supplied with every IPC request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientInfoV1 {
    name: String,
    version: String,
    pid: u32,
    product: String,
    contract_manifest_digest: String,
}

impl ClientInfoV1 {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        pid: u32,
    ) -> Result<Self, ProtocolError> {
        let identity = build_identity_v1();
        Self::new_with_contract_identity(
            name,
            version,
            pid,
            identity.product(),
            identity.contract_manifest_digest(),
        )
    }

    pub fn new_with_contract_identity(
        name: impl Into<String>,
        version: impl Into<String>,
        pid: u32,
        product: impl Into<String>,
        contract_manifest_digest: impl Into<String>,
    ) -> Result<Self, ProtocolError> {
        let client = Self {
            name: name.into(),
            version: version.into(),
            pid,
            product: product.into(),
            contract_manifest_digest: contract_manifest_digest.into(),
        };
        client.validate()?;
        Ok(client)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub const fn pid(&self) -> u32 {
        self.pid
    }

    pub fn product(&self) -> &str {
        &self.product
    }

    pub fn contract_manifest_digest(&self) -> &str {
        &self.contract_manifest_digest
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        validate_non_empty_scalar_bounded(&self.name, MAX_CLIENT_TEXT_SCALARS_V1, "client.name")?;
        validate_non_empty_scalar_bounded(
            &self.version,
            MAX_CLIENT_TEXT_SCALARS_V1,
            "client.version",
        )?;
        if self.pid == 0 {
            return Err(ProtocolError::EmptyValue {
                field: "client.pid",
            });
        }
        validate_non_empty_scalar_bounded(
            &self.product,
            MAX_CLIENT_TEXT_SCALARS_V1,
            "client.product",
        )?;
        validate_sha256_digest_v1(
            &self.contract_manifest_digest,
            "client.contract_manifest_digest",
        )?;
        Ok(())
    }
}

/// A bounded canonical command identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CommandNameV1(String);

impl CommandNameV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        validate_non_empty_byte_bounded(&value, MAX_COMMAND_BYTES_V1, "command")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        validate_non_empty_byte_bounded(&self.0, MAX_COMMAND_BYTES_V1, "command")
    }
}

impl TryFrom<String> for CommandNameV1 {
    type Error = ProtocolError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<CommandNameV1> for String {
    fn from(value: CommandNameV1) -> Self {
        value.into_inner()
    }
}

impl<'de> Deserialize<'de> for CommandNameV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// A root and optional known identity supplied by the CLI. The daemon must independently verify it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceContextV1 {
    root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_uuid: Option<WorkspaceId>,
}

impl WorkspaceContextV1 {
    pub fn new(
        root: impl Into<String>,
        expected_uuid: Option<WorkspaceId>,
    ) -> Result<Self, ProtocolError> {
        let context = Self {
            root: root.into(),
            expected_uuid,
        };
        context.validate()?;
        Ok(context)
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn expected_uuid(&self) -> Option<&WorkspaceId> {
        self.expected_uuid.as_ref()
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        validate_non_empty_scalar_bounded(
            &self.root,
            MAX_WORKSPACE_ROOT_SCALARS_V1,
            "workspace.root",
        )
    }
}

impl<'de> Deserialize<'de> for WorkspaceContextV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawWorkspaceContextV1 {
            root: String,
            #[serde(default)]
            expected_uuid: OptionalField<WorkspaceId>,
        }

        let raw = RawWorkspaceContextV1::deserialize(deserializer)?;
        let context = Self {
            root: raw.root,
            expected_uuid: raw.expected_uuid.0,
        };
        context.validate().map_err(de::Error::custom)?;
        Ok(context)
    }
}

/// A request idempotency key bounded to the v1 transport contract.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct IdempotencyKeyV1(String);

impl IdempotencyKeyV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        validate_non_empty_byte_bounded(&value, MAX_IDEMPOTENCY_KEY_BYTES_V1, "idempotency_key")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        validate_non_empty_byte_bounded(&self.0, MAX_IDEMPOTENCY_KEY_BYTES_V1, "idempotency_key")
    }
}

impl TryFrom<String> for IdempotencyKeyV1 {
    type Error = ProtocolError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<IdempotencyKeyV1> for String {
    fn from(value: IdempotencyKeyV1) -> Self {
        value.into_inner()
    }
}

impl<'de> Deserialize<'de> for IdempotencyKeyV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Optimistic-concurrency conditions carried by an IPC request.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreconditionsV1 {
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_revision: Option<Revision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt_id: Option<AttemptId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_revision: Option<Revision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_state: Option<JobStateV1>,
}

impl PreconditionsV1 {
    pub fn new(
        session_id: Option<SessionId>,
        session_revision: Option<Revision>,
        attempt_id: Option<AttemptId>,
        item_revision: Option<Revision>,
        blocker_id: Option<String>,
        job_state: Option<JobStateV1>,
    ) -> Result<Self, ProtocolError> {
        let preconditions = Self {
            session_id,
            session_revision,
            attempt_id,
            item_revision,
            blocker_id,
            job_state,
        };
        preconditions.validate()?;
        Ok(preconditions)
    }

    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    pub fn session_revision(&self) -> Option<Revision> {
        self.session_revision
    }

    pub fn attempt_id(&self) -> Option<&AttemptId> {
        self.attempt_id.as_ref()
    }

    pub fn item_revision(&self) -> Option<Revision> {
        self.item_revision
    }

    pub fn blocker_id(&self) -> Option<&str> {
        self.blocker_id.as_deref()
    }

    pub fn job_state(&self) -> Option<JobStateV1> {
        self.job_state
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        if let Some(blocker_id) = &self.blocker_id {
            validate_uuid(blocker_id, "preconditions.blocker_id")?;
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for PreconditionsV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawPreconditionsV1 {
            #[serde(default)]
            session_id: OptionalField<SessionId>,
            #[serde(default)]
            session_revision: OptionalField<Revision>,
            #[serde(default)]
            attempt_id: OptionalField<AttemptId>,
            #[serde(default)]
            item_revision: OptionalField<Revision>,
            #[serde(default)]
            blocker_id: OptionalField<String>,
            #[serde(default)]
            job_state: OptionalField<JobStateV1>,
        }

        let raw = RawPreconditionsV1::deserialize(deserializer)?;
        let preconditions = Self {
            session_id: raw.session_id.0,
            session_revision: raw.session_revision.0,
            attempt_id: raw.attempt_id.0,
            item_revision: raw.item_revision.0,
            blocker_id: raw.blocker_id.0,
            job_state: raw.job_state.0,
        };
        preconditions.validate().map_err(de::Error::custom)?;
        Ok(preconditions)
    }
}

/// Mutation-waiting preferences.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestOptionsV1 {
    detach: bool,
    wait_timeout_ms: u64,
}

impl RequestOptionsV1 {
    pub fn new(detach: bool, wait_timeout_ms: u64) -> Result<Self, ProtocolError> {
        let options = Self {
            detach,
            wait_timeout_ms,
        };
        options.validate()?;
        Ok(options)
    }

    pub const fn detach(&self) -> bool {
        self.detach
    }

    pub const fn wait_timeout_ms(&self) -> u64 {
        self.wait_timeout_ms
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        if self.wait_timeout_ms > MAX_WAIT_TIMEOUT_MILLIS_V1 {
            return Err(ProtocolError::WaitTimeoutTooLarge {
                value: self.wait_timeout_ms,
                maximum: MAX_WAIT_TIMEOUT_MILLIS_V1,
            });
        }
        Ok(())
    }
}

/// Validated fields used to construct one v1 request envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestEnvelopeInputV1 {
    pub request_id: RequestIdV1,
    pub client: ClientInfoV1,
    pub operation: OperationV1,
    pub command: CommandNameV1,
    pub workspace: Option<WorkspaceContextV1>,
    pub idempotency_key: Option<IdempotencyKeyV1>,
    pub preconditions: PreconditionsV1,
    pub options: RequestOptionsV1,
    pub payload: Map<String, Value>,
}

/// The bounded, validated request frame payload defined by `podway.ipc/v1`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RequestEnvelopeV1 {
    protocol: String,
    request_id: RequestIdV1,
    client: ClientInfoV1,
    operation: OperationV1,
    command: CommandNameV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace: Option<WorkspaceContextV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<IdempotencyKeyV1>,
    #[serde(skip_serializing_if = "PreconditionsV1::is_empty")]
    preconditions: PreconditionsV1,
    options: RequestOptionsV1,
    payload: Map<String, Value>,
}

impl RequestEnvelopeV1 {
    pub fn new(input: RequestEnvelopeInputV1) -> Result<Self, ProtocolError> {
        let RequestEnvelopeInputV1 {
            request_id,
            client,
            operation,
            command,
            workspace,
            idempotency_key,
            preconditions,
            options,
            payload,
        } = input;
        let request = Self {
            protocol: IPC_PROTOCOL_V1.to_owned(),
            request_id,
            client,
            operation,
            command,
            workspace,
            idempotency_key,
            preconditions,
            options,
            payload,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    pub fn request_id(&self) -> &RequestIdV1 {
        &self.request_id
    }

    pub fn client(&self) -> &ClientInfoV1 {
        &self.client
    }

    pub const fn operation(&self) -> OperationV1 {
        self.operation
    }

    pub fn command(&self) -> &CommandNameV1 {
        &self.command
    }

    pub fn workspace(&self) -> Option<&WorkspaceContextV1> {
        self.workspace.as_ref()
    }

    pub fn idempotency_key(&self) -> Option<&IdempotencyKeyV1> {
        self.idempotency_key.as_ref()
    }

    pub fn preconditions(&self) -> &PreconditionsV1 {
        &self.preconditions
    }

    pub const fn options(&self) -> RequestOptionsV1 {
        self.options
    }

    pub fn payload(&self) -> &Map<String, Value> {
        &self.payload
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        require_compatible_protocol(&self.protocol)?;
        self.request_id.validate()?;
        self.client.validate()?;
        self.command.validate()?;
        if let Some(workspace) = &self.workspace {
            workspace.validate()?;
        }
        if let Some(idempotency_key) = &self.idempotency_key {
            idempotency_key.validate()?;
        }
        self.preconditions.validate()?;
        self.options.validate()?;
        validate_json_map_depth(&self.payload, 1)?;

        match self.operation {
            OperationV1::Mutate | OperationV1::Bootstrap => {
                if self.workspace.is_none() {
                    return Err(ProtocolError::MissingWorkspace);
                }
                if self.idempotency_key.is_none() {
                    return Err(ProtocolError::MissingIdempotencyKey);
                }
            }
            OperationV1::Query if self.idempotency_key.is_some() => {
                return Err(ProtocolError::UnexpectedIdempotencyKey);
            }
            OperationV1::Query | OperationV1::Control => {}
        }

        Ok(())
    }
}

impl<'de> Deserialize<'de> for RequestEnvelopeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawRequestEnvelopeV1 {
            protocol: String,
            request_id: RequestIdV1,
            client: ClientInfoV1,
            operation: OperationV1,
            command: CommandNameV1,
            #[serde(default)]
            workspace: OptionalField<WorkspaceContextV1>,
            #[serde(default)]
            idempotency_key: OptionalField<IdempotencyKeyV1>,
            #[serde(default)]
            preconditions: OptionalField<PreconditionsV1>,
            options: RequestOptionsV1,
            payload: Map<String, Value>,
        }

        let raw = RawRequestEnvelopeV1::deserialize(deserializer)?;
        require_compatible_protocol(&raw.protocol).map_err(de::Error::custom)?;
        let request = Self {
            protocol: raw.protocol,
            request_id: raw.request_id,
            client: raw.client,
            operation: raw.operation,
            command: raw.command,
            workspace: raw.workspace.0,
            idempotency_key: raw.idempotency_key.0,
            preconditions: raw.preconditions.0.unwrap_or_default(),
            options: raw.options,
            payload: raw.payload,
        };
        request.validate().map_err(de::Error::custom)?;
        Ok(request)
    }
}

impl PreconditionsV1 {
    fn is_empty(&self) -> bool {
        self.session_id.is_none()
            && self.session_revision.is_none()
            && self.attempt_id.is_none()
            && self.item_revision.is_none()
            && self.blocker_id.is_none()
            && self.job_state.is_none()
    }
}

/// A v1 job state serialized in public output and request preconditions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStateV1 {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// A timestamp represented as RFC 3339 UTC with mandatory millisecond precision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Rfc3339MillisV1(String);

impl Rfc3339MillisV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        validate_rfc3339_millis(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        validate_rfc3339_millis(&self.0)
    }
}

impl<'de> Deserialize<'de> for Rfc3339MillisV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// A workspace summary embedded in public success output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceOutputV1 {
    uuid: WorkspaceId,
    root: String,
    latest_workspace_sequence: u64,
}

impl WorkspaceOutputV1 {
    pub fn new(
        uuid: WorkspaceId,
        root: impl Into<String>,
        latest_workspace_sequence: u64,
    ) -> Result<Self, ProtocolError> {
        let output = Self {
            uuid,
            root: root.into(),
            latest_workspace_sequence,
        };
        output.validate()?;
        Ok(output)
    }

    pub fn uuid(&self) -> &WorkspaceId {
        &self.uuid
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub const fn latest_workspace_sequence(&self) -> u64 {
        self.latest_workspace_sequence
    }
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_non_empty_scalar_bounded(
            &self.root,
            MAX_WORKSPACE_ROOT_SCALARS_V1,
            "workspace.root",
        )
    }
}
impl<'de> Deserialize<'de> for WorkspaceOutputV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawWorkspaceOutputV1 {
            uuid: WorkspaceId,
            root: String,
            latest_workspace_sequence: u64,
        }

        let raw = RawWorkspaceOutputV1::deserialize(deserializer)?;
        let output = Self {
            uuid: raw.uuid,
            root: raw.root,
            latest_workspace_sequence: raw.latest_workspace_sequence,
        };
        output.validate().map_err(de::Error::custom)?;
        Ok(output)
    }
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}
struct OptionalField<T>(Option<T>);

impl<T> Default for OptionalField<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<'de, T> Deserialize<'de> for OptionalField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self(Some(T::deserialize(deserializer)?)))
    }
}

/// A durable-job summary embedded in public success output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JobOutputV1 {
    id: JobId,
    sequence: u64,
    state: JobStateV1,
    submitted_at: Rfc3339MillisV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    claimed_at: Option<Rfc3339MillisV1>,
    finished_at: Option<Rfc3339MillisV1>,
}

impl JobOutputV1 {
    pub fn new(
        id: JobId,
        sequence: u64,
        state: JobStateV1,
        submitted_at: Rfc3339MillisV1,
        claimed_at: Option<Rfc3339MillisV1>,
        finished_at: Option<Rfc3339MillisV1>,
    ) -> Result<Self, ProtocolError> {
        let output = Self {
            id,
            sequence,
            state,
            submitted_at,
            claimed_at,
            finished_at,
        };
        output.validate()?;
        Ok(output)
    }

    pub fn id(&self) -> &JobId {
        &self.id
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub const fn state(&self) -> JobStateV1 {
        self.state
    }

    pub fn submitted_at(&self) -> &Rfc3339MillisV1 {
        &self.submitted_at
    }

    pub fn claimed_at(&self) -> Option<&Rfc3339MillisV1> {
        self.claimed_at.as_ref()
    }

    pub fn finished_at(&self) -> Option<&Rfc3339MillisV1> {
        self.finished_at.as_ref()
    }
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.sequence == 0 {
            return Err(ProtocolError::InvalidJobSequence);
        }
        self.submitted_at.validate()?;
        if let Some(claimed_at) = &self.claimed_at {
            claimed_at.validate()?;
        }
        if let Some(finished_at) = &self.finished_at {
            finished_at.validate()?;
        }
        match self.state {
            JobStateV1::Queued | JobStateV1::Running if self.finished_at.is_some() => {
                Err(ProtocolError::NonterminalJobHasFinishedAt)
            }
            JobStateV1::Succeeded | JobStateV1::Failed | JobStateV1::Cancelled
                if self.finished_at.is_none() =>
            {
                Err(ProtocolError::TerminalJobMissingFinishedAt)
            }
            _ => Ok(()),
        }
    }
}
impl<'de> Deserialize<'de> for JobOutputV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawJobOutputV1 {
            id: JobId,
            sequence: u64,
            state: JobStateV1,
            submitted_at: Rfc3339MillisV1,
            claimed_at: Option<Rfc3339MillisV1>,
            #[serde(deserialize_with = "deserialize_required_option")]
            finished_at: Option<Rfc3339MillisV1>,
        }

        let raw = RawJobOutputV1::deserialize(deserializer)?;
        let output = Self {
            id: raw.id,
            sequence: raw.sequence,
            state: raw.state,
            submitted_at: raw.submitted_at,
            claimed_at: raw.claimed_at,
            finished_at: raw.finished_at,
        };
        output.validate().map_err(de::Error::custom)?;
        Ok(output)
    }
}

/// A public session lifecycle serialization distinct from the internal domain state enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycleV1 {
    Running,
    Completed,
    Cancelled,
}

/// A session summary embedded in public success output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionOutputV1 {
    id: SessionId,
    title: String,
    lifecycle: SessionLifecycleV1,
    revision_before: Revision,
    revision_after: Revision,
}

impl SessionOutputV1 {
    pub fn new(
        id: SessionId,
        title: impl Into<String>,
        lifecycle: SessionLifecycleV1,
        revision_before: Revision,
        revision_after: Revision,
    ) -> Result<Self, ProtocolError> {
        let output = Self {
            id,
            title: title.into(),
            lifecycle,
            revision_before,
            revision_after,
        };
        output.validate()?;
        Ok(output)
    }

    pub fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub const fn lifecycle(&self) -> SessionLifecycleV1 {
        self.lifecycle
    }

    pub const fn revision_before(&self) -> Revision {
        self.revision_before
    }

    pub const fn revision_after(&self) -> Revision {
        self.revision_after
    }
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_non_empty_scalar_bounded(&self.title, 500, "session.title")
    }
}
impl<'de> Deserialize<'de> for SessionOutputV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawSessionOutputV1 {
            id: SessionId,
            title: String,
            lifecycle: SessionLifecycleV1,
            revision_before: Revision,
            revision_after: Revision,
        }

        let raw = RawSessionOutputV1::deserialize(deserializer)?;
        let output = Self {
            id: raw.id,
            title: raw.title,
            lifecycle: raw.lifecycle,
            revision_before: raw.revision_before,
            revision_after: raw.revision_after,
        };
        output.validate().map_err(de::Error::custom)?;
        Ok(output)
    }
}

/// Validated fields used to construct one v1 success envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputEnvelopeInputV1 {
    pub request_id: RequestIdV1,
    pub command: CommandNameV1,
    pub generated_at: Rfc3339MillisV1,
    pub workspace: Option<WorkspaceOutputV1>,
    pub job: Option<JobOutputV1>,
    pub session: Option<SessionOutputV1>,
    pub result: Map<String, Value>,
    pub warnings: Vec<Map<String, Value>>,
}

/// A validated `podway.output/v1` response envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OutputEnvelopeV1 {
    schema: String,
    request_id: RequestIdV1,
    command: CommandNameV1,
    generated_at: Rfc3339MillisV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace: Option<WorkspaceOutputV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job: Option<JobOutputV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<SessionOutputV1>,
    result: Map<String, Value>,
    warnings: Vec<Map<String, Value>>,
}

impl OutputEnvelopeV1 {
    pub fn new(input: OutputEnvelopeInputV1) -> Result<Self, ProtocolError> {
        let OutputEnvelopeInputV1 {
            request_id,
            command,
            generated_at,
            workspace,
            job,
            session,
            result,
            warnings,
        } = input;
        let output = Self {
            schema: OUTPUT_SCHEMA_V1.to_owned(),
            request_id,
            command,
            generated_at,
            workspace,
            job,
            session,
            result,
            warnings,
        };
        output.validate()?;
        Ok(output)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.schema != OUTPUT_SCHEMA_V1 {
            return Err(ProtocolError::UnsupportedProtocol {
                received: self.schema.clone(),
                supported: SUPPORTED_OUTPUT_SCHEMAS_V1,
            });
        }
        self.request_id.validate()?;
        self.command.validate()?;
        self.generated_at.validate()?;
        if let Some(workspace) = &self.workspace {
            workspace.validate()?;
        }
        if let Some(job) = &self.job {
            job.validate()?;
        }
        if let Some(session) = &self.session {
            session.validate()?;
        }
        validate_json_map_depth(&self.result, 1)?;
        if let Some(admission) = self.result.get("admission") {
            let (job_id, sequence) = validate_admission_metadata_v1(admission, false)?
                .ok_or(ProtocolError::InvalidAdmissionMetadata)?;
            let job = self
                .job
                .as_ref()
                .ok_or(ProtocolError::InvalidAdmissionMetadata)?;
            if job.id() != &job_id || job.sequence() != sequence {
                return Err(ProtocolError::InvalidAdmissionMetadata);
            }
        }
        for warning in &self.warnings {
            validate_json_map_depth(warning, 2)?;
        }
        Ok(())
    }

    pub fn request_id(&self) -> &RequestIdV1 {
        &self.request_id
    }

    pub fn command(&self) -> &CommandNameV1 {
        &self.command
    }

    pub fn generated_at(&self) -> &Rfc3339MillisV1 {
        &self.generated_at
    }

    pub fn workspace(&self) -> Option<&WorkspaceOutputV1> {
        self.workspace.as_ref()
    }

    pub fn job(&self) -> Option<&JobOutputV1> {
        self.job.as_ref()
    }

    pub fn session(&self) -> Option<&SessionOutputV1> {
        self.session.as_ref()
    }

    pub fn result(&self) -> &Map<String, Value> {
        &self.result
    }

    pub fn warnings(&self) -> &[Map<String, Value>] {
        &self.warnings
    }
}
impl<'de> Deserialize<'de> for OutputEnvelopeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawOutputEnvelopeV1 {
            schema: String,
            request_id: RequestIdV1,
            command: CommandNameV1,
            generated_at: Rfc3339MillisV1,
            #[serde(default)]
            workspace: OptionalField<WorkspaceOutputV1>,
            #[serde(default)]
            job: OptionalField<JobOutputV1>,
            #[serde(default)]
            session: OptionalField<SessionOutputV1>,
            result: Map<String, Value>,
            warnings: Vec<Map<String, Value>>,
        }

        let value = Value::deserialize(deserializer)?;
        validate_json_document_depth(&value).map_err(de::Error::custom)?;
        let raw = RawOutputEnvelopeV1::deserialize(value).map_err(de::Error::custom)?;
        let output = Self {
            schema: raw.schema,
            request_id: raw.request_id,
            command: raw.command,
            generated_at: raw.generated_at,
            workspace: raw.workspace.0,
            job: raw.job.0,
            session: raw.session.0,
            result: raw.result,
            warnings: raw.warnings,
        };
        output.validate().map_err(de::Error::custom)?;
        Ok(output)
    }
}

#[derive(Clone, Copy)]
struct ErrorCodeCatalogEntryV1 {
    code: &'static str,
    exit_code: u8,
    retryable: bool,
}

const ERROR_CODE_CATALOG_V1: &[ErrorCodeCatalogEntryV1] = &[
    ErrorCodeCatalogEntryV1 {
        code: "DAEMON_NOT_INSTALLED",
        exit_code: 3,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "DAEMON_UNAVAILABLE",
        exit_code: 3,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "DAEMON_SHUTTING_DOWN",
        exit_code: 3,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "DAEMON_VERSION_INCOMPATIBLE",
        exit_code: 3,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "DAEMON_CONTRACT_MISMATCH",
        exit_code: 3,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PROTOCOL_VERSION_UNSUPPORTED",
        exit_code: 3,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "REQUEST_TOO_LARGE",
        exit_code: 2,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "REQUEST_INVALID",
        exit_code: 2,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "NOT_A_GIT_WORKTREE",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "BARE_GIT_REPOSITORY",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKTREE_GONE",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_NOT_INITIALIZED",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_ALREADY_INITIALIZED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_INIT_CONFLICT",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_ID_CONFLICT",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_UUID_MISMATCH",
        exit_code: 4,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_CONFIG_INVALID",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_STATE_UNREADABLE",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_SCHEMA_UNSUPPORTED",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_QUEUE_FULL",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_MAINTENANCE",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_PATH_UNSAFE",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PATH_OUTSIDE_WORKTREE",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "MIGRATION_FAILED",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PROCEDURE_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PROCEDURE_INVALID",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PROCEDURE_SCHEMA_UNSUPPORTED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PROCEDURE_DIGEST_MISMATCH",
        exit_code: 4,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PRESET_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_ID_MISMATCH",
        exit_code: 4,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_ALREADY_EXISTS",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_NOT_RUNNING",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_NOT_COMPLETED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_CANCELLED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_REVISION_CONFLICT",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ATTEMPT_NOT_CURRENT",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "STAGE_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "STAGE_NOT_SKIPPABLE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "RETURN_NOT_ALLOWED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "REOPEN_NOT_ALLOWED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "REQUIRED_ITEMS_MISSING",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "BLOCKERS_PRESENT",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ITEM_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ITEM_TYPE_MISMATCH",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ITEM_CONSTRAINT_FAILED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ITEM_REVISION_CONFLICT",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ITEM_ALREADY_SET",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "LIST_VALUE_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "LIST_VALUE_DUPLICATE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ARTIFACT_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ARTIFACT_UNREADABLE",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ARTIFACT_CHANGED",
        exit_code: 1,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ARTIFACT_MEDIA_TYPE_NOT_ALLOWED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "BLOCKER_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "BLOCKER_NOT_CURRENT",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "IDEMPOTENCY_KEY_REUSED",
        exit_code: 2,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "JOB_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "JOB_NOT_CANCELLABLE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "JOB_WAIT_TIMEOUT",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "CONFIRMATION_REQUIRED",
        exit_code: 2,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "INTERNAL_ERROR",
        exit_code: 6,
        retryable: false,
    },
];
/// Returns the complete frozen v1 error catalog in wire order.
pub fn error_code_catalog_v1() -> impl ExactSizeIterator<Item = (&'static str, u8, bool)> {
    ERROR_CODE_CATALOG_V1
        .iter()
        .map(|entry| (entry.code, entry.exit_code, entry.retryable))
}

fn error_code_catalog_entry_v1(code: &str) -> Option<ErrorCodeCatalogEntryV1> {
    ERROR_CODE_CATALOG_V1
        .iter()
        .copied()
        .find(|entry| entry.code == code)
}
/// A stable public error code from the catalog.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ErrorCodeV1(String);

impl ErrorCodeV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        validate_error_code(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        validate_error_code(&self.0)
    }
}

impl<'de> Deserialize<'de> for ErrorCodeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// A v1 process exit status for public error responses.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ExitCodeV1(u8);

impl ExitCodeV1 {
    pub fn new(value: u8) -> Result<Self, ProtocolError> {
        if !(1..=6).contains(&value) {
            return Err(ProtocolError::InvalidExitCode { value });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u8 {
        self.0
    }
    fn validate(self) -> Result<(), ProtocolError> {
        Self::new(self.0).map(|_| ())
    }
}
impl<'de> Deserialize<'de> for ExitCodeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u8::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Validated fields used to construct one v1 error envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorEnvelopeInputV1 {
    pub request_id: RequestIdV1,
    pub command: CommandNameV1,
    pub generated_at: Rfc3339MillisV1,
    pub code: ErrorCodeV1,
    pub message: String,
    pub retryable: bool,
    pub exit_code: ExitCodeV1,
    pub workspace: Option<Map<String, Value>>,
    pub details: Map<String, Value>,
}

/// A validated `podway.error/v1` response envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ErrorEnvelopeV1 {
    schema: String,
    request_id: RequestIdV1,
    command: CommandNameV1,
    generated_at: Rfc3339MillisV1,
    code: ErrorCodeV1,
    message: String,
    retryable: bool,
    exit_code: ExitCodeV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace: Option<Map<String, Value>>,
    details: Map<String, Value>,
}

impl ErrorEnvelopeV1 {
    pub fn new(input: ErrorEnvelopeInputV1) -> Result<Self, ProtocolError> {
        let ErrorEnvelopeInputV1 {
            request_id,
            command,
            generated_at,
            code,
            message,
            retryable,
            exit_code,
            workspace,
            details,
        } = input;
        let output = Self {
            schema: ERROR_SCHEMA_V1.to_owned(),
            request_id,
            command,
            generated_at,
            code,
            message,
            retryable,
            exit_code,
            workspace,
            details,
        };
        output.validate()?;
        Ok(output)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.schema != ERROR_SCHEMA_V1 {
            return Err(ProtocolError::UnsupportedProtocol {
                received: self.schema.clone(),
                supported: SUPPORTED_ERROR_SCHEMAS_V1,
            });
        }
        self.request_id.validate()?;
        self.command.validate()?;
        self.generated_at.validate()?;
        self.code.validate()?;
        self.exit_code.validate()?;
        let catalog_entry = error_code_catalog_entry_v1(self.code.as_str())
            .ok_or(ProtocolError::InvalidErrorCode)?;
        if self.exit_code.get() != catalog_entry.exit_code
            || self.retryable != catalog_entry.retryable
        {
            return Err(ProtocolError::ErrorCodeMetadataMismatch {
                code: self.code.as_str().to_owned(),
                expected_exit_code: catalog_entry.exit_code,
                expected_retryable: catalog_entry.retryable,
                actual_exit_code: self.exit_code.get(),
                actual_retryable: self.retryable,
            });
        }
        validate_non_empty_scalar_bounded(&self.message, usize::MAX, "message")?;
        validate_json_map_depth(&self.details, 1)?;
        if let Some(admission) = self.details.get("admission") {
            validate_admission_metadata_v1(admission, true)?;
        }
        validate_identity_conflict_details_v1(self.code.as_str(), &self.details)?;
        validate_procedure_digest_mismatch_details_v1(self.code.as_str(), &self.details)?;
        if let Some(workspace) = &self.workspace {
            validate_json_map_depth(workspace, 1)?;
        }
        Ok(())
    }

    pub fn request_id(&self) -> &RequestIdV1 {
        &self.request_id
    }

    pub fn command(&self) -> &CommandNameV1 {
        &self.command
    }

    pub fn code(&self) -> &ErrorCodeV1 {
        &self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    pub const fn exit_code(&self) -> ExitCodeV1 {
        self.exit_code
    }

    pub fn details(&self) -> &Map<String, Value> {
        &self.details
    }
}

fn validate_admission_metadata_v1(
    admission: &Value,
    allow_not_admitted: bool,
) -> Result<Option<(JobId, u64)>, ProtocolError> {
    let admission = admission
        .as_object()
        .ok_or(ProtocolError::InvalidAdmissionMetadata)?;
    match admission.get("admitted").and_then(Value::as_bool) {
        Some(false) if allow_not_admitted && admission.len() == 1 => Ok(None),
        Some(true) if admission.len() == 3 => {
            let job_id = admission
                .get("job_id")
                .and_then(Value::as_str)
                .ok_or(ProtocolError::InvalidAdmissionMetadata)?;
            let job_id = JobId::new(job_id).map_err(|_| ProtocolError::InvalidAdmissionMetadata)?;
            let sequence = admission
                .get("workspace_sequence")
                .and_then(Value::as_u64)
                .filter(|sequence| *sequence > 0)
                .ok_or(ProtocolError::InvalidAdmissionMetadata)?;
            Ok(Some((job_id, sequence)))
        }
        _ => Err(ProtocolError::InvalidAdmissionMetadata),
    }
}

fn validate_identity_conflict_details_v1(
    code: &str,
    details: &Map<String, Value>,
) -> Result<(), ProtocolError> {
    let (schema, expected_key, actual_key, session) = match code {
        "WORKSPACE_UUID_MISMATCH" => (
            "podway.workspace-uuid-mismatch-details/v1",
            "expected_workspace_uuid",
            "actual_workspace_uuid",
            false,
        ),
        "SESSION_ID_MISMATCH" => (
            "podway.session-id-mismatch-details/v1",
            "expected_session_id",
            "actual_session_id",
            true,
        ),
        _ => return Ok(()),
    };
    if details.len() != 4
        || details.get("schema").and_then(Value::as_str) != Some(schema)
        || !details.contains_key(expected_key)
        || !details.contains_key(actual_key)
        || !details.contains_key("admission")
    {
        return Err(ProtocolError::InvalidIdentityConflictDetails);
    }
    let expected = details
        .get(expected_key)
        .and_then(Value::as_str)
        .ok_or(ProtocolError::InvalidIdentityConflictDetails)?;
    if session {
        SessionId::new(expected).map_err(|_| ProtocolError::InvalidIdentityConflictDetails)?;
        if let Some(actual) = details.get(actual_key).and_then(Value::as_str) {
            SessionId::new(actual).map_err(|_| ProtocolError::InvalidIdentityConflictDetails)?;
        } else if details.get(actual_key) != Some(&Value::Null) {
            return Err(ProtocolError::InvalidIdentityConflictDetails);
        }
    } else {
        WorkspaceId::new(expected).map_err(|_| ProtocolError::InvalidIdentityConflictDetails)?;
        let actual = details
            .get(actual_key)
            .and_then(Value::as_str)
            .ok_or(ProtocolError::InvalidIdentityConflictDetails)?;
        WorkspaceId::new(actual).map_err(|_| ProtocolError::InvalidIdentityConflictDetails)?;
    }
    let admission = details
        .get("admission")
        .and_then(Value::as_object)
        .ok_or(ProtocolError::InvalidIdentityConflictDetails)?;
    match admission.get("admitted").and_then(Value::as_bool) {
        Some(false) if admission.len() == 1 => Ok(()),
        Some(true) if admission.len() == 3 => {
            let job_id = admission
                .get("job_id")
                .and_then(Value::as_str)
                .ok_or(ProtocolError::InvalidIdentityConflictDetails)?;
            JobId::new(job_id).map_err(|_| ProtocolError::InvalidIdentityConflictDetails)?;
            let sequence = admission
                .get("workspace_sequence")
                .and_then(Value::as_u64)
                .ok_or(ProtocolError::InvalidIdentityConflictDetails)?;
            if sequence == 0 {
                return Err(ProtocolError::InvalidIdentityConflictDetails);
            }
            Ok(())
        }
        _ => Err(ProtocolError::InvalidIdentityConflictDetails),
    }
}

fn validate_procedure_digest_mismatch_details_v1(
    code: &str,
    details: &Map<String, Value>,
) -> Result<(), ProtocolError> {
    if code != "PROCEDURE_DIGEST_MISMATCH" {
        return Ok(());
    }
    if details.len() != 4
        || details.get("schema").and_then(Value::as_str)
            != Some("podway.procedure-digest-mismatch-details/v1")
        || details
            .get("admission")
            .and_then(Value::as_object)
            .is_none_or(|admission| {
                admission.len() != 1
                    || admission.get("admitted").and_then(Value::as_bool) != Some(false)
            })
    {
        return Err(ProtocolError::InvalidProcedureDigestMismatchDetails);
    }
    for field in ["expected_procedure_digest", "actual_procedure_digest"] {
        let value = details
            .get(field)
            .and_then(Value::as_str)
            .ok_or(ProtocolError::InvalidProcedureDigestMismatchDetails)?;
        Sha256Digest::new(value)
            .map_err(|_| ProtocolError::InvalidProcedureDigestMismatchDetails)?;
    }
    Ok(())
}
impl<'de> Deserialize<'de> for ErrorEnvelopeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawErrorEnvelopeV1 {
            schema: String,
            request_id: RequestIdV1,
            command: CommandNameV1,
            generated_at: Rfc3339MillisV1,
            code: ErrorCodeV1,
            message: String,
            retryable: bool,
            exit_code: ExitCodeV1,
            #[serde(default)]
            workspace: OptionalField<Map<String, Value>>,
            details: Map<String, Value>,
        }

        let value = Value::deserialize(deserializer)?;
        validate_json_document_depth(&value).map_err(de::Error::custom)?;
        let raw = RawErrorEnvelopeV1::deserialize(value).map_err(de::Error::custom)?;
        let output = Self {
            schema: raw.schema,
            request_id: raw.request_id,
            command: raw.command,
            generated_at: raw.generated_at,
            code: raw.code,
            message: raw.message,
            retryable: raw.retryable,
            exit_code: raw.exit_code,
            workspace: raw.workspace.0,
            details: raw.details,
        };
        output.validate().map_err(de::Error::custom)?;
        Ok(output)
    }
}

/// The single response envelope carried in one IPC response frame.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ResponseEnvelopeV1 {
    Output(OutputEnvelopeV1),
    Error(ErrorEnvelopeV1),
}

impl ResponseEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Output(output) => output.validate(),
            Self::Error(error) => error.validate(),
        }
    }
}
impl<'de> Deserialize<'de> for ResponseEnvelopeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        validate_json_document_depth(&value).map_err(de::Error::custom)?;

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawResponseEnvelopeV1 {
            Output(OutputEnvelopeV1),
            Error(ErrorEnvelopeV1),
        }

        let response = match RawResponseEnvelopeV1::deserialize(value).map_err(de::Error::custom)? {
            RawResponseEnvelopeV1::Output(output) => Self::Output(output),
            RawResponseEnvelopeV1::Error(error) => Self::Error(error),
        };
        Ok(response)
    }
}

fn validate_uuid(value: &str, field: &'static str) -> Result<(), ProtocolError> {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return Err(ProtocolError::InvalidUuid { field });
    }

    for (index, byte) in bytes.iter().copied().enumerate() {
        let valid = match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'),
        };
        if !valid {
            return Err(ProtocolError::InvalidUuid { field });
        }
    }
    Ok(())
}

fn validate_non_empty_byte_bounded(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), ProtocolError> {
    if value.is_empty() {
        return Err(ProtocolError::EmptyValue { field });
    }
    if value.len() > maximum {
        return Err(ProtocolError::ValueTooLong {
            field,
            maximum,
            actual: value.len(),
        });
    }
    Ok(())
}

fn validate_non_empty_scalar_bounded(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), ProtocolError> {
    let length = value.chars().count();
    if length == 0 {
        return Err(ProtocolError::EmptyValue { field });
    }
    if length > maximum {
        return Err(ProtocolError::ValueTooLong {
            field,
            maximum,
            actual: length,
        });
    }
    Ok(())
}

fn validate_sha256_digest_v1(value: &str, field: &'static str) -> Result<(), ProtocolError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(ProtocolError::InvalidSha256Digest { field });
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ProtocolError::InvalidSha256Digest { field });
    }
    Ok(())
}

fn validate_rfc3339_millis(value: &str) -> Result<(), ProtocolError> {
    let bytes = value.as_bytes();
    if bytes.len() != 24
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
        || bytes[23] != b'Z'
    {
        return Err(ProtocolError::InvalidTimestamp);
    }

    for index in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 21, 22] {
        if !bytes[index].is_ascii_digit() {
            return Err(ProtocolError::InvalidTimestamp);
        }
    }

    let year = decimal_component(bytes, 0, 4);
    let month = decimal_component(bytes, 5, 2);
    let day = decimal_component(bytes, 8, 2);
    let hour = decimal_component(bytes, 11, 2);
    let minute = decimal_component(bytes, 14, 2);
    let second = decimal_component(bytes, 17, 2);

    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if divides_evenly(year, 4)
            && (!divides_evenly(year, 100) || divides_evenly(year, 400)) =>
        {
            29
        }
        2 => 28,
        _ => return Err(ProtocolError::InvalidTimestamp),
    };
    if day == 0 || day > days_in_month || hour > 23 || minute > 59 || second > 59 {
        return Err(ProtocolError::InvalidTimestamp);
    }
    Ok(())
}

fn decimal_component(bytes: &[u8], start: usize, length: usize) -> u16 {
    bytes[start..start + length]
        .iter()
        .fold(0, |value, byte| value * 10 + u16::from(*byte - b'0'))
}

fn divides_evenly(value: u16, divisor: u16) -> bool {
    value.checked_rem(divisor) == Some(0)
}

fn validate_error_code(value: &str) -> Result<(), ProtocolError> {
    if error_code_catalog_entry_v1(value).is_none() {
        return Err(ProtocolError::InvalidErrorCode);
    }
    Ok(())
}

fn validate_json_map_depth(
    map: &Map<String, Value>,
    map_depth: usize,
) -> Result<(), ProtocolError> {
    for value in map.values() {
        validate_json_value_depth(value, map_depth + 1)?;
    }
    Ok(())
}
pub(crate) fn validate_json_document_depth(value: &Value) -> Result<(), ProtocolError> {
    validate_json_value_depth(value, 0)
}

fn validate_json_value_depth(value: &Value, depth: usize) -> Result<(), ProtocolError> {
    if depth > MAX_JSON_DEPTH_V1 {
        return Err(ProtocolError::JsonDepthExceeded {
            maximum: MAX_JSON_DEPTH_V1,
        });
    }

    match value {
        Value::Array(values) => {
            for value in values {
                validate_json_value_depth(value, depth + 1)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                validate_json_value_depth(value, depth + 1)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}
