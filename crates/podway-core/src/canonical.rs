use std::fmt;

use serde::Serialize;
use serde_json::Value;

/// Failures produced by the dependency-free canonical JSON primitive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanonicalJsonErrorV1 {
    Serialization(String),
    InvalidJson(String),
    NonCanonical,
    UnsupportedNumber,
}

impl fmt::Display for CanonicalJsonErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialization(error) => write!(formatter, "JSON serialization failed: {error}"),
            Self::InvalidJson(error) => write!(formatter, "invalid JSON: {error}"),
            Self::NonCanonical => formatter.write_str("JSON is not Podway Canonical JSON v1"),
            Self::UnsupportedNumber => formatter.write_str(
                "Podway Canonical JSON v1 supports only signed and unsigned integer numbers",
            ),
        }
    }
}

impl std::error::Error for CanonicalJsonErrorV1 {}

/// Serializes a value as deterministic JSON for config-owned document production and core's
/// persisted-record verifier.
///
/// Object keys are sorted by their UTF-8 byte sequence, array order is preserved, and output is
/// compact JSON. Only signed and unsigned integer JSON numbers are supported.
pub fn canonicalize_json_v1<T: Serialize>(value: &T) -> Result<String, CanonicalJsonErrorV1> {
    let value = serde_json::to_value(value)
        .map_err(|error| CanonicalJsonErrorV1::Serialization(error.to_string()))?;
    canonicalize_value(&value)
}

/// Verifies that bytes are valid, exact deterministic JSON.
///
/// The persisted-record verifier uses this dependency-free primitive to reject byte-equivalent
/// alternatives; configuration remains the owner of document production and policy.
pub fn verify_canonical_json_v1(input: &[u8]) -> Result<(), CanonicalJsonErrorV1> {
    let value = serde_json::from_slice(input)
        .map_err(|error| CanonicalJsonErrorV1::InvalidJson(error.to_string()))?;
    let canonical = canonicalize_value(&value)?;
    if canonical.as_bytes() == input {
        Ok(())
    } else {
        Err(CanonicalJsonErrorV1::NonCanonical)
    }
}

fn canonicalize_value(value: &Value) -> Result<String, CanonicalJsonErrorV1> {
    let mut output = Vec::new();
    write_canonical_json(value, &mut output)?;
    Ok(String::from_utf8(output).expect("serde_json only emits UTF-8 JSON"))
}

fn write_canonical_json(value: &Value, output: &mut Vec<u8>) -> Result<(), CanonicalJsonErrorV1> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(value) => {
            if *value {
                output.extend_from_slice(b"true");
            } else {
                output.extend_from_slice(b"false");
            }
        }
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                output.extend_from_slice(value.to_string().as_bytes());
            } else if let Some(value) = value.as_u64() {
                output.extend_from_slice(value.to_string().as_bytes());
            } else {
                return Err(CanonicalJsonErrorV1::UnsupportedNumber);
            }
        }
        Value::String(value) => serde_json::to_writer(&mut *output, value)
            .map_err(|error| CanonicalJsonErrorV1::Serialization(error.to_string()))?,
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.as_bytes().cmp(right.as_bytes()));

            output.push(b'{');
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)
                    .map_err(|error| CanonicalJsonErrorV1::Serialization(error.to_string()))?;
                output.push(b':');
                write_canonical_json(value, output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{CanonicalJsonErrorV1, canonicalize_json_v1, verify_canonical_json_v1};

    #[test]
    fn canonicalizes_object_keys_by_utf8_bytes() {
        let canonical = canonicalize_json_v1(&json!({"z": 2, "é": 3, "a": 1})).unwrap();

        assert_eq!(canonical, r#"{"a":1,"z":2,"é":3}"#);
        assert_eq!(verify_canonical_json_v1(canonical.as_bytes()), Ok(()));
    }

    #[test]
    fn preserves_array_order_and_uses_json_string_escaping() {
        let canonical = canonicalize_json_v1(&json!({
            "items": ["second", "first", "quote\" slash\\ newline\n tab\t backspace\u{8}"]
        }))
        .unwrap();

        assert_eq!(
            canonical,
            r#"{"items":["second","first","quote\" slash\\ newline\n tab\t backspace\b"]}"#
        );
    }

    #[test]
    fn supports_signed_and_unsigned_integer_edges() {
        let canonical = canonicalize_json_v1(&json!([i64::MIN, i64::MAX, u64::MAX])).unwrap();

        assert_eq!(
            canonical,
            format!("[{},{},{}]", i64::MIN, i64::MAX, u64::MAX)
        );
    }

    #[test]
    fn rejects_floating_point_numbers() {
        assert_eq!(
            canonicalize_json_v1(&1.5_f64),
            Err(CanonicalJsonErrorV1::UnsupportedNumber)
        );
    }

    #[test]
    fn classifies_invalid_noncanonical_and_unsupported_numbers_exactly() {
        assert!(matches!(
            verify_canonical_json_v1(b"{"),
            Err(CanonicalJsonErrorV1::InvalidJson(_))
        ));
        assert_eq!(
            verify_canonical_json_v1(br#"{"b":1,"a":2}"#),
            Err(CanonicalJsonErrorV1::NonCanonical)
        );
        assert_eq!(
            verify_canonical_json_v1(b"1.0"),
            Err(CanonicalJsonErrorV1::UnsupportedNumber)
        );
    }
}
