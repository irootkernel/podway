use std::io::{self, Read, Write};

use podway_protocol::{
    ClientInfoV1, CommandNameV1, ErrorCodeV1, ErrorEnvelopeInputV1, ErrorEnvelopeV1, ExitCodeV1,
    FrameErrorV1, FrameIoPhaseV1, MAX_FRAME_PAYLOAD_BYTES_V1, OperationV1, OutputEnvelopeInputV3,
    OutputEnvelopeV3, PayloadCodecErrorV1, PreconditionsV1, ProtocolError, RequestEnvelopeInputV1,
    RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2, Rfc3339MillisV1,
    decode_request_payload_v1, decode_response_payload_v2, decode_single_frame_v1, encode_frame_v1,
    encode_request_payload_v1, encode_response_payload_v2, read_single_frame_v1, write_frame_v1,
};
use serde_json::{Map, Value, json};

const REQUEST_ID: &str = "123e4567-e89b-12d3-a456-426614174000";
const GENERATED_AT: &str = "2026-07-14T12:34:56.789Z";

struct CappedReader {
    bytes: Vec<u8>,
    position: usize,
    cap: usize,
    calls: usize,
    interrupted_calls: Vec<usize>,
    failure: Option<(usize, io::ErrorKind)>,
}

impl CappedReader {
    fn new(bytes: Vec<u8>, cap: usize) -> Self {
        Self {
            bytes,
            position: 0,
            cap,
            calls: 0,
            interrupted_calls: Vec::new(),
            failure: None,
        }
    }

    fn with_interrupts(mut self, calls: impl IntoIterator<Item = usize>) -> Self {
        self.interrupted_calls.extend(calls);
        self
    }

    fn with_failure(mut self, call: usize, kind: io::ErrorKind) -> Self {
        self.failure = Some((call, kind));
        self
    }
}

impl Read for CappedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.interrupted_calls.contains(&self.calls) {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        if matches!(self.failure, Some((call, _)) if call == self.calls) {
            return Err(io::Error::from(
                self.failure.expect("failure must be present").1,
            ));
        }
        if self.position == self.bytes.len() {
            return Ok(0);
        }

        let count = self
            .cap
            .min(buffer.len())
            .min(self.bytes.len() - self.position);
        buffer[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

struct CappedWriter {
    bytes: Vec<u8>,
    cap: usize,
    calls: usize,
    interrupted_calls: Vec<usize>,
    failure: Option<(usize, io::ErrorKind)>,
}

impl CappedWriter {
    fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            cap,
            calls: 0,
            interrupted_calls: Vec::new(),
            failure: None,
        }
    }

    fn with_interrupts(mut self, calls: impl IntoIterator<Item = usize>) -> Self {
        self.interrupted_calls.extend(calls);
        self
    }

    fn with_failure(mut self, call: usize, kind: io::ErrorKind) -> Self {
        self.failure = Some((call, kind));
        self
    }
}

impl Write for CappedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.interrupted_calls.contains(&self.calls) {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        if matches!(self.failure, Some((call, _)) if call == self.calls) {
            return Err(io::Error::from(
                self.failure.expect("failure must be present").1,
            ));
        }

        let count = self.cap.min(bytes.len());
        self.bytes.extend_from_slice(&bytes[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn valid_request() -> RequestEnvelopeV1 {
    valid_request_with_payload(Map::new())
}

fn valid_request_with_payload(payload: Map<String, Value>) -> RequestEnvelopeV1 {
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).expect("request fixture must be valid"),
        client: ClientInfoV1::new("podway-cli", "0.1.0", 1).expect("client fixture must be valid"),
        operation: OperationV1::Query,
        command: CommandNameV1::new("workspace.show").expect("command fixture must be valid"),
        workspace: None,
        idempotency_key: None,
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).expect("options fixture must be valid"),
        payload,
    })
    .expect("request fixture must be valid")
}

fn timestamp() -> Rfc3339MillisV1 {
    Rfc3339MillisV1::new(GENERATED_AT).expect("timestamp fixture must be valid")
}

