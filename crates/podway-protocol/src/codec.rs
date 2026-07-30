use std::{fmt, str};

use serde_json::Value;

use crate::{
    COMPACT_STATUS_RESULT_SCHEMA_V1, ERROR_SCHEMA_V1, ErrorEnvelopeV1,
    MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1, OUTPUT_SCHEMA_V1, OutputEnvelopeV1, ProtocolError,
    RequestEnvelopeV1, ResponseEnvelopeV1, validate_frame_payload_length,
    validate_json_document_depth,
};

static SUPPORTED_RESPONSE_SCHEMAS_V1: &[&str] = &[OUTPUT_SCHEMA_V1, ERROR_SCHEMA_V1];

/// A payload serialization or decoding failure that preserves wire-contract distinctions.
#[derive(Debug)]
pub enum PayloadCodecErrorV1 {
    InvalidLength(ProtocolError),
    InvalidUtf8(str::Utf8Error),
    InvalidJson(serde_json::Error),
    JsonContract(ProtocolError),
    MissingOrInvalidDiscriminator {
        field: &'static str,
    },
    UnsupportedResponseSchema {
        received: String,
        supported: &'static [&'static str],
    },
    InvalidEnvelope(serde_json::Error),
    Serialize(serde_json::Error),
}

impl fmt::Display for PayloadCodecErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength(error) => error.fmt(formatter),
            Self::InvalidUtf8(error) => write!(formatter, "payload is not valid UTF-8: {error}"),
            Self::InvalidJson(error) => write!(formatter, "payload is not valid JSON: {error}"),
            Self::JsonContract(error) => error.fmt(formatter),
            Self::MissingOrInvalidDiscriminator { field } => {
                write!(
                    formatter,
                    "payload is missing a string {field:?} discriminator"
                )
            }
            Self::UnsupportedResponseSchema {
                received,
                supported,
            } => write!(
                formatter,
                "unsupported response schema {received:?}; supported schemas: {}",
                supported.join(", ")
            ),
            Self::InvalidEnvelope(error) => write!(formatter, "invalid envelope: {error}"),
            Self::Serialize(error) => write!(formatter, "failed to serialize envelope: {error}"),
        }
    }
}

impl std::error::Error for PayloadCodecErrorV1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidLength(error) | Self::JsonContract(error) => Some(error),
            Self::InvalidUtf8(error) => Some(error),
            Self::InvalidJson(error) | Self::InvalidEnvelope(error) | Self::Serialize(error) => {
                Some(error)
            }
            Self::MissingOrInvalidDiscriminator { .. } | Self::UnsupportedResponseSchema { .. } => {
                None
            }
        }
    }
}

/// Validates and compactly serializes one v1 request payload.
pub fn encode_request_payload_v1(
    request: &RequestEnvelopeV1,
) -> Result<Vec<u8>, PayloadCodecErrorV1> {
    request
        .validate()
        .map_err(PayloadCodecErrorV1::JsonContract)?;
    let payload = serde_json::to_vec(request).map_err(PayloadCodecErrorV1::Serialize)?;
    validate_frame_payload_length(payload.len()).map_err(PayloadCodecErrorV1::InvalidLength)?;
    Ok(payload)
}

/// Decodes one bounded UTF-8 JSON request payload and negotiates its protocol before envelope validation.
pub fn decode_request_payload_v1(payload: &[u8]) -> Result<RequestEnvelopeV1, PayloadCodecErrorV1> {
    validate_frame_payload_length(payload.len()).map_err(PayloadCodecErrorV1::InvalidLength)?;
    let document = str::from_utf8(payload).map_err(PayloadCodecErrorV1::InvalidUtf8)?;
    let value =
        serde_json::from_str::<Value>(document).map_err(PayloadCodecErrorV1::InvalidJson)?;
    validate_json_document_depth(&value).map_err(PayloadCodecErrorV1::JsonContract)?;

    let protocol = top_level_discriminator(&value, "protocol")?;
    crate::require_compatible_protocol(protocol).map_err(PayloadCodecErrorV1::JsonContract)?;
    serde_json::from_value(value).map_err(PayloadCodecErrorV1::InvalidEnvelope)
}

/// Validates and compactly serializes one v1 response payload.
pub fn encode_response_payload_v1(
    response: &ResponseEnvelopeV1,
) -> Result<Vec<u8>, PayloadCodecErrorV1> {
    response
        .validate()
        .map_err(PayloadCodecErrorV1::JsonContract)?;
    let payload = serde_json::to_vec(response).map_err(PayloadCodecErrorV1::Serialize)?;
    validate_frame_payload_length(payload.len()).map_err(PayloadCodecErrorV1::InvalidLength)?;
    if matches!(response, ResponseEnvelopeV1::Output(output) if is_compact_status_result(output.result()))
    {
        validate_compact_status_payload_length(payload.len())
            .map_err(PayloadCodecErrorV1::JsonContract)?;
    }
    Ok(payload)
}

/// Decodes one bounded UTF-8 JSON response payload using explicit schema dispatch.
pub fn decode_response_payload_v1(
    payload: &[u8],
) -> Result<ResponseEnvelopeV1, PayloadCodecErrorV1> {
    validate_frame_payload_length(payload.len()).map_err(PayloadCodecErrorV1::InvalidLength)?;
    let document = str::from_utf8(payload).map_err(PayloadCodecErrorV1::InvalidUtf8)?;
    let value =
        serde_json::from_str::<Value>(document).map_err(PayloadCodecErrorV1::InvalidJson)?;
    validate_json_document_depth(&value).map_err(PayloadCodecErrorV1::JsonContract)?;

    let schema = top_level_discriminator(&value, "schema")?.to_owned();
    match schema.as_str() {
        OUTPUT_SCHEMA_V1 => {
            if value
                .get("result")
                .and_then(Value::as_object)
                .is_some_and(is_compact_status_result)
            {
                validate_compact_status_payload_length(payload.len())
                    .map_err(PayloadCodecErrorV1::JsonContract)?;
            }
            serde_json::from_value::<OutputEnvelopeV1>(value)
                .map(ResponseEnvelopeV1::Output)
                .map_err(PayloadCodecErrorV1::InvalidEnvelope)
        }
        ERROR_SCHEMA_V1 => serde_json::from_value::<ErrorEnvelopeV1>(value)
            .map(ResponseEnvelopeV1::Error)
            .map_err(PayloadCodecErrorV1::InvalidEnvelope),
        _ => Err(PayloadCodecErrorV1::UnsupportedResponseSchema {
            received: schema,
            supported: SUPPORTED_RESPONSE_SCHEMAS_V1,
        }),
    }
}

fn is_compact_status_result(result: &serde_json::Map<String, Value>) -> bool {
    result.get("schema").and_then(Value::as_str) == Some(COMPACT_STATUS_RESULT_SCHEMA_V1)
}

fn validate_compact_status_payload_length(length: usize) -> Result<(), ProtocolError> {
    let length = length
        .checked_add(1)
        .ok_or(ProtocolError::CompactStatusEnvelopeTooLarge {
            length: usize::MAX,
            maximum: MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1,
        })?;
    if length > MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1 {
        return Err(ProtocolError::CompactStatusEnvelopeTooLarge {
            length,
            maximum: MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1,
        });
    }
    Ok(())
}

fn top_level_discriminator<'a>(
    value: &'a Value,
    field: &'static str,
) -> Result<&'a str, PayloadCodecErrorV1> {
    value
        .as_object()
        .and_then(|object| object.get(field))
        .and_then(Value::as_str)
        .ok_or(PayloadCodecErrorV1::MissingOrInvalidDiscriminator { field })
}
