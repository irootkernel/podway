//! Exact serialized JSON lengths without retaining an output buffer.

use serde::Serialize;
use std::io::{self, Write};

/// A byte-discarding sink that counts what serde_json writes.
struct CountingSink(usize);

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(buf.len())
            .ok_or_else(|| io::Error::other("serialized length overflow"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Measures the exact compact JSON serialization of `value` in bytes.
///
/// The value is written through serde_json into a discarding sink, so no output buffer is
/// retained and the count carries serde_json's own serialization error semantics: any
/// serializer failure, including a counting-sink length overflow reported as an `io::Error`
/// and converted by `serde_json::to_writer`, returns `Err`. This is a workspace-internal
/// helper for request-local byte budgets; Engine projection bounds use it and PARSE
/// response bounds are expected to reuse it.
pub fn serialized_json_len_v1<T: Serialize + ?Sized>(
    value: &T,
) -> Result<usize, serde_json::Error> {
    let mut sink = CountingSink(0);
    serde_json::to_writer(&mut sink, value)?;
    Ok(sink.0)
}

#[cfg(test)]
mod tests {
    use super::serialized_json_len_v1;
    use serde::{Serialize, ser::Error as _};
    use serde_json::{Value, json};

    #[test]
    fn views002_serialized_len_matches_to_vec_across_value_shapes() {
        let corpus: Vec<Value> = vec![
            Value::Null,
            json!(true),
            json!(0),
            json!(u64::MAX),
            json!(i64::MIN),
            json!(1.5),
            json!(""),
            json!("escape heavy \" \\ \u{2028} é \u{0007} \u{001f}"),
            json!("multibyte \u{1f600} ᄀ"),
            json!([]),
            json!({}),
            json!([1, null, "x\"", [2, {"deep": false}]]),
            json!({"nested": {"inner": ["a", 7, null]}, "flag": true}),
            json!({"trace_sequence": 10_000, "payload": "x"}),
        ];
        for value in &corpus {
            let measured = serialized_json_len_v1(value).unwrap();
            let encoded = serde_json::to_vec(value).unwrap();
            assert_eq!(measured, encoded.len(), "value {value}");
        }
    }

    #[test]
    fn views002_serialized_len_reports_serialization_failures() {
        struct Unserializable;
        impl Serialize for Unserializable {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(S::Error::custom("not serializable"))
            }
        }
        assert!(serialized_json_len_v1(&Unserializable).is_err());
        assert!(serde_json::to_vec(&Unserializable).is_err());
    }

    #[test]
    fn views002_serialized_len_agrees_at_history_window_scale() {
        // One history-window budget worth of payload keeps the counting path honest at the
        // scale Engine budgets with; the byte count must still equal the real encoding.
        let value = json!("x".repeat(65_536));
        let measured = serialized_json_len_v1(&value).unwrap();
        assert_eq!(measured, serde_json::to_vec(&value).unwrap().len());
        assert!(measured > 65_536);
    }
}
