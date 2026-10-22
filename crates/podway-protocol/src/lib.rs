#![forbid(unsafe_code)]
//! Public JSON contracts and in-process command values for Podway.
use podway_core::WorkspaceId;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value};
use std::fmt;
mod direct_envelope;
mod evidence_page;
mod identity;
mod json_len;
#[cfg(feature = "release-contract-verifier")]
mod release_contract;
mod result_contract;
mod slice;
pub use direct_envelope::{
    DirectSessionOutputV1, ERROR_SCHEMA_V2, ErrorEnvelopeInputV2, ErrorEnvelopeV2,
    OUTPUT_SCHEMA_V4, OutputEnvelopeInputV4, OutputEnvelopeV4, RECOVERY_RECIPE_SCHEMA_V2,
    RecoveryRecipeV2,
};
pub use evidence_page::{
    EVIDENCE_PAGE_TOKEN_VERSION_V1, EvidencePageTokenV1, MAX_EVIDENCE_PAGE_TOKEN_CHARS_V1,
};
pub use identity::{BuildIdentityV1, build_identity_v1};
pub use json_len::serialized_json_len_v1;
#[cfg(feature = "release-contract-verifier")]
pub use release_contract::{
    ReleaseContractErrorV1, ReleaseContractVerifierConfigV1, VerificationReceiptV1,
    verify_release_contract_v1,
};
pub use result_contract::{
    ContractGenerationV1, DAEMON_FREE_DOCUMENT_SCHEMAS_V1, EXISTING_ROUTE_RESULT_SCHEMAS_V2,
    MAX_DIRECT_INPUT_BYTES_V1, MAX_DIRECT_OUTPUT_BYTES_V1, MAX_V2_OUTPUT_WARNINGS,
    MAX_V2_WARNING_CODE_CHARS, MAX_V2_WARNING_MESSAGE_CHARS, MAX_V2_WARNING_PATH_CHARS,
    NEW_ROUTE_RESULT_SCHEMAS_V1, PROCEDURE_DIAGNOSTICS_RESULT_SCHEMA_V1,
    RELEASED_CONTRACT_MANIFESTS_V1, ResultSchemaContractV2, decode_result_schema_contract_v2,
    ensure_procedure_independent_result_schema_v1, released_writer_release_v1,
    result_schema_top_level_fields_v2, validate_command_result_v2,
    validate_daemon_free_document_v1, validate_delegation_document_v1,
    validate_procedure_independent_result_v1, validate_v2_output_warnings,
};
pub use slice::*;
pub const MAX_COMMAND_BYTES_V1: usize = 128;
pub const MAX_IDEMPOTENCY_KEY_BYTES_V1: usize = 256;
pub const MAX_WORKSPACE_ROOT_SCALARS_V1: usize = 4096;
pub const MAX_JSON_DEPTH_V1: usize = 64;
pub const SESSION_ARCHIVE_LIST_WINDOW_V2: usize = 256;
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
    InvalidTimestamp,
    InvalidErrorCode,
    InvalidEvidencePageToken,
    InvalidErrorDetails {
        code: String,
    },
    InvalidCommandResult {
        command: String,
    },
    InvalidOutputWarnings,
    InvalidExitCode {
        value: u8,
    },
    JsonDepthExceeded {
        maximum: usize,
    },
}
impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedProtocol {
                received,
                supported,
            } => write!(
                f,
                "unsupported contract {received:?}; supported contracts: {}",
                supported.join(", ")
            ),
            Self::InvalidUuid { field } => write!(f, "{field} must be a canonical UUID"),
            Self::EmptyValue { field } => write!(f, "{field} must not be empty"),
            Self::ValueTooLong {
                field,
                maximum,
                actual,
            } => write!(
                f,
                "{field} exceeds its maximum of {maximum} (received {actual})"
            ),
            Self::InvalidTimestamp => {
                f.write_str("timestamp must be RFC 3339 UTC with milliseconds")
            }
            Self::InvalidErrorCode => {
                f.write_str("error code is not defined in the direct catalog")
            }
            Self::InvalidEvidencePageToken => {
                f.write_str("the evidence page token is not exactly one canonical bounded payload")
            }
            Self::InvalidErrorDetails { code } => {
                write!(f, "error {code} details violate their closed schema")
            }
            Self::InvalidCommandResult { command } => {
                write!(f, "{command} result violates its closed schema")
            }
            Self::InvalidOutputWarnings => {
                f.write_str("output warnings violate their closed bounds")
            }
            Self::InvalidExitCode { value } => write!(f, "unsupported exit code {value}"),
            Self::JsonDepthExceeded { maximum } => {
                write!(f, "JSON exceeds maximum depth {maximum}")
            }
        }
    }
}
impl std::error::Error for ProtocolError {}
/// A canonical UUID used for an public request identifier.
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

