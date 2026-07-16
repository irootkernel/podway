use podway_core::{JobId, Revision, SessionId, WorkspaceId};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, ERROR_SCHEMA_V1, ErrorCodeV1, ErrorEnvelopeInputV1,
    ErrorEnvelopeV1, ExitCodeV1, FRAME_LENGTH_PREFIX_BYTES_V1, IPC_PROTOCOL_V1, JobOutputV1,
    JobStateV1, MAX_FRAME_PAYLOAD_BYTES_V1, MAX_JSON_DEPTH_V1, MAX_WORKSPACE_ROOT_SCALARS_V1,
    MIN_FRAME_PAYLOAD_BYTES_V1, OUTPUT_SCHEMA_V1, OperationV1, OutputEnvelopeInputV1,
    OutputEnvelopeV1, PreconditionsV1, ProtocolCompatibilityV1, ProtocolError, ProtocolVersionV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV1,
    Rfc3339MillisV1, SUPPORTED_ERROR_SCHEMAS_V1, SUPPORTED_OUTPUT_SCHEMAS_V1,
    SUPPORTED_PROTOCOLS_V1, SessionLifecycleV1, SessionOutputV1, WorkspaceOutputV1,
    negotiate_protocol, require_compatible_protocol, validate_frame_payload_length,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};

const REQUEST_ID: &str = "123e4567-e89b-12d3-a456-426614174000";

// API-004: v1 IPC names, schemas, and bounded frame limits are frozen public contracts.
#[test]
fn api_004_v1_constants_and_frame_payload_bounds_are_stable() {
    assert_eq!(IPC_PROTOCOL_V1, "podway.ipc/v1");
    assert_eq!(OUTPUT_SCHEMA_V1, "podway.output/v1");
    assert_eq!(ERROR_SCHEMA_V1, "podway.error/v1");
    assert_eq!(SUPPORTED_PROTOCOLS_V1, &[IPC_PROTOCOL_V1]);
    assert_eq!(SUPPORTED_OUTPUT_SCHEMAS_V1, &[OUTPUT_SCHEMA_V1]);
    assert_eq!(SUPPORTED_ERROR_SCHEMAS_V1, &[ERROR_SCHEMA_V1]);
    assert_eq!(FRAME_LENGTH_PREFIX_BYTES_V1, 4);
    assert_eq!(MIN_FRAME_PAYLOAD_BYTES_V1, 1);
    assert_eq!(MAX_FRAME_PAYLOAD_BYTES_V1, 1_048_576);

    assert_eq!(
        validate_frame_payload_length(MIN_FRAME_PAYLOAD_BYTES_V1),
        Ok(())
    );
    assert_eq!(
        validate_frame_payload_length(MAX_FRAME_PAYLOAD_BYTES_V1),
        Ok(())
    );
    assert_eq!(
        validate_frame_payload_length(0),
        Err(ProtocolError::ZeroLengthFrame)
    );
    assert_eq!(
        validate_frame_payload_length(MAX_FRAME_PAYLOAD_BYTES_V1 + 1),
        Err(ProtocolError::FrameTooLarge {
            length: MAX_FRAME_PAYLOAD_BYTES_V1 + 1,
            maximum: MAX_FRAME_PAYLOAD_BYTES_V1,
        })
    );
}

// API-004: Negotiation recognizes only the advertised v1 IPC protocol.
#[test]
fn api_004_protocol_version_negotiation_accepts_v1_and_rejects_unknown_versions() {
    assert_eq!(ProtocolVersionV1::V1.identifier(), IPC_PROTOCOL_V1);
    assert_eq!(
        negotiate_protocol(IPC_PROTOCOL_V1),
        ProtocolCompatibilityV1::Compatible {
            version: ProtocolVersionV1::V1,
        }
    );
    assert_eq!(
        require_compatible_protocol(IPC_PROTOCOL_V1),
        Ok(ProtocolVersionV1::V1)
    );

    assert_eq!(
        negotiate_protocol("podway.ipc/v2"),
        ProtocolCompatibilityV1::Unsupported {
            received: "podway.ipc/v2".to_owned(),
            supported: SUPPORTED_PROTOCOLS_V1,
        }
    );
    assert_eq!(
        require_compatible_protocol("podway.ipc/v2"),
        Err(ProtocolError::UnsupportedProtocol {
            received: "podway.ipc/v2".to_owned(),
            supported: SUPPORTED_PROTOCOLS_V1,
        })
    );
}

