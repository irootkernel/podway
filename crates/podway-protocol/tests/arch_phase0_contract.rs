use podway_core::{JobId, Revision, SessionId, WorkspaceId};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, ERROR_SCHEMA_V1, ErrorCodeV1, ErrorEnvelopeInputV1,
    ErrorEnvelopeV1, ExitCodeV1, FRAME_LENGTH_PREFIX_BYTES_V1, IPC_PROTOCOL_V1, JobOutputV1,
    JobStateV1, MAX_FRAME_PAYLOAD_BYTES_V1, MAX_JSON_DEPTH_V1, MAX_WORKSPACE_ROOT_SCALARS_V1,
    MIN_FRAME_PAYLOAD_BYTES_V1, OUTPUT_SCHEMA_V3, OperationV1, OutputEnvelopeInputV3,
    OutputEnvelopeV3, PreconditionsV1, ProtocolCompatibilityV1, ProtocolError, ProtocolVersionV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    Rfc3339MillisV1, SUPPORTED_ERROR_SCHEMAS_V1, SUPPORTED_OUTPUT_SCHEMAS_V3,
    SUPPORTED_PROTOCOLS_V1, SessionLifecycleV1, SessionOutputV1, WorkspaceOutputV1,
    build_identity_v1, decode_request_payload_v1, decode_response_payload_v2,
    encode_request_payload_v1, encode_response_payload_v2, error_code_catalog_v1,
    negotiate_protocol, require_compatible_protocol, validate_frame_payload_length,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};

const REQUEST_ID: &str = "123e4567-e89b-12d3-a456-426614174000";

