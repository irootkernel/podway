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
    verify_canonical_json_value_v1(input, &value)
}

/// Verifies exact canonical bytes against an already parsed JSON value.
///
/// The value alone cannot establish canonical input: its canonical encoding must match the
/// original bytes, including key order, escaping and number representation.
pub fn verify_canonical_json_value_v1(
    input: &[u8],
    value: &Value,
) -> Result<(), CanonicalJsonErrorV1> {
    let canonical = canonicalize_value(value)?;
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

    use super::{
        CanonicalJsonErrorV1, canonicalize_json_v1, verify_canonical_json_v1,
        verify_canonical_json_value_v1,
    };

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
        let i64_max_plus_one = (i64::MAX as u64) + 1;
        let canonical = canonicalize_json_v1(&json!({
            "booleans": [false, true],
            "edges": [i64::MIN, 0, i64::MAX, i64_max_plus_one, u64::MAX],
            "nested": {
                "i64_min": i64::MIN,
                "i64_max": i64::MAX,
                "i64_max_plus_one": i64_max_plus_one,
                "u64_max": u64::MAX,
                "zero": 0,
            }
        }))
        .unwrap();

        assert_eq!(
            canonical,
            format!(
                r#"{{"booleans":[false,true],"edges":[{},0,{},{},{}],"nested":{{"i64_max":{},"i64_max_plus_one":{},"i64_min":{},"u64_max":{},"zero":0}}}}"#,
                i64::MIN,
                i64::MAX,
                i64_max_plus_one,
                u64::MAX,
                i64::MAX,
                i64_max_plus_one,
                i64::MIN,
                u64::MAX
            )
        );
        assert_eq!(verify_canonical_json_v1(canonical.as_bytes()), Ok(()));
    }

    #[test]
    fn rejects_floating_point_numbers() {
        assert_eq!(
            canonicalize_json_v1(&1.5_f64),
            Err(CanonicalJsonErrorV1::UnsupportedNumber)
        );
        assert_eq!(
            canonicalize_json_v1(&json!({"nested": [1, 2.5]})),
            Err(CanonicalJsonErrorV1::UnsupportedNumber)
        );
    }

    #[test]
    fn classifies_invalid_noncanonical_and_unsupported_numbers_exactly() {
        for invalid in [b"{".as_slice(), b"+1".as_slice(), b"01".as_slice()] {
            assert!(
                matches!(
                    verify_canonical_json_v1(invalid),
                    Err(CanonicalJsonErrorV1::InvalidJson(_))
                ),
                "{:?}",
                std::str::from_utf8(invalid)
            );
        }
        assert_eq!(
            verify_canonical_json_v1(br#"{"b":1,"a":2}"#),
            Err(CanonicalJsonErrorV1::NonCanonical)
        );
        for unsupported in [
            b"1.0".as_slice(),
            b"1.25".as_slice(),
            b"1e0".as_slice(),
            b"1e2".as_slice(),
            b"1E-3".as_slice(),
            b"-0".as_slice(),
            b"-0.0".as_slice(),
            b"-9223372036854775809".as_slice(),
            b"18446744073709551616".as_slice(),
            br#"{"items":[-9223372036854775809]}"#.as_slice(),
            br#"{"nested":{"limit":18446744073709551616}}"#.as_slice(),
        ] {
            assert_eq!(
                verify_canonical_json_v1(unsupported),
                Err(CanonicalJsonErrorV1::UnsupportedNumber),
                "{:?}",
                std::str::from_utf8(unsupported)
            );
        }
    }

    #[test]
    fn value_verifier_compares_original_bytes_instead_of_normalized_values() {
        for input in [
            r#"{ "a":1}"#,
            r#"{"b":1,"a":2}"#,
            r#"{"a":1,"a":1}"#,
            r#"{"a":"\u0061"}"#,
        ] {
            let value = serde_json::from_str(input).unwrap();
            assert_eq!(
                verify_canonical_json_value_v1(input.as_bytes(), &value),
                Err(CanonicalJsonErrorV1::NonCanonical),
                "{input}"
            );
        }
        for input in [
            "1.0",
            "1e0",
            "1e2",
            "-0",
            "-9223372036854775809",
            "18446744073709551616",
            r#"{"items":[-9223372036854775809]}"#,
            r#"{"nested":{"limit":18446744073709551616}}"#,
        ] {
            let value = serde_json::from_str(input).unwrap();
            assert_eq!(
                verify_canonical_json_value_v1(input.as_bytes(), &value),
                Err(CanonicalJsonErrorV1::UnsupportedNumber),
                "{input}"
            );
        }
        let i64_max_plus_one = (i64::MAX as u64) + 1;
        let input = format!(
            r#"{{"a":[{},0,{},{},{}],"booleans":[false,true],"é":"\\\""}}"#,
            i64::MIN,
            i64::MAX,
            i64_max_plus_one,
            u64::MAX
        );
        let value = serde_json::from_str(&input).unwrap();
        assert_eq!(
            verify_canonical_json_value_v1(input.as_bytes(), &value),
            Ok(())
        );
        assert_eq!(
            verify_canonical_json_value_v1(br#"{"a":2}"#, &json!({"a":1})),
            Err(CanonicalJsonErrorV1::NonCanonical)
        );
    }
}