fn valid_query_envelope() -> RequestEnvelopeV1 {
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        client: ClientInfoV1::new("podway-cli", "0.1.0", 1).unwrap(),
        operation: OperationV1::Query,
        command: CommandNameV1::new("status").unwrap(),
        workspace: None,
        idempotency_key: None,
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).unwrap(),
        payload: Map::new(),
    })
    .unwrap()
}

fn valid_query_envelope_json() -> Value {
    json!({
        "protocol": IPC_PROTOCOL_V1,
        "request_id": REQUEST_ID,
        "client": {"name": "podway-cli", "version": "0.1.0", "pid": 1},
        "operation": "query",
        "command": "status",
        "options": {"detach": false, "wait_timeout_ms": 0},
        "payload": {},
    })
}

// API-004: Request envelopes validate in memory and reject invalid serialized wire data.
#[test]
fn api_004_request_envelope_validation_and_serde_rejection_are_enforced() {
    let envelope = valid_query_envelope();
    assert_eq!(envelope.validate(), Ok(()));

    let serialized = serde_json::to_value(&envelope).expect("validated request must serialize");
    assert_eq!(serialized["protocol"], IPC_PROTOCOL_V1);
    let decoded = serde_json::from_value::<RequestEnvelopeV1>(serialized)
        .expect("serialized validated request must deserialize");
    assert_eq!(decoded, envelope);

    let mut unexpected_idempotency_key = valid_query_envelope_json();
    unexpected_idempotency_key
        .as_object_mut()
        .expect("request fixture must be an object")
        .insert("idempotency_key".to_owned(), json!("request-key"));
    assert!(serde_json::from_value::<RequestEnvelopeV1>(unexpected_idempotency_key).is_err());

    let mut unsupported_protocol = valid_query_envelope_json();
    unsupported_protocol["protocol"] = json!("podway.ipc/v2");
    assert!(serde_json::from_value::<RequestEnvelopeV1>(unsupported_protocol).is_err());

    let mut unknown_field = valid_query_envelope_json();
    unknown_field["unknown"] = json!(true);
    assert!(serde_json::from_value::<RequestEnvelopeV1>(unknown_field).is_err());
    for field in ["workspace", "idempotency_key", "preconditions"] {
        let mut request = valid_query_envelope_json();
        request[field] = Value::Null;
        assert!(
            serde_json::from_value::<RequestEnvelopeV1>(request).is_err(),
            "explicit null must be rejected for {field}"
        );
    }

    let mut null_workspace_identity = valid_query_envelope_json();
    null_workspace_identity["workspace"] = json!({"root": "/workspace", "expected_uuid": null});
    assert!(serde_json::from_value::<RequestEnvelopeV1>(null_workspace_identity).is_err());

    for field in [
        "session_id",
        "session_revision",
        "attempt_id",
        "item_revision",
        "blocker_id",
        "job_state",
    ] {
        let mut request = valid_query_envelope_json();
        request["preconditions"] = json!({});
        request["preconditions"][field] = Value::Null;
        assert!(
            serde_json::from_value::<RequestEnvelopeV1>(request).is_err(),
            "explicit null must be rejected for preconditions.{field}"
        );
    }
}
const WORKSPACE_ID: &str = "223e4567-e89b-12d3-a456-426614174000";
const JOB_ID: &str = "323e4567-e89b-12d3-a456-426614174000";
const SESSION_ID: &str = "423e4567-e89b-12d3-a456-426614174000";
const GENERATED_AT: &str = "2026-07-14T12:34:56.789Z";

fn timestamp() -> Rfc3339MillisV1 {
    Rfc3339MillisV1::new(GENERATED_AT).expect("timestamp fixture must be valid")
}