fn valid_output() -> OutputEnvelopeV3 {
    OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
        request_id: RequestIdV1::new(REQUEST_ID).expect("request fixture must be valid"),
        command: CommandNameV1::new("workspace.show").expect("command fixture must be valid"),
        generated_at: timestamp(),
        workspace: None,
        job: None,
        session: None,
        result: Map::new(),
        warnings: Vec::new(),
    })
    .expect("output fixture must be valid")
}

fn valid_error() -> ErrorEnvelopeV1 {
    ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).expect("request fixture must be valid"),
        command: CommandNameV1::new("status").expect("command fixture must be valid"),
        generated_at: timestamp(),
        code: ErrorCodeV1::new("REQUEST_INVALID").expect("error code fixture must be valid"),
        message: "Request is invalid.".to_owned(),
        retryable: false,
        exit_code: ExitCodeV1::new(2).expect("exit code fixture must be valid"),
        workspace: None,
        details: Map::new(),
    })
    .expect("error fixture must be valid")
}

#[test]
fn frames_use_known_big_endian_prefixes_and_enforce_exact_size_bounds() {
    let single = encode_frame_v1(&[0xa5]).expect("one-byte payload must frame");
    assert_eq!(single, vec![0, 0, 0, 1, 0xa5]);

    let payload_258 = vec![0x5a; 258];
    let frame_258 = encode_frame_v1(&payload_258).expect("258-byte payload must frame");
    assert_eq!(&frame_258[..4], &[0, 0, 1, 2]);
    assert_eq!(
        decode_single_frame_v1(&frame_258).expect("frame must decode"),
        payload_258.as_slice()
    );

    let maximum = vec![0x7f; MAX_FRAME_PAYLOAD_BYTES_V1];
    let maximum_frame = encode_frame_v1(&maximum).expect("maximum payload must frame");
    assert_eq!(&maximum_frame[..4], &[0, 16, 0, 0]);
    assert_eq!(
        decode_single_frame_v1(&maximum_frame).expect("frame must decode"),
        maximum.as_slice()
    );

    assert_eq!(encode_frame_v1(&[]), Err(ProtocolError::ZeroLengthFrame));
    assert_eq!(
        encode_frame_v1(&vec![0; MAX_FRAME_PAYLOAD_BYTES_V1 + 1]),
        Err(ProtocolError::FrameTooLarge {
            length: MAX_FRAME_PAYLOAD_BYTES_V1 + 1,
            maximum: MAX_FRAME_PAYLOAD_BYTES_V1,
        })
    );
}

#[test]
fn frame_reading_handles_fragmentation_and_interrupted_operations() {
    let payload = b"fragmented";
    let frame = encode_frame_v1(payload).expect("payload must frame");

    for cap in 1..=frame.len() {
        let mut reader = CappedReader::new(frame.clone(), cap);
        assert_eq!(
            read_single_frame_v1(&mut reader).expect("fragmented frame must decode"),
            Some(payload.to_vec()),
            "cap {cap}"
        );
    }

    let mut reader = CappedReader::new(frame.clone(), 1).with_interrupts([1, 4, 9]);
    assert_eq!(
        read_single_frame_v1(&mut reader).expect("interrupted frame must decode"),
        Some(payload.to_vec())
    );

    let mut writer = CappedWriter::new(16).with_interrupts([1, 3]);
    write_frame_v1(&mut writer, payload).expect("interrupted writes must retry");
    assert_eq!(writer.bytes, frame);
}