// API-004: v1 IPC names, schemas, and bounded frame limits are frozen public contracts.
#[test]
fn api_004_v1_constants_and_frame_payload_bounds_are_stable() {
    assert_eq!(IPC_PROTOCOL_V1, "podway.ipc/v1");
    assert_eq!(OUTPUT_SCHEMA_V3, "podway.output/v3");
    assert_eq!(ERROR_SCHEMA_V1, "podway.error/v1");
    assert_eq!(SUPPORTED_PROTOCOLS_V1, &[IPC_PROTOCOL_V1]);
    assert_eq!(SUPPORTED_OUTPUT_SCHEMAS_V3, &[OUTPUT_SCHEMA_V3]);
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
    let identity = build_identity_v1();
    json!({
        "protocol": IPC_PROTOCOL_V1,
        "request_id": REQUEST_ID,
        "client": {
            "name": "podway-cli",
            "version": "0.1.0",
            "pid": 1,
            "product": identity.product(),
            "contract_manifest_digest": identity.contract_manifest_digest(),
        },
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

    for field in ["product", "contract_manifest_digest"] {
        let mut missing_identity = valid_query_envelope_json();
        missing_identity["client"]
            .as_object_mut()
            .expect("client fixture must be an object")
            .remove(field);
        assert!(
            serde_json::from_value::<RequestEnvelopeV1>(missing_identity).is_err(),
            "client.{field} is required"
        );
    }
    let mut malformed_digest = valid_query_envelope_json();
    malformed_digest["client"]["contract_manifest_digest"] = json!("sha256:ABC");
    assert!(serde_json::from_value::<RequestEnvelopeV1>(malformed_digest).is_err());

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

fn valid_output_envelope() -> OutputEnvelopeV3 {
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

    OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
        request_id: RequestIdV1::new(REQUEST_ID).expect("request id fixture must be valid"),
        command: CommandNameV1::new("workspace.show").expect("command fixture must be valid"),
        generated_at: timestamp(),
        workspace: Some(workspace),
        job: Some(job),
        session: Some(session),
        result,
        warnings: Vec::new(),
    })
    .expect("output fixture must be valid")
}

fn valid_error_envelope() -> ErrorEnvelopeV1 {
    let mut workspace = Map::new();
    workspace.insert("uuid".to_owned(), json!(WORKSPACE_ID));

    let details = Map::from_iter([
        ("expected_revision".to_owned(), json!(3)),
        ("current_revision".to_owned(), json!(4)),
    ]);

    ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).expect("request id fixture must be valid"),
        command: CommandNameV1::new("session.status").expect("command fixture must be valid"),
        generated_at: timestamp(),
        code: ErrorCodeV1::new("ITEM_REVISION_CONFLICT").expect("error code fixture must be valid"),
        message: "Session revision changed.".to_owned(),
        retryable: true,
        exit_code: ExitCodeV1::new(4).expect("exit code fixture must be valid"),
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

const FROZEN_ERROR_CATALOG: &[(&str, u8, bool)] = &[
    ("DAEMON_NOT_INSTALLED", 3, false),
    ("DAEMON_UNAVAILABLE", 3, true),
    ("DAEMON_SHUTTING_DOWN", 3, true),
    ("DAEMON_VERSION_INCOMPATIBLE", 3, false),
    ("DAEMON_CONTRACT_MISMATCH", 3, false),
    ("PROTOCOL_VERSION_UNSUPPORTED", 3, false),
    ("REQUEST_TOO_LARGE", 2, false),
    ("REQUEST_INVALID", 2, false),
    ("SOCKET_ENDPOINT_INVALID", 2, false),
    ("NOT_A_GIT_WORKTREE", 5, false),
    ("BARE_GIT_REPOSITORY", 5, false),
    ("WORKTREE_GONE", 5, false),
    ("WORKSPACE_NOT_INITIALIZED", 5, false),
    ("WORKSPACE_ALREADY_INITIALIZED", 1, false),
    ("WORKSPACE_INIT_CONFLICT", 5, false),
    ("WORKSPACE_ID_CONFLICT", 5, false),
    ("WORKSPACE_UUID_MISMATCH", 4, false),
    ("WORKSPACE_CONFIG_INVALID", 5, false),
    ("WORKSPACE_STATE_UNREADABLE", 5, false),
    ("WORKSPACE_SCHEMA_UNSUPPORTED", 5, false),
    ("LEGACY_PROCEDURE_STATE_UNSUPPORTED", 5, false),
    ("WORKSPACE_QUEUE_FULL", 4, true),
    ("WORKSPACE_MAINTENANCE", 4, true),
    ("WORKSPACE_PATH_UNSAFE", 5, false),
    ("PATH_OUTSIDE_WORKTREE", 5, false),
    ("MIGRATION_FAILED", 5, false),
    ("PROCEDURE_NOT_FOUND", 1, false),
    ("PROCEDURE_INVALID", 1, false),
    ("PROCEDURE_SCHEMA_UNSUPPORTED", 1, false),
    ("PROCEDURE_DIGEST_MISMATCH", 4, false),
    ("PRESET_NOT_FOUND", 1, false),
    ("SESSION_NOT_FOUND", 1, false),
    ("SESSION_ID_MISMATCH", 4, false),
    ("SESSION_ALREADY_EXISTS", 1, false),
    ("SESSION_NOT_RUNNING", 1, false),
    ("SESSION_NOT_TERMINAL", 1, false),
    ("SESSION_RESET_NOT_ELIGIBLE", 1, false),
    ("SESSION_CANCELLED", 1, false),
    ("SESSION_REVISION_CONFLICT", 4, true),
    ("ATTEMPT_NOT_CURRENT", 4, true),
    ("STAGE_NOT_SKIPPABLE", 1, false),
    ("REQUIRED_ITEMS_MISSING", 1, false),
    ("BLOCKERS_PRESENT", 1, false),
    ("BLOCKER_LIMIT_REACHED", 1, false),
    ("ITEM_NOT_FOUND", 1, false),
    ("ITEM_TYPE_MISMATCH", 1, false),
    ("ITEM_CONSTRAINT_FAILED", 1, false),
    ("ITEM_REVISION_CONFLICT", 4, true),
    ("ITEM_ALREADY_SET", 4, true),
    ("LIST_VALUE_NOT_FOUND", 1, false),
    ("LIST_VALUE_DUPLICATE", 1, false),
    ("ARTIFACT_NOT_FOUND", 1, false),
    ("ARTIFACT_UNREADABLE", 5, false),
    ("ARTIFACT_CHANGED", 1, true),
    ("ARTIFACT_MEDIA_TYPE_NOT_ALLOWED", 1, false),
    ("BLOCKER_NOT_FOUND", 1, false),
    ("BLOCKER_NOT_CURRENT", 4, true),
    ("IDEMPOTENCY_KEY_REUSED", 2, false),
    ("JOB_NOT_FOUND", 1, false),
    ("JOB_NOT_CANCELLABLE", 1, false),
    ("JOB_WAIT_TIMEOUT", 4, true),
    ("MUTATION_OUTCOME_UNKNOWN", 4, true),
    ("CONFIRMATION_REQUIRED", 2, false),
    ("INTERNAL_ERROR", 6, false),
    ("PROCEDURE_V2_SCHEMA_INVALID", 1, false),
    ("GRAPH_NODE_NOT_FOUND", 1, false),
    ("NODE_DEFINITION_NOT_FOUND", 1, false),
    ("GRAPH_NODE_TYPE_MISMATCH", 1, false),
    ("OPTION_NOT_ALLOWED", 1, false),
    ("ROUTE_NOT_ALLOWED", 1, false),
    ("DECISION_REASON_MISSING", 1, false),
    ("EVIDENCE_REFERENCE_UNRESOLVED", 1, false),
    ("EVIDENCE_REFERENCE_STALE", 4, true),
    ("MANUAL_REWORK_TARGET_NOT_ALLOWED", 1, false),
    ("MANUAL_REWORK_TARGET_NOT_ON_TRACE", 1, false),
    ("GOAL_TRACKING_NOT_ENABLED", 1, false),
    ("SESSION_GOAL_MISSING", 1, false),
    ("SESSION_GOAL_ALREADY_DEFINED", 1, false),
    ("GOAL_REVISION_STALE", 4, true),
    ("GOAL_REVISION_TARGET_NOT_ALLOWED", 1, false),
    ("GOAL_REVISION_TARGET_NOT_REVISION_SAFE", 1, false),
    ("REACTIVATION_FLAG_REQUIRED", 1, false),
    ("CRITERION_MODE_MIXED", 1, false),
    ("CRITERION_CITATION_INVALID", 1, false),
    ("CRITERION_RESULT_MISSING", 1, false),
    ("CRITERION_NOT_FOUND", 1, false),
    ("FRESH_GOAL_ASSESSMENT_MISSING", 1, false),
    ("GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED", 1, false),
    ("DIGEST_CONFIRMATION_REQUIRED", 2, false),
    ("UNSUPPORTED_V2_CAPABILITY", 3, false),
];

fn v2_runtime_error_details(code: &str) -> Option<Map<String, Value>> {
    let fields = match code {
        "PROCEDURE_V2_SCHEMA_INVALID" => json!({"diagnostic_codes":["AUTHORING_SCHEMA_INVALID"]}),
        "GRAPH_NODE_NOT_FOUND" | "DECISION_REASON_MISSING" => json!({"graph_node_id":"review"}),
        "NODE_DEFINITION_NOT_FOUND" => json!({"node_definition_id":"review"}),
        "GRAPH_NODE_TYPE_MISMATCH" => {
            json!({"graph_node_id":"review","expected_node_type":"decision","actual_node_type":"action"})
        }
        "OPTION_NOT_ALLOWED" => {
            json!({"graph_node_id":"review","option_id":"reject","allowed_option_ids":["accept"]})
        }
        "ROUTE_NOT_ALLOWED" => json!({"graph_node_id":"review","option_id":"accept"}),
        "EVIDENCE_REFERENCE_UNRESOLVED" => {
            json!({"graph_node_id":"review","source_graph_node_ids":["build"]})
        }
        "EVIDENCE_REFERENCE_STALE" => {
            json!({"graph_node_id":"review","source_graph_node_id":"build","expected_source_attempt_id":REQUEST_ID,"current_source_attempt_id":null})
        }
        "MANUAL_REWORK_TARGET_NOT_ALLOWED"
        | "MANUAL_REWORK_TARGET_NOT_ON_TRACE"
        | "GOAL_REVISION_TARGET_NOT_ALLOWED"
        | "GOAL_REVISION_TARGET_NOT_REVISION_SAFE" => json!({"target_graph_node_id":"build"}),
        "GOAL_TRACKING_NOT_ENABLED" | "SESSION_GOAL_MISSING" | "REACTIVATION_FLAG_REQUIRED" => {
            json!({})
        }
        "SESSION_GOAL_ALREADY_DEFINED" | "FRESH_GOAL_ASSESSMENT_MISSING" => {
            json!({"goal_revision":1})
        }
        "GOAL_REVISION_STALE" => json!({"expected_goal_revision":1,"actual_goal_revision":2}),
        "CRITERION_MODE_MIXED" => {
            json!({"criterion_id":"tests","expected_mode":"assessment","actual_status":"not_applicable"})
        }
        "CRITERION_CITATION_INVALID" => {
            json!({"criterion_id":"tests","citation":{"local_item_id":"evidence"}})
        }
        "CRITERION_RESULT_MISSING" => json!({"missing_criterion_ids":["tests"]}),
        "CRITERION_NOT_FOUND" => json!({"criterion_id":"tests"}),
        "GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED" => {
            json!({"option_id":"accept","determined_outcome":"achieved","allowed_option_ids":["reject"]})
        }
        "DIGEST_CONFIRMATION_REQUIRED" => {
            json!({"procedure_digest":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"})
        }
        "UNSUPPORTED_V2_CAPABILITY" => {
            json!({"capability":"session.decide","required_result_schema":"podway.decision-result/v1"})
        }
        _ => return None,
    };
    Some(fields.as_object().unwrap().clone())
}

#[test]
fn api_004_error_catalog_is_exhaustive_and_error_pairs_fail_closed() {
    assert_eq!(
        error_code_catalog_v1().collect::<Vec<_>>(),
        FROZEN_ERROR_CATALOG,
        "the runtime catalog must contain exactly the frozen public entries"
    );
    for &(code, exit_code, retryable) in FROZEN_ERROR_CATALOG {
        let details = match code {
            "WORKSPACE_UUID_MISMATCH" => identity_details(
                "podway.workspace-uuid-mismatch-details/v2",
                "expected_workspace_uuid",
                WORKSPACE_ID,
                "actual_workspace_uuid",
                Some("523e4567-e89b-12d3-a456-426614174000"),
                false,
            ),
            "SESSION_ID_MISMATCH" => identity_details(
                "podway.session-id-mismatch-details/v2",
                "expected_session_id",
                SESSION_ID,
                "actual_session_id",
                None,
                false,
            ),
            "PROCEDURE_DIGEST_MISMATCH" => Map::from_iter([
                (
                    "schema".to_owned(),
                    json!("podway.procedure-digest-mismatch-details/v2"),
                ),
                (
                    "expected_procedure_digest".to_owned(),
                    json!(
                        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    ),
                ),
                (
                    "actual_procedure_digest".to_owned(),
                    json!(
                        "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
                    ),
                ),
                ("admission".to_owned(), json!({"admitted": false})),
            ]),
            "MUTATION_OUTCOME_UNKNOWN" => mutation_outcome_unknown_details("recon-key"),
            "DAEMON_CONTRACT_MISMATCH" => json!({
                "expected": {
                    "product": "podway",
                    "contract_manifest_digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                },
                "actual": {
                    "product": "podway",
                    "contract_manifest_digest": "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
                },
                "admission": {"admitted": false}
            })
            .as_object()
            .unwrap()
            .clone(),
            "SOCKET_ENDPOINT_INVALID" => Map::from_iter([(
                "reason".to_owned(),
                json!("relative"),
            )]),
            "SESSION_REVISION_CONFLICT" | "ITEM_REVISION_CONFLICT" => {
                Map::from_iter([("expected_revision".to_owned(), json!(1))])
            }
            "ATTEMPT_NOT_CURRENT" => Map::from_iter([(
                "expected_attempt_id".to_owned(),
                json!("00000000-0000-4000-8000-000000000019"),
            )]),
            "SESSION_RESET_NOT_ELIGIBLE" => json!({
                "lifecycle": "running",
                "current_terminal_disposition": false,
                "required_action": "force",
                "admission": {"admitted": false}
            })
            .as_object()
            .unwrap()
            .clone(),
            "BLOCKER_LIMIT_REACHED" => {
                Map::from_iter([("maximum_open_blockers".to_owned(), json!(64))])
            }
            "IDEMPOTENCY_KEY_REUSED" => {
                Map::from_iter([("admission".to_owned(), json!({"admitted": false}))])
            }
            _ => v2_runtime_error_details(code).unwrap_or_default(),
        };
        let envelope = ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
            request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
            command: CommandNameV1::new("status").unwrap(),
            generated_at: timestamp(),
            code: ErrorCodeV1::new(code).unwrap(),
            message: "catalog test".to_owned(),
            retryable,
            exit_code: ExitCodeV1::new(exit_code).unwrap(),
            workspace: None,
            details,
        })
        .unwrap();
        assert_eq!(envelope.code().as_str(), code);
        assert_eq!(envelope.exit_code().get(), exit_code);
        assert_eq!(envelope.retryable(), retryable);
        assert_round_trip(envelope);
    }

    assert_eq!(
        ErrorCodeV1::new("PRECONDITION_FAILED"),
        Err(ProtocolError::InvalidErrorCode)
    );
    assert!(serde_json::from_value::<ErrorCodeV1>(json!("PRECONDITION_FAILED")).is_err());

    let mut unknown_code = valid_error_envelope_json();
    unknown_code["code"] = json!("PRECONDITION_FAILED");
    assert_error_rejected(unknown_code);

    let mut mismatched_exit = valid_error_envelope_json();
    mismatched_exit["exit_code"] = json!(1);
    assert_error_rejected(mismatched_exit);

    let mut mismatched_retryable = valid_error_envelope_json();
    mismatched_retryable["retryable"] = json!(false);
    assert_error_rejected(mismatched_retryable);
    for (exit_code, retryable) in [(1, true), (4, false)] {
        assert!(
            ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
                request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
                command: CommandNameV1::new("status").unwrap(),
                generated_at: timestamp(),
                code: ErrorCodeV1::new("ITEM_REVISION_CONFLICT").unwrap(),
                message: "catalog mismatch".to_owned(),
                retryable,
                exit_code: ExitCodeV1::new(exit_code).unwrap(),
                workspace: None,
                details: Map::new(),
            })
            .is_err()
        );
    }
}