fn valid_output_envelope() -> OutputEnvelopeV1 {
    let workspace = WorkspaceOutputV1::new(
        WorkspaceId::new(WORKSPACE_ID).expect("workspace id fixture must be valid"),
        "/tmp/podway",
        7,
    )
    .expect("workspace fixture must be valid");
    let job = JobOutputV1::new(
        JobId::new(JOB_ID).expect("job id fixture must be valid"),
        1,
        JobStateV1::Succeeded,
        timestamp(),
        Some(timestamp()),
        Some(timestamp()),
    )
    .expect("job fixture must be valid");
    let session = SessionOutputV1::new(
        SessionId::new(SESSION_ID).expect("session id fixture must be valid"),
        "Phase 0",
        SessionLifecycleV1::Completed,
        Revision::new(1),
        Revision::new(2),
    )
    .expect("session fixture must be valid");

    let mut result = Map::new();
    result.insert("status".to_owned(), json!({"complete": true}));

    OutputEnvelopeV1::new(OutputEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).expect("request id fixture must be valid"),
        command: CommandNameV1::new("session.status").expect("command fixture must be valid"),
        generated_at: timestamp(),
        workspace: Some(workspace),
        job: Some(job),
        session: Some(session),
        result,
        warnings: vec![Map::new()],
    })
    .expect("output fixture must be valid")
}

fn valid_error_envelope() -> ErrorEnvelopeV1 {
    let mut workspace = Map::new();
    workspace.insert("uuid".to_owned(), json!(WORKSPACE_ID));

    let mut details = Map::new();
    details.insert("reason".to_owned(), json!("precondition"));

    ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).expect("request id fixture must be valid"),
        command: CommandNameV1::new("session.status").expect("command fixture must be valid"),
        generated_at: timestamp(),
        code: ErrorCodeV1::new("PRECONDITION_FAILED").expect("error code fixture must be valid"),
        message: "Session revision changed.".to_owned(),
        retryable: true,
        exit_code: ExitCodeV1::new(3).expect("exit code fixture must be valid"),
        workspace: Some(workspace),
        details,
    })
    .expect("error fixture must be valid")
}

fn valid_output_envelope_json() -> Value {
    serde_json::to_value(valid_output_envelope()).expect("output fixture must serialize")
}

fn valid_error_envelope_json() -> Value {
    serde_json::to_value(valid_error_envelope()).expect("error fixture must serialize")
}

fn assert_wire_type<T>()
where
    T: Serialize + DeserializeOwned,
{
    let _ = std::mem::size_of::<T>();
}

fn assert_round_trip<T>(value: T)
where
    T: Serialize + DeserializeOwned + Eq + std::fmt::Debug,
{
    let encoded = serde_json::to_value(&value).expect("validated value must serialize");
    let decoded = serde_json::from_value::<T>(encoded).expect("serialized value must deserialize");
    assert_eq!(decoded, value);
}

fn assert_output_rejected(value: Value) {
    assert!(serde_json::from_value::<OutputEnvelopeV1>(value.clone()).is_err());
    assert!(serde_json::from_value::<ResponseEnvelopeV1>(value).is_err());
}

fn assert_error_rejected(value: Value) {
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(value.clone()).is_err());
    assert!(serde_json::from_value::<ResponseEnvelopeV1>(value).is_err());
}

// API-004: The public response wire surface is serializable, deserializable, and schema-stable.
#[test]
fn api_004_response_wire_surface_is_executable_and_round_trips() {
    assert_wire_type::<WorkspaceOutputV1>();
    assert_wire_type::<JobOutputV1>();
    assert_wire_type::<SessionOutputV1>();
    assert_wire_type::<OutputEnvelopeV1>();
    assert_wire_type::<ErrorCodeV1>();
    assert_wire_type::<ExitCodeV1>();
    assert_wire_type::<ErrorEnvelopeV1>();
    assert_wire_type::<ResponseEnvelopeV1>();

    let output = valid_output_envelope();
    let output_value = serde_json::to_value(&output).expect("output fixture must serialize");
    assert_eq!(output_value["schema"], OUTPUT_SCHEMA_V1);
    assert_eq!(output_value["command"], "session.status");
    assert_round_trip(output.clone());
    assert_round_trip(ResponseEnvelopeV1::Output(output));

    let error = valid_error_envelope();
    let error_value = serde_json::to_value(&error).expect("error fixture must serialize");
    assert_eq!(error_value["schema"], ERROR_SCHEMA_V1);
    assert_eq!(error_value["command"], "session.status");
    assert_round_trip(error.clone());
    assert_round_trip(ResponseEnvelopeV1::Error(error));
}