/// A request idempotency key bounded to the direct input contract.
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

    /// Constructs the canonical UTC timestamp for milliseconds since the Unix epoch.
    pub fn from_unix_millis(value: u64) -> Result<Self, ProtocolError> {
        let seconds = value / 1_000;
        let millis = value % 1_000;
        let days = seconds / 86_400;
        let seconds_of_day = seconds % 86_400;
        let z = i128::from(days) + 719_468;
        let era = z / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let mut year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = mp + if mp < 10 { 3 } else { -9 };
        if month <= 2 {
            year += 1;
        }
        let hour = seconds_of_day / 3_600;
        let minute = (seconds_of_day % 3_600) / 60;
        let second = seconds_of_day % 60;
        Self::new(format!(
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z"
        ))
    }

    /// Returns epoch milliseconds, or no value for a timestamp before the Unix epoch.
    pub fn to_unix_millis(&self) -> Option<u64> {
        let number = |start, end| self.0.get(start..end)?.parse::<i64>().ok();
        let mut year = number(0, 4)?;
        let month = number(5, 7)?;
        let day = number(8, 10)?;
        if month <= 2 {
            year -= 1;
        }
        let era = year.div_euclid(400);
        let yoe = year - era * 400;
        let mp = month + if month > 2 { -3 } else { 9 };
        let doy = (153 * mp + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        let seconds =
            days * 86_400 + number(11, 13)? * 3_600 + number(14, 16)? * 60 + number(17, 19)?;
        u64::try_from(seconds * 1_000 + number(20, 23)?).ok()
    }

    pub fn into_inner(self) -> String {
        self.0
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
#[derive(Clone, Copy)]
struct ErrorCodeCatalogEntryV1 {
    code: &'static str,
    exit_code: u8,
    retryable: bool,
}

const ERROR_CODE_CATALOG_V1: &[ErrorCodeCatalogEntryV1] = &[
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
        code: "ATTEMPT_CONTENT_LIMIT_EXCEEDED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ATTEMPT_NOT_CURRENT",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ATTEMPT_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "BARE_GIT_REPOSITORY",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "BLOCKERS_PRESENT",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "BLOCKER_LIMIT_REACHED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "BLOCKER_NOT_CURRENT",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "BLOCKER_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "COMMAND_RETIRED",
        exit_code: 2,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "CONFIRMATION_REQUIRED",
        exit_code: 2,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "CRITERION_CITATION_INVALID",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "CRITERION_MODE_MIXED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "CRITERION_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "CRITERION_RESULT_MISSING",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "DECISION_REASON_MISSING",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "DELEGATION_ALREADY_ACCEPTED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "DELEGATION_BINDING_MISMATCH",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "DELEGATION_NOT_APPLICABLE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "DELEGATION_RECEIPT_REQUIRED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "DELEGATION_RECEIPT_STALE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "DIGEST_CONFIRMATION_REQUIRED",
        exit_code: 2,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "EVIDENCE_NOT_AVAILABLE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "EVIDENCE_PAGE_TOKEN_EXHAUSTED",
        exit_code: 2,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "EVIDENCE_PAGE_TOKEN_STALE",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "EVIDENCE_REFERENCE_STALE",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "EVIDENCE_REFERENCE_UNRESOLVED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "FRESH_GOAL_ASSESSMENT_MISSING",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "GOAL_REVISION_STALE",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "GOAL_REVISION_TARGET_NOT_ALLOWED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "GOAL_REVISION_TARGET_NOT_REVISION_SAFE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "GOAL_TRACKING_NOT_ENABLED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "GRAPH_NODE_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "GRAPH_NODE_TYPE_MISMATCH",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "IDEMPOTENCY_KEY_REUSED",
        exit_code: 2,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "INTERNAL_ERROR",
        exit_code: 6,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ITEM_ALREADY_SET",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ITEM_CONSTRAINT_FAILED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ITEM_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ITEM_REVISION_CONFLICT",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ITEM_TYPE_MISMATCH",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "LEGACY_RUNTIME_STATE_UNSUPPORTED",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "LIST_VALUE_DUPLICATE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "LIST_VALUE_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "MANUAL_REWORK_TARGET_NOT_ALLOWED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "MANUAL_REWORK_TARGET_NOT_ON_TRACE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "MIGRATION_FAILED",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "MUTATION_OUTCOME_UNKNOWN",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "NODE_DEFINITION_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "NOT_A_GIT_WORKTREE",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "OPTION_GUARD_UNSATISFIED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "OPTION_NOT_ALLOWED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PATH_OUTSIDE_WORKTREE",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PRESET_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PROCEDURE_DIGEST_MISMATCH",
        exit_code: 4,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PROCEDURE_INVALID",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PROCEDURE_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PROCEDURE_SCHEMA_UNSUPPORTED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "PROCEDURE_V2_SCHEMA_INVALID",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "REACTIVATION_FLAG_REQUIRED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "REQUEST_INVALID",
        exit_code: 2,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "REQUEST_TOO_LARGE",
        exit_code: 2,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "REQUIRED_ITEMS_MISSING",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "ROUTE_NOT_ALLOWED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_ALREADY_EXISTS",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_ARCHIVE_LIMIT_REACHED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_ARCHIVE_NOT_ELIGIBLE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_ARCHIVE_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_CANCELLED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_GOAL_ALREADY_DEFINED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_GOAL_MISSING",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_ID_MISMATCH",
        exit_code: 4,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_NOT_FOUND",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_NOT_RUNNING",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_NOT_TERMINAL",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_RESET_NOT_ELIGIBLE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_REVISION_CONFLICT",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_START_DECISION_REQUIRED",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "SESSION_START_STATE_CONFLICT",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "STAGE_NOT_SKIPPABLE",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_ALREADY_INITIALIZED",
        exit_code: 1,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_BUSY",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_CONFIG_INVALID",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_CONTRACT_MISMATCH",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_ID_CONFLICT",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_INIT_CONFLICT",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_MAINTENANCE",
        exit_code: 4,
        retryable: true,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_NOT_INITIALIZED",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_PATH_UNSAFE",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_SCHEMA_UNSUPPORTED",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_STATE_UNREADABLE",
        exit_code: 5,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKSPACE_UUID_MISMATCH",
        exit_code: 4,
        retryable: false,
    },
    ErrorCodeCatalogEntryV1 {
        code: "WORKTREE_GONE",
        exit_code: 5,
        retryable: false,
    },
];
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
        if !matches!(value, 1 | 2 | 4 | 5 | 6) {
            return Err(ProtocolError::InvalidExitCode { value });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u8 {
        self.0
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