#[test]
fn api_004_automation_error_details_reject_unknown_fields() {
    let cases = [
        ("DAEMON_UNAVAILABLE", 3, true, Map::new()),
        (
            "SESSION_REVISION_CONFLICT",
            4,
            true,
            Map::from_iter([("expected_revision".to_owned(), json!(1))]),
        ),
        (
            "ATTEMPT_NOT_CURRENT",
            4,
            true,
            Map::from_iter([(
                "expected_attempt_id".to_owned(),
                json!("00000000-0000-4000-8000-000000000019"),
            )]),
        ),
        (
            "IDEMPOTENCY_KEY_REUSED",
            2,
            false,
            Map::from_iter([("admission".to_owned(), json!({"admitted": false}))]),
        ),
        (
            "BLOCKER_LIMIT_REACHED",
            1,
            false,
            Map::from_iter([("maximum_open_blockers".to_owned(), json!(1024))]),
        ),
        ("JOB_WAIT_TIMEOUT", 4, true, Map::new()),
    ];
    for (code, exit_code, retryable, mut details) in cases {
        details.insert("unexpected".to_owned(), json!(true));
        assert!(matches!(
            ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
                request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
                command: CommandNameV1::new("status").unwrap(),
                generated_at: timestamp(),
                code: ErrorCodeV1::new(code).unwrap(),
                message: "closed detail test".to_owned(),
                retryable,
                exit_code: ExitCodeV1::new(exit_code).unwrap(),
                workspace: None,
                details,
            }),
            Err(ProtocolError::InvalidErrorDetails { .. })
        ));
    }
}