// API-004: Validated response DTOs reject invariant-bypassing wire payloads at every level.
#[test]
fn api_004_response_deserialization_rejects_invalid_nested_values_and_unknown_fields() {
    let mut wrong_output_schema = valid_output_envelope_json();
    wrong_output_schema["schema"] = json!("podway.output/v2");
    assert_output_rejected(wrong_output_schema);

    let mut wrong_error_schema = valid_error_envelope_json();
    wrong_error_schema["schema"] = json!("podway.error/v2");
    assert_error_rejected(wrong_error_schema);

    let mut zero_job_sequence = valid_output_envelope_json();
    zero_job_sequence["job"]["sequence"] = json!(0);
    assert!(serde_json::from_value::<JobOutputV1>(zero_job_sequence["job"].clone()).is_err());
    assert_output_rejected(zero_job_sequence);

    let mut invalid_job_state = valid_output_envelope_json();
    invalid_job_state["job"]["state"] = json!("stopped");
    assert_output_rejected(invalid_job_state);

    let mut invalid_nested_timestamp = valid_output_envelope_json();
    invalid_nested_timestamp["job"]["submitted_at"] = json!("2026-07-14T12:34:56Z");
    assert_output_rejected(invalid_nested_timestamp);

    let mut oversized_workspace_root = valid_output_envelope_json();
    oversized_workspace_root["workspace"]["root"] =
        json!("x".repeat(MAX_WORKSPACE_ROOT_SCALARS_V1 + 1));
    assert!(
        serde_json::from_value::<WorkspaceOutputV1>(oversized_workspace_root["workspace"].clone())
            .is_err()
    );
    assert_output_rejected(oversized_workspace_root);
    let mut missing_finished_at = valid_output_envelope_json();
    missing_finished_at["job"]
        .as_object_mut()
        .expect("job fixture must be an object")
        .remove("finished_at");
    assert!(serde_json::from_value::<JobOutputV1>(missing_finished_at["job"].clone()).is_err());
    assert_output_rejected(missing_finished_at);

    let mut oversized_session_title = valid_output_envelope_json();
    oversized_session_title["session"]["title"] = json!("x".repeat(501));
    assert_output_rejected(oversized_session_title);

    let mut null_output_workspace = valid_output_envelope_json();
    null_output_workspace["workspace"] = Value::Null;
    assert_output_rejected(null_output_workspace);

    let mut unknown_nested_output_field = valid_output_envelope_json();
    unknown_nested_output_field["session"]["unknown"] = json!(true);
    assert!(
        serde_json::from_value::<SessionOutputV1>(unknown_nested_output_field["session"].clone())
            .is_err()
    );
    assert_output_rejected(unknown_nested_output_field);

    let mut unknown_output_field = valid_output_envelope_json();
    unknown_output_field["unknown"] = json!(true);
    assert_output_rejected(unknown_output_field);

    let mut deeply_nested_warning = Value::Null;
    for _ in 0..=MAX_JSON_DEPTH_V1 {
        deeply_nested_warning = Value::Array(vec![deeply_nested_warning]);
    }
    let mut warning = Map::new();
    warning.insert("nested".to_owned(), deeply_nested_warning);
    let mut oversized_nested_collection = valid_output_envelope_json();
    oversized_nested_collection["warnings"] = Value::Array(vec![Value::Object(warning)]);
    assert_output_rejected(oversized_nested_collection);

    assert!(serde_json::from_value::<ExitCodeV1>(json!(0)).is_err());
    assert!(serde_json::from_value::<ExitCodeV1>(json!(7)).is_err());

    let mut invalid_exit_code = valid_error_envelope_json();
    invalid_exit_code["exit_code"] = json!(7);
    assert_error_rejected(invalid_exit_code);
    let mut null_error_workspace = valid_error_envelope_json();
    null_error_workspace["workspace"] = Value::Null;
    assert_error_rejected(null_error_workspace);
    let mut empty_error_message = valid_error_envelope_json();
    empty_error_message["message"] = json!("");
    assert_error_rejected(empty_error_message);

    let mut deeply_nested_detail = Value::Null;
    for _ in 0..=MAX_JSON_DEPTH_V1 {
        deeply_nested_detail = Value::Array(vec![deeply_nested_detail]);
    }
    let mut deeply_nested_error_details = valid_error_envelope_json();
    deeply_nested_error_details["details"] = json!({"nested": deeply_nested_detail});
    assert_error_rejected(deeply_nested_error_details);

    let mut invalid_error_code = valid_error_envelope_json();
    invalid_error_code["code"] = json!("precondition_failed");
    assert_error_rejected(invalid_error_code);

    let mut unknown_error_field = valid_error_envelope_json();
    unknown_error_field["unknown"] = json!(true);
    assert_error_rejected(unknown_error_field);
}