#[test]
fn frame_reading_reports_clean_partial_and_trailing_end_of_stream() {
    let payload = b"abc";
    let frame = encode_frame_v1(payload).expect("payload must frame");

    let mut reader = CappedReader::new(Vec::new(), 4);
    assert_eq!(
        read_single_frame_v1(&mut reader).expect("clean EOF must decode"),
        None
    );

    for received in 0..4 {
        let error = decode_single_frame_v1(&frame[..received]).expect_err("prefix is incomplete");
        assert!(matches!(
            error,
            FrameErrorV1::UnexpectedEof {
                phase: FrameIoPhaseV1::LengthPrefix,
                expected: 4,
                received: actual,
            } if actual == received
        ));
    }
    for received in 1..4 {
        let mut reader = CappedReader::new(frame[..received].to_vec(), 4);
        let error = read_single_frame_v1(&mut reader).expect_err("prefix is incomplete");
        assert!(matches!(
            error,
            FrameErrorV1::UnexpectedEof {
                phase: FrameIoPhaseV1::LengthPrefix,
                expected: 4,
                received: actual,
            } if actual == received
        ));
    }

    for received in 0..payload.len() {
        let mut reader = CappedReader::new(frame[..4 + received].to_vec(), 4);
        let error = read_single_frame_v1(&mut reader).expect_err("payload is incomplete");
        assert!(matches!(
            error,
            FrameErrorV1::UnexpectedEof {
                phase: FrameIoPhaseV1::Payload,
                expected: 3,
                received: actual,
            } if actual == received
        ));
    }

    let mut trailing = frame.clone();
    trailing.extend_from_slice(b"extra");
    assert!(matches!(
        decode_single_frame_v1(&trailing),
        Err(FrameErrorV1::TrailingData)
    ));
    let mut reader = CappedReader::new(trailing, 4);
    assert!(matches!(
        read_single_frame_v1(&mut reader),
        Err(FrameErrorV1::TrailingData)
    ));
}

#[test]
fn frame_i_o_errors_preserve_their_phase_and_kind() {
    let frame = encode_frame_v1(b"abc").expect("payload must frame");
    for (phase, call) in [
        (FrameIoPhaseV1::LengthPrefix, 1),
        (FrameIoPhaseV1::Payload, 2),
        (FrameIoPhaseV1::EndOfStream, 3),
    ] {
        let mut reader =
            CappedReader::new(frame.clone(), 4).with_failure(call, io::ErrorKind::TimedOut);
        let error = read_single_frame_v1(&mut reader).expect_err("timed-out read must fail");
        match error {
            FrameErrorV1::Io {
                phase: actual,
                source,
            } => {
                assert_eq!(actual, phase);
                assert_eq!(source.kind(), io::ErrorKind::TimedOut);
            }
            other => panic!("expected I/O error, received {other:?}"),
        }
    }

    let mut writer = CappedWriter::new(16).with_failure(2, io::ErrorKind::BrokenPipe);
    let error = write_frame_v1(&mut writer, b"abc").expect_err("payload write must fail");
    match error {
        FrameErrorV1::Io {
            phase: FrameIoPhaseV1::Payload,
            source,
        } => assert_eq!(source.kind(), io::ErrorKind::BrokenPipe),
        other => panic!("expected payload I/O error, received {other:?}"),
    }
}

#[test]
fn payload_codecs_round_trip_requests_outputs_and_errors() {
    let request = valid_request();
    let request_payload = encode_request_payload_v1(&request).expect("request must encode");
    assert_eq!(
        decode_request_payload_v1(&request_payload).expect("request must decode"),
        request
    );

    let output = ResponseEnvelopeV2::OutputV2(valid_output());
    let output_payload = encode_response_payload_v2(&output).expect("output must encode");
    assert_eq!(
        decode_response_payload_v2(&output_payload).expect("output must decode"),
        output
    );

    let error = ResponseEnvelopeV2::Error(valid_error());
    let error_payload = encode_response_payload_v2(&error).expect("error must encode");
    assert_eq!(
        decode_response_payload_v2(&error_payload).expect("error must decode"),
        error
    );
}
#[test]
fn payload_encoding_rejects_compact_documents_larger_than_one_frame() {
    let mut payload = Map::new();
    payload.insert(
        "blob".to_owned(),
        Value::String("x".repeat(MAX_FRAME_PAYLOAD_BYTES_V1)),
    );

    assert!(matches!(
        encode_request_payload_v1(&valid_request_with_payload(payload)),
        Err(PayloadCodecErrorV1::InvalidLength(ProtocolError::FrameTooLarge {
            length,
            maximum: MAX_FRAME_PAYLOAD_BYTES_V1,
        })) if length > MAX_FRAME_PAYLOAD_BYTES_V1
    ));
}