fn session_reset_not_eligible_error(
    details: Map<String, Value>,
) -> Result<ErrorEnvelopeV1, ProtocolError> {
    ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        command: CommandNameV1::new("session.reset").unwrap(),
        generated_at: timestamp(),
        code: ErrorCodeV1::new("SESSION_RESET_NOT_ELIGIBLE").unwrap(),
        message: "session reset is not eligible".to_owned(),
        retryable: false,
        exit_code: ExitCodeV1::new(1).unwrap(),
        workspace: None,
        details,
    })
}

#[test]
fn api_004_session_reset_not_eligible_details_are_closed_and_state_correlated() {
    let details = |lifecycle: &str, required_action: &str| {
        json!({
            "lifecycle": lifecycle,
            "current_terminal_disposition": false,
            "required_action": required_action,
            "admission": {"admitted": false}
        })
        .as_object()
        .unwrap()
        .clone()
    };
    for valid in [
        details("running", "force"),
        details("completed", "record_disposition"),
        details("cancelled", "record_disposition"),
    ] {
        assert_round_trip(session_reset_not_eligible_error(valid).unwrap());
    }

    let mut invalid = vec![
        details("running", "record_disposition"),
        details("completed", "force"),
        details("prepared", "force"),
    ];
    let mut current_disposition = details("running", "force");
    current_disposition.insert("current_terminal_disposition".to_owned(), json!(true));
    invalid.push(current_disposition);
    let mut malformed_admission = details("running", "force");
    malformed_admission.insert("admission".to_owned(), json!({"admitted": true}));
    invalid.push(malformed_admission);
    let mut missing = details("running", "force");
    missing.remove("lifecycle");
    invalid.push(missing);
    let mut extra = details("running", "force");
    extra.insert("unexpected".to_owned(), json!(true));
    invalid.push(extra);
    let mut wrong_schema = details("running", "force");
    wrong_schema.insert("schema".to_owned(), json!("podway.unknown/v1"));
    invalid.push(wrong_schema);

    for details in invalid {
        assert!(session_reset_not_eligible_error(details).is_err());
    }
}

