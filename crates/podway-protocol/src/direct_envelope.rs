use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value};

use crate::{
    CommandNameV1, ErrorCodeV1, ExitCodeV1, ProtocolError, RequestIdV1, Rfc3339MillisV1,
    WorkspaceOutputV1, validate_daemon_free_document_v1,
};

pub const OUTPUT_SCHEMA_V4: &str = "podway.output/v4";
pub const ERROR_SCHEMA_V2: &str = "podway.error/v2";
pub const RECOVERY_RECIPE_SCHEMA_V2: &str = "podway.recovery-recipe/v2";

/// The direct family includes the prepared lifecycle omitted from the old public summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DirectSessionOutputV1 {
    pub id: podway_core::SessionId,
    pub title: String,
    #[serde(serialize_with = "serialize_direct_lifecycle")]
    pub lifecycle: podway_core::SessionLifecycle,
    pub revision_before: podway_core::Revision,
    pub revision_after: podway_core::Revision,
}

fn serialize_direct_lifecycle<S: serde::Serializer>(
    lifecycle: &podway_core::SessionLifecycle,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(match lifecycle {
        podway_core::SessionLifecycle::Prepared => "prepared",
        podway_core::SessionLifecycle::Running => "running",
        podway_core::SessionLifecycle::Completed => "completed",
        podway_core::SessionLifecycle::Cancelled => "cancelled",
    })
}

/// Runtime-neutral fields for a committed direct success or coherent read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OutputEnvelopeInputV4 {
    pub request_id: RequestIdV1,
    pub command: CommandNameV1,
    pub generated_at: Rfc3339MillisV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceOutputV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<DirectSessionOutputV1>,
    pub result: Map<String, Value>,
    pub warnings: Vec<Map<String, Value>>,
}

/// A closed, bounded, canonically validated direct success envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct OutputEnvelopeV4(Value);

impl OutputEnvelopeV4 {
    pub fn new(input: OutputEnvelopeInputV4) -> Result<Self, ProtocolError> {
        let command = input.command.as_str().to_owned();
        let mut value = serde_json::to_value(input)
            .map_err(|_| ProtocolError::InvalidCommandResult { command })?;
        value["schema"] = Value::String(OUTPUT_SCHEMA_V4.to_owned());
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, ProtocolError> {
        if !validate_daemon_free_document_v1(OUTPUT_SCHEMA_V4, &value) {
            return Err(ProtocolError::InvalidCommandResult {
                command: value["command"].as_str().unwrap_or_default().to_owned(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }
}

impl<'de> Deserialize<'de> for OutputEnvelopeV4 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::from_value(Value::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// Scalar fields for a direct error; validation uses the direct catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorEnvelopeInputV2 {
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

/// A direct error, without daemon admission, jobs or exit class three.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ErrorEnvelopeV2(Value);

impl ErrorEnvelopeV2 {
    pub fn new(input: ErrorEnvelopeInputV2) -> Result<Self, ProtocolError> {
        let mut value = serde_json::json!({
            "schema": ERROR_SCHEMA_V2,
            "request_id": input.request_id,
            "command": input.command,
            "generated_at": input.generated_at,
            "code": input.code,
            "message": input.message,
            "retryable": input.retryable,
            "exit_code": input.exit_code,
            "details": input.details,
        });
        if let Some(workspace) = input.workspace {
            value["workspace"] = Value::Object(workspace);
        }
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, ProtocolError> {
        if !validate_daemon_free_document_v1(ERROR_SCHEMA_V2, &value) {
            return Err(ProtocolError::InvalidErrorDetails {
                code: value["code"].as_str().unwrap_or_default().to_owned(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }
}

impl<'de> Deserialize<'de> for ErrorEnvelopeV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::from_value(Value::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// A validated direct recovery recipe, including explicit destructive authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RecoveryRecipeV2(Value);

impl RecoveryRecipeV2 {
    pub fn from_value(value: Value) -> Result<Self, ProtocolError> {
        if !validate_daemon_free_document_v1(RECOVERY_RECIPE_SCHEMA_V2, &value) {
            return Err(ProtocolError::InvalidErrorDetails {
                code: "REQUEST_INVALID".to_owned(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }
}

impl<'de> Deserialize<'de> for RecoveryRecipeV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::from_value(Value::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}