#[test]
fn framing_rejects_malformed_and_catalog_invalid_errors() {
    assert!(matches!(
        decode_single_frame_v1(&[0, 0, 0]),
        Err(FrameErrorV1::UnexpectedEof {
            phase: FrameIoPhaseV1::LengthPrefix,
            expected: 4,
            received: 3,
        })
    ));
    assert!(matches!(
        decode_single_frame_v1(&[0, 0, 0, 0]),
        Err(FrameErrorV1::InvalidLength(ProtocolError::ZeroLengthFrame))
    ));
    assert!(matches!(
        decode_single_frame_v1(&[0, 0, 0, 1, 123, 125]),
        Err(FrameErrorV1::TrailingData)
    ));
    let oversized_frame = (u32::try_from(MAX_FRAME_PAYLOAD_BYTES_V1 + 1)
        .expect("frame maximum must fit the wire prefix"))
    .to_be_bytes();
    assert!(matches!(
        decode_single_frame_v1(&oversized_frame),
        Err(FrameErrorV1::InvalidLength(ProtocolError::FrameTooLarge {
            length,
            maximum: MAX_FRAME_PAYLOAD_BYTES_V1,
        })) if length == MAX_FRAME_PAYLOAD_BYTES_V1 + 1
    ));
    assert!(matches!(
        decode_response_payload_v2(&vec![b' '; MAX_FRAME_PAYLOAD_BYTES_V1 + 1]),
        Err(PayloadCodecErrorV1::InvalidLength(ProtocolError::FrameTooLarge {
            length,
            maximum: MAX_FRAME_PAYLOAD_BYTES_V1,
        })) if length == MAX_FRAME_PAYLOAD_BYTES_V1 + 1
    ));

    let mut unsupported_protocol =
        serde_json::to_value(valid_request()).expect("request fixture must serialize");
    unsupported_protocol["protocol"] = json!("podway.ipc/v2");
    let unsupported_protocol =
        serde_json::to_vec(&unsupported_protocol).expect("fixture must serialize");
    assert!(matches!(
        decode_request_payload_v1(&unsupported_protocol),
        Err(PayloadCodecErrorV1::JsonContract(ProtocolError::UnsupportedProtocol {
            received,
            ..
        })) if received == "podway.ipc/v2"
    ));

    let mut unsupported_schema =
        serde_json::to_value(valid_output()).expect("output fixture must serialize");
    unsupported_schema["schema"] = json!("podway.output/v4");
    let unsupported_schema =
        serde_json::to_vec(&unsupported_schema).expect("fixture must serialize");
    assert!(matches!(
        decode_response_payload_v2(&unsupported_schema),
        Err(PayloadCodecErrorV1::UnsupportedResponseSchema { received, .. })
            if received == "podway.output/v4"
    ));

    let valid = serde_json::to_value(valid_error()).expect("error fixture must serialize");

    let mut invalid_catalog_code = valid.clone();
    invalid_catalog_code["code"] = json!(false);
    let invalid_catalog_code =
        serde_json::to_vec(&invalid_catalog_code).expect("invalid fixture must serialize");
    assert!(matches!(
        decode_response_payload_v2(&invalid_catalog_code),
        Err(PayloadCodecErrorV1::InvalidEnvelope(error))
            if error.to_string().contains("invalid type")
    ));

    let mut unknown_catalog_code = valid.clone();
    unknown_catalog_code["code"] = json!("PRECONDITION_FAILED");
    let unknown_catalog_code =
        serde_json::to_vec(&unknown_catalog_code).expect("unknown fixture must serialize");
    assert!(matches!(
        decode_response_payload_v2(&unknown_catalog_code),
        Err(PayloadCodecErrorV1::InvalidEnvelope(error))
            if error
                .to_string()
                .contains("error code is not defined in the v1 catalog")
    ));

    let mut mismatched_catalog_metadata = valid;
    mismatched_catalog_metadata["exit_code"] = json!(1);
    let mismatched_catalog_metadata =
        serde_json::to_vec(&mismatched_catalog_metadata).expect("mismatch fixture must serialize");
    assert!(matches!(
        decode_response_payload_v2(&mismatched_catalog_metadata),
        Err(PayloadCodecErrorV1::InvalidEnvelope(error))
            if error
                .to_string()
                .contains("error code REQUEST_INVALID requires exit code 2 and retryable=false")
    ));
}