fn mutation_outcome_unknown_details(idempotency_key: &str) -> Map<String, Value> {
    Map::from_iter([
        (
            "schema".to_owned(),
            json!("podway.mutation-outcome-unknown-details/v2"),
        ),
        ("outcome".to_owned(), json!("unknown")),
        ("idempotency_key".to_owned(), json!(idempotency_key)),
        (
            "reconcile".to_owned(),
            json!({
                "command": "job.lookup",
                "idempotency_key": idempotency_key,
            }),
        ),
    ])
}

fn mutation_outcome_unknown_error(
    details: Map<String, Value>,
) -> Result<ErrorEnvelopeV1, ProtocolError> {
    ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        command: CommandNameV1::new("session.start").unwrap(),
        generated_at: timestamp(),
        code: ErrorCodeV1::new("MUTATION_OUTCOME_UNKNOWN").unwrap(),
        message: "mutation outcome is unknown".to_owned(),
        retryable: true,
        exit_code: ExitCodeV1::new(4).unwrap(),
        workspace: None,
        details,
    })
}

#[test]
fn api_004_mutation_outcome_unknown_details_are_closed_and_reconcilable() {
    let valid = mutation_outcome_unknown_details("recon-key");
    assert_round_trip(mutation_outcome_unknown_error(valid.clone()).unwrap());

    let mut invalid_cases = Vec::new();
    for field in ["outcome", "idempotency_key", "reconcile"] {
        let mut details = valid.clone();
        details.remove(field);
        invalid_cases.push(details);
    }
    let mut extra = valid.clone();
    extra.insert("admission".to_owned(), json!({"admitted": false}));
    invalid_cases.push(extra);
    let mut wrong_schema = valid.clone();
    wrong_schema.insert("schema".to_owned(), json!("podway.unknown/v1"));
    invalid_cases.push(wrong_schema);
    let mut wrong_outcome = valid.clone();
    wrong_outcome.insert("outcome".to_owned(), json!("failed"));
    invalid_cases.push(wrong_outcome);
    let mut invalid_key = mutation_outcome_unknown_details("");
    invalid_key.insert("idempotency_key".to_owned(), json!(""));
    invalid_cases.push(invalid_key);
    let mut overlong_key = mutation_outcome_unknown_details(&"x".repeat(257));
    overlong_key.insert("idempotency_key".to_owned(), json!("x".repeat(257)));
    invalid_cases.push(overlong_key);
    let mut wrong_command = valid.clone();
    wrong_command["reconcile"]["command"] = json!("job.status");
    invalid_cases.push(wrong_command);
    let mut mismatched_key = valid.clone();
    mismatched_key["reconcile"]["idempotency_key"] = json!("different-key");
    invalid_cases.push(mismatched_key);
    let mut extra_reconcile = valid;
    extra_reconcile["reconcile"]["extra"] = json!(true);
    invalid_cases.push(extra_reconcile);

    for details in invalid_cases {
        assert_eq!(
            mutation_outcome_unknown_error(details),
            Err(ProtocolError::InvalidMutationOutcomeUnknownDetails)
        );
    }

    let mut missing_schema = serde_json::to_value(
        mutation_outcome_unknown_error(mutation_outcome_unknown_details("recon-key")).unwrap(),
    )
    .unwrap();
    missing_schema["details"]
        .as_object_mut()
        .unwrap()
        .remove("schema");
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(missing_schema).is_err());
}

fn identity_details(
    schema: &str,
    expected_key: &str,
    expected: &str,
    actual_key: &str,
    actual: Option<&str>,
    admitted: bool,
) -> Map<String, Value> {
    let admission = if admitted {
        json!({
            "admitted": true,
            "job_id": JOB_ID,
            "workspace_sequence": 7
        })
    } else {
        json!({"admitted": false})
    };
    Map::from_iter([
        ("schema".to_owned(), json!(schema)),
        (expected_key.to_owned(), json!(expected)),
        (
            actual_key.to_owned(),
            actual.map_or(Value::Null, |value| json!(value)),
        ),
        ("admission".to_owned(), admission),
    ])
}

#[test]
fn api_004_identity_conflict_details_are_closed_and_admission_aware() {
    for (code, exit_code, details) in [
        (
            "WORKSPACE_UUID_MISMATCH",
            4,
            identity_details(
                "podway.workspace-uuid-mismatch-details/v2",
                "expected_workspace_uuid",
                WORKSPACE_ID,
                "actual_workspace_uuid",
                Some("523e4567-e89b-12d3-a456-426614174000"),
                false,
            ),
        ),
        (
            "SESSION_ID_MISMATCH",
            4,
            identity_details(
                "podway.session-id-mismatch-details/v2",
                "expected_session_id",
                SESSION_ID,
                "actual_session_id",
                None,
                true,
            ),
        ),
    ] {
        let envelope = ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
            request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
            command: CommandNameV1::new("status").unwrap(),
            generated_at: timestamp(),
            code: ErrorCodeV1::new(code).unwrap(),
            message: "identity mismatch".to_owned(),
            retryable: false,
            exit_code: ExitCodeV1::new(exit_code).unwrap(),
            workspace: None,
            details: details.clone(),
        })
        .unwrap();
        assert_round_trip(envelope);

        let mut unknown = details.clone();
        unknown.insert("diagnostic".to_owned(), json!("forbidden"));
        let mut malformed = valid_error_envelope_json();
        malformed["code"] = json!(code);
        malformed["exit_code"] = json!(exit_code);
        malformed["retryable"] = json!(false);
        malformed["details"] = Value::Object(unknown);
        assert_error_rejected(malformed);

        let mut malformed = valid_error_envelope_json();
        malformed["code"] = json!(code);
        malformed["exit_code"] = json!(exit_code);
        malformed["retryable"] = json!(false);
        malformed["details"] = Value::Object(details.clone());
        malformed["details"]["admission"] = json!({"admitted": true});
        assert_error_rejected(malformed);
    }

    let mut wrong_schema = valid_error_envelope_json();
    wrong_schema["code"] = json!("SESSION_ID_MISMATCH");
    wrong_schema["exit_code"] = json!(4);
    wrong_schema["retryable"] = json!(false);
    wrong_schema["details"] = Value::Object(identity_details(
        "podway.session-id-mismatch-details/v2",
        "expected_session_id",
        SESSION_ID,
        "actual_session_id",
        Some("523e4567-e89b-12d3-a456-426614174000"),
        false,
    ));
    assert_error_rejected(wrong_schema);

    let workspace_details = identity_details(
        "podway.workspace-uuid-mismatch-details/v2",
        "expected_workspace_uuid",
        WORKSPACE_ID,
        "actual_workspace_uuid",
        Some("523e4567-e89b-12d3-a456-426614174000"),
        false,
    );
    for invalid_details in [
        {
            let mut details = workspace_details.clone();
            details.insert("actual_workspace_uuid".to_owned(), Value::Null);
            details
        },
        {
            let mut details = workspace_details.clone();
            details.insert("expected_workspace_uuid".to_owned(), json!("not-a-uuid"));
            details
        },
        {
            let mut details = workspace_details.clone();
            details.insert(
                "admission".to_owned(),
                json!({"admitted": false, "job_id": JOB_ID}),
            );
            details
        },
        {
            let mut details = workspace_details.clone();
            details.insert(
                "admission".to_owned(),
                json!({
                    "admitted": true,
                    "job_id": JOB_ID,
                    "workspace_sequence": 0
                }),
            );
            details
        },
    ] {
        let mut malformed = valid_error_envelope_json();
        malformed["code"] = json!("WORKSPACE_UUID_MISMATCH");
        malformed["exit_code"] = json!(4);
        malformed["retryable"] = json!(false);
        malformed["details"] = Value::Object(invalid_details);
        assert_error_rejected(malformed);
    }
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
    assert!(serde_json::from_value::<OutputEnvelopeV3>(value.clone()).is_err());
    assert!(serde_json::from_value::<ResponseEnvelopeV2>(value).is_err());
}

fn assert_error_rejected(value: Value) {
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(value.clone()).is_err());
    assert!(serde_json::from_value::<ResponseEnvelopeV2>(value).is_err());
}

// API-004: The public response wire surface is serializable, deserializable, and schema-stable.
#[test]
fn api_004_response_wire_surface_is_executable_and_round_trips() {
    assert_wire_type::<WorkspaceOutputV1>();
    assert_wire_type::<JobOutputV1>();
    assert_wire_type::<SessionOutputV1>();
    assert_wire_type::<OutputEnvelopeV3>();
    assert_wire_type::<ErrorCodeV1>();
    assert_wire_type::<ExitCodeV1>();
    assert_wire_type::<ErrorEnvelopeV1>();
    assert_wire_type::<ResponseEnvelopeV2>();

    let output = valid_output_envelope();
    let output_value = serde_json::to_value(&output).expect("output fixture must serialize");
    assert_eq!(output_value["schema"], OUTPUT_SCHEMA_V3);
    assert_eq!(output_value["command"], "workspace.show");
    assert_round_trip(output.clone());
    assert_round_trip(ResponseEnvelopeV2::OutputV2(output));

    let error = valid_error_envelope();
    let error_value = serde_json::to_value(&error).expect("error fixture must serialize");
    assert_eq!(error_value["schema"], ERROR_SCHEMA_V1);
    assert_eq!(error_value["command"], "session.status");
    assert_round_trip(error.clone());
    assert_round_trip(ResponseEnvelopeV2::Error(error));
}

#[test]
fn api_004_admission_metadata_is_closed_and_matches_the_output_job() {
    let admitted = json!({
        "admitted": true,
        "job_id": JOB_ID,
        "workspace_sequence": 1,
    });
    let mut output = valid_output_envelope_json();
    output["result"]["admission"] = admitted.clone();
    assert!(serde_json::from_value::<OutputEnvelopeV3>(output.clone()).is_ok());

    for malformed in [
        json!({"admitted": true}),
        json!({"admitted": false}),
        json!({
            "admitted": true,
            "job_id": "523e4567-e89b-12d3-a456-426614174000",
            "workspace_sequence": 1,
        }),
        json!({
            "admitted": true,
            "job_id": JOB_ID,
            "workspace_sequence": 2,
        }),
        json!({
            "admitted": true,
            "job_id": JOB_ID,
            "workspace_sequence": 1,
            "unknown": true,
        }),
    ] {
        let mut malformed_output = output.clone();
        malformed_output["result"]["admission"] = malformed;
        assert_output_rejected(malformed_output);
    }

    for admission in [json!({"admitted": false}), admitted] {
        let mut error = valid_error_envelope_json();
        let is_admitted = admission["admitted"] == json!(true);
        error["details"]["admission"] = admission;
        if is_admitted {
            error["details"]["job_id"] = json!(JOB_ID);
            error["details"]["job_sequence"] = json!(1);
        }
        assert!(serde_json::from_value::<ErrorEnvelopeV1>(error).is_ok());
    }

    for malformed in [
        json!({"admitted": false, "job_id": JOB_ID}),
        json!({"admitted": true, "job_id": JOB_ID, "workspace_sequence": 0}),
    ] {
        let mut error = valid_error_envelope_json();
        error["details"]["admission"] = malformed;
        assert_error_rejected(error);
    }
}