#[test]
fn payload_codecs_reject_invalid_json_contracts_in_protocol_order() {
    assert!(matches!(
        decode_request_payload_v1(&[0xff]),
        Err(PayloadCodecErrorV1::InvalidUtf8(_))
    ));
    assert!(matches!(
        decode_request_payload_v1(b"{"),
        Err(PayloadCodecErrorV1::InvalidJson(_))
    ));
    assert!(matches!(
        decode_request_payload_v1(b"[]"),
        Err(PayloadCodecErrorV1::MissingOrInvalidDiscriminator { field: "protocol" })
    ));
    assert!(matches!(
        decode_response_payload_v2(b"{}"),
        Err(PayloadCodecErrorV1::MissingOrInvalidDiscriminator { field: "schema" })
    ));

    let mut unsupported = serde_json::to_value(valid_request()).expect("request must serialize");
    unsupported["protocol"] = json!("podway.ipc/v2");
    unsupported["command"] = json!(false);
    let unsupported = serde_json::to_vec(&unsupported).expect("fixture must serialize");
    assert!(matches!(
        decode_request_payload_v1(&unsupported),
        Err(PayloadCodecErrorV1::JsonContract(ProtocolError::UnsupportedProtocol {
            received,
            ..
        })) if received == "podway.ipc/v2"
    ));

    let unsupported_schema =
        serde_json::to_vec(&json!({"schema": "podway.output/v4"})).expect("fixture must serialize");
    assert!(matches!(
        decode_response_payload_v2(&unsupported_schema),
        Err(PayloadCodecErrorV1::UnsupportedResponseSchema { received, .. })
            if received == "podway.output/v4"
    ));

    let mut non_string_schema =
        serde_json::to_value(valid_output()).expect("output must serialize");
    non_string_schema["schema"] = json!(false);
    let non_string_schema = serde_json::to_vec(&non_string_schema).expect("fixture must serialize");
    assert!(matches!(
        decode_response_payload_v2(&non_string_schema),
        Err(PayloadCodecErrorV1::MissingOrInvalidDiscriminator { field: "schema" })
    ));
}

#[test]
fn request_payload_depth_limit_is_applied_to_the_whole_document() {
    let mut depth_64 = serde_json::to_value(valid_request()).expect("request must serialize");
    depth_64["payload"] = json!({"nested": nested_arrays(62)});
    let depth_64 = serde_json::to_vec(&depth_64).expect("fixture must serialize");
    assert!(decode_request_payload_v1(&depth_64).is_ok());

    let mut depth_65 = serde_json::to_value(valid_request()).expect("request must serialize");
    depth_65["payload"] = json!({"nested": nested_arrays(63)});
    let depth_65 = serde_json::to_vec(&depth_65).expect("fixture must serialize");
    assert!(matches!(
        decode_request_payload_v1(&depth_65),
        Err(PayloadCodecErrorV1::JsonContract(
            ProtocolError::JsonDepthExceeded { maximum: 64 }
        ))
    ));
}

#[test]
fn payload_codec_and_frame_decoders_are_safe_for_a_small_arbitrary_byte_corpus() {
    let mut corpus = vec![
        Vec::new(),
        vec![0xff],
        vec![0, 0, 0, 0],
        vec![0, 0, 0, 1],
        vec![0, 16, 0, 1],
        b"{}{}".to_vec(),
    ];
    corpus.extend((0_u8..=u8::MAX).map(|byte| vec![byte]));

    let mut state = 0x9e37_79b9_u64;
    for length in 0..64 {
        let mut bytes = Vec::with_capacity(length);
        for _ in 0..length {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            bytes.push((state >> 24) as u8);
        }
        corpus.push(bytes);
    }

    for bytes in corpus {
        let _ = decode_single_frame_v1(&bytes);
        let _ = decode_request_payload_v1(&bytes);
        let _ = decode_response_payload_v2(&bytes);
    }
}

fn nested_arrays(count: usize) -> Value {
    let mut value = Value::Null;
    for _ in 0..count {
        value = Value::Array(vec![value]);
    }
    value
}