// API-004: Validated response DTOs reject invariant-bypassing known and nested wire payloads.
#[test]
fn api_004_response_deserialization_rejects_invalid_known_and_nested_values() {
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
    let mut terminal_null_finished_at = valid_output_envelope_json();
    terminal_null_finished_at["job"]["finished_at"] = Value::Null;
    assert!(
        serde_json::from_value::<JobOutputV1>(terminal_null_finished_at["job"].clone()).is_err()
    );
    assert_output_rejected(terminal_null_finished_at);

    let mut nonterminal_finished_at = valid_output_envelope_json();
    nonterminal_finished_at["job"]["state"] = json!("queued");
    assert!(serde_json::from_value::<JobOutputV1>(nonterminal_finished_at["job"].clone()).is_err());
    assert_output_rejected(nonterminal_finished_at);

    let mut oversized_session_title = valid_output_envelope_json();
    oversized_session_title["session"]["title"] = json!("x".repeat(501));
    assert_output_rejected(oversized_session_title);

    let mut null_output_workspace = valid_output_envelope_json();
    null_output_workspace["workspace"] = Value::Null;
    assert_output_rejected(null_output_workspace);

    for field in ["workspace", "job", "session"] {
        let mut additive_nested_output = valid_output_envelope_json();
        additive_nested_output[field]["future_extension"] = json!({"enabled": true});
        assert!(
            serde_json::from_value::<ResponseEnvelopeV2>(additive_nested_output).is_ok(),
            "additive {field} DTO field must decode"
        );
    }

    let output = valid_output_envelope();
    let mut additive_output = serde_json::to_value(&output).expect("output fixture must serialize");
    additive_output["unknown"] = json!({"extension": ["supported", true]});
    assert_eq!(
        decode_response_payload_v2(
            &serde_json::to_vec(&additive_output).expect("additive output must serialize")
        )
        .expect("additive output must decode"),
        ResponseEnvelopeV2::OutputV2(output)
    );

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

    let error = valid_error_envelope();
    let mut additive_error = serde_json::to_value(&error).expect("error fixture must serialize");
    additive_error["unknown"] = json!({"extension": ["supported", true]});
    assert_eq!(
        decode_response_payload_v2(
            &serde_json::to_vec(&additive_error).expect("additive error must serialize")
        )
        .expect("additive error must decode"),
        ResponseEnvelopeV2::Error(error)
    );
}
#[test]
fn api_004_job_finished_at_state_invariant_is_typed_and_compatible() {
    let job_id = JobId::new(JOB_ID).expect("job id fixture must be valid");

    for state in [
        JobStateV1::Succeeded,
        JobStateV1::Failed,
        JobStateV1::Cancelled,
    ] {
        assert_eq!(
            JobOutputV1::new(job_id.clone(), 1, state, timestamp(), None, None),
            Err(ProtocolError::TerminalJobMissingFinishedAt)
        );
        assert!(
            JobOutputV1::new(
                job_id.clone(),
                1,
                state,
                timestamp(),
                Some(timestamp()),
                Some(timestamp()),
            )
            .is_ok()
        );
    }

    for state in [JobStateV1::Queued, JobStateV1::Running] {
        assert_eq!(
            JobOutputV1::new(
                job_id.clone(),
                1,
                state,
                timestamp(),
                Some(timestamp()),
                Some(timestamp()),
            ),
            Err(ProtocolError::NonterminalJobHasFinishedAt)
        );
        assert!(JobOutputV1::new(job_id.clone(), 1, state, timestamp(), None, None).is_ok());
    }
}

#[test]
fn api_004_timestamp_calendar_and_clock_ranges_are_exact() {
    for value in [
        "2024-02-29T00:00:00.000Z",
        "2000-02-29T23:59:59.999Z",
        "0000-02-29T12:34:56.789Z",
    ] {
        assert!(Rfc3339MillisV1::new(value).is_ok(), "{value} must be valid");
    }
    for value in [
        "2023-02-29T00:00:00.000Z",
        "1900-02-29T00:00:00.000Z",
        "2026-04-31T00:00:00.000Z",
        "2026-00-01T00:00:00.000Z",
        "2026-01-00T00:00:00.000Z",
        "2026-01-01T24:00:00.000Z",
        "2026-01-01T23:60:00.000Z",
        "2026-01-01T23:59:60.000Z",
    ] {
        assert!(
            Rfc3339MillisV1::new(value).is_err(),
            "{value} must be invalid"
        );
    }
}

fn nested_arrays(count: usize) -> Value {
    let mut value = Value::Null;
    for _ in 0..count {
        value = Value::Array(vec![value]);
    }
    value
}
fn response_with_additive_depth(mut response: Value, path: &[&str], array_count: usize) -> Value {
    let mut target = response
        .as_object_mut()
        .expect("response fixture must be an object");
    for field in path {
        target = target
            .get_mut(*field)
            .and_then(Value::as_object_mut)
            .expect("response nested DTO must be an object");
    }
    target.insert("future_extension".to_owned(), nested_arrays(array_count));
    response
}
fn assert_response_decodes(value: Value) {
    match value["schema"].as_str() {
        Some(OUTPUT_SCHEMA_V3) => {
            assert!(serde_json::from_value::<OutputEnvelopeV3>(value.clone()).is_ok());
        }
        Some(ERROR_SCHEMA_V1) => {
            assert!(serde_json::from_value::<ErrorEnvelopeV1>(value.clone()).is_ok());
        }
        _ => panic!("response depth fixture must have a supported schema"),
    }
    assert!(serde_json::from_value::<ResponseEnvelopeV2>(value.clone()).is_ok());
    assert!(
        decode_response_payload_v2(
            &serde_json::to_vec(&value).expect("response depth fixture must serialize")
        )
        .is_ok()
    );
}

fn assert_response_depth_rejected(value: Value) {
    match value["schema"].as_str() {
        Some(OUTPUT_SCHEMA_V3) => {
            assert!(serde_json::from_value::<OutputEnvelopeV3>(value.clone()).is_err());
        }
        Some(ERROR_SCHEMA_V1) => {
            assert!(serde_json::from_value::<ErrorEnvelopeV1>(value.clone()).is_err());
        }
        _ => panic!("response depth fixture must have a supported schema"),
    }
    assert!(serde_json::from_value::<ResponseEnvelopeV2>(value.clone()).is_err());
    assert!(
        decode_response_payload_v2(
            &serde_json::to_vec(&value).expect("response depth fixture must serialize")
        )
        .is_err()
    );
}

#[test]
fn api_004_response_additive_fields_enforce_full_document_depth() {
    for (_, response) in [
        ("output", valid_output_envelope_json()),
        ("error", valid_error_envelope_json()),
    ] {
        let at_limit = response_with_additive_depth(response.clone(), &[], MAX_JSON_DEPTH_V1 - 1);
        assert_response_decodes(at_limit);

        let over_limit = response_with_additive_depth(response, &[], MAX_JSON_DEPTH_V1);
        assert_response_depth_rejected(over_limit);
    }

    for field in ["workspace", "job", "session"] {
        let at_limit = response_with_additive_depth(
            valid_output_envelope_json(),
            &[field],
            MAX_JSON_DEPTH_V1 - 2,
        );
        assert_response_decodes(at_limit);

        let over_limit = response_with_additive_depth(
            valid_output_envelope_json(),
            &[field],
            MAX_JSON_DEPTH_V1 - 1,
        );
        assert_response_depth_rejected(over_limit);
    }
    let at_limit = response_with_additive_depth(
        valid_error_envelope_json(),
        &["workspace"],
        MAX_JSON_DEPTH_V1 - 2,
    );
    assert_response_decodes(at_limit);

    let over_limit = response_with_additive_depth(
        valid_error_envelope_json(),
        &["workspace"],
        MAX_JSON_DEPTH_V1 - 1,
    );
    assert_response_depth_rejected(over_limit);
}

#[test]
fn api_004_component_depth_matches_the_full_envelope_boundary() {
    let mut result = Map::new();
    result.insert("nested".to_owned(), nested_arrays(MAX_JSON_DEPTH_V1 - 2));
    let output = OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        command: CommandNameV1::new("workspace.show").unwrap(),
        generated_at: timestamp(),
        workspace: None,
        job: None,
        session: None,
        result,
        warnings: Vec::new(),
    })
    .expect("maximum output component depth must encode");
    let encoded_output = encode_response_payload_v2(&ResponseEnvelopeV2::OutputV2(output))
        .expect("maximum output depth must encode");
    assert!(decode_response_payload_v2(&encoded_output).is_ok());

    let mut details = Map::new();
    details.insert("nested".to_owned(), nested_arrays(MAX_JSON_DEPTH_V1 - 2));
    let error = ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        command: CommandNameV1::new("session.status").unwrap(),
        generated_at: timestamp(),
        code: ErrorCodeV1::new("INTERNAL_ERROR").unwrap(),
        message: "failed".to_owned(),
        retryable: false,
        exit_code: ExitCodeV1::new(6).unwrap(),
        workspace: None,
        details,
    })
    .expect("maximum error component depth must encode");
    let encoded_error = encode_response_payload_v2(&ResponseEnvelopeV2::Error(error))
        .expect("maximum error depth must encode");
    assert!(decode_response_payload_v2(&encoded_error).is_ok());

    let mut payload = Map::new();
    payload.insert("nested".to_owned(), nested_arrays(MAX_JSON_DEPTH_V1 - 2));
    let request = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        client: ClientInfoV1::new("podway-cli", "0.1.0", 1).unwrap(),
        operation: OperationV1::Query,
        command: CommandNameV1::new("status").unwrap(),
        workspace: None,
        idempotency_key: None,
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).unwrap(),
        payload,
    })
    .expect("maximum request component depth must encode");
    let encoded_request =
        encode_request_payload_v1(&request).expect("maximum request depth must encode");
    assert!(decode_request_payload_v1(&encoded_request).is_ok());

    let mut too_deep_result = Map::new();
    too_deep_result.insert("nested".to_owned(), nested_arrays(MAX_JSON_DEPTH_V1 - 1));
    assert!(
        OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
            request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
            command: CommandNameV1::new("session.status").unwrap(),
            generated_at: timestamp(),
            workspace: None,
            job: None,
            session: None,
            result: too_deep_result,
            warnings: Vec::new(),
        })
        .is_err()
    );
}
