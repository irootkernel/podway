use podway_protocol::{
    MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1, PayloadCodecErrorV1, ProtocolError, ResponseEnvelopeV2,
    decode_response_payload_v2, encode_response_payload_v2,
};
use serde_json::{Value, json};

fn diagnostics_output_v2() -> Value {
    json!({
        "schema": "podway.output/v3",
        "request_id": "00000000-0000-4000-8000-000000000001",
        "command": "procedure.validate",
        "generated_at": "2026-08-09T00:00:00.000Z",
        "result": {
            "schema": "podway.procedure-diagnostics-result/v1",
            "operation": "validate",
            "procedure_schema": "podway.procedure/v2",
            "file": "procedure.yaml",
            "valid": true,
            "diagnostics": [],
            "diagnostics_truncated": false,
            "diagnostics_total": 0
        },
        "warnings": []
    })
}

fn legacy_output_v1() -> Value {
    json!({
        "schema": "podway.output/v1",
        "request_id": "00000000-0000-4000-8000-000000000001",
        "command": "status",
        "generated_at": "2026-08-09T00:00:00.000Z",
        "result": {},
        "warnings": []
    })
}

fn shared_mutation_output_v2(command: &str, result_schema: &str) -> Value {
    let mut fixtures: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/protocol/result-families.json"
    ))
    .unwrap();
    let result = fixtures["fixtures"][result_schema].take();
    json!({
        "schema": "podway.output/v3",
        "request_id": "00000000-0000-4000-8000-000000000001",
        "command": command,
        "generated_at": "2026-08-09T00:00:00.000Z",
        "workspace": {
            "uuid": "00000000-0000-4000-8000-000000000002",
            "root": "/worktree",
            "latest_workspace_sequence": 1
        },
        "job": {
            "id": "00000000-0000-4000-8000-000000000001",
            "sequence": 1,
            "state": "succeeded",
            "submitted_at": "2026-08-09T00:00:00.000Z",
            "claimed_at": "2026-08-09T00:00:00.000Z",
            "finished_at": "2026-08-09T00:00:00.000Z"
        },
        "result": result,
        "warnings": []
    })
}

fn reset_dry_run_output_v2() -> Value {
    json!({
        "schema": "podway.output/v3",
        "request_id": "00000000-0000-4000-8000-000000000001",
        "command": "session.reset",
        "generated_at": "2026-08-09T00:00:00.000Z",
        "workspace": {
            "uuid": "00000000-0000-4000-8000-000000000002",
            "root": "/worktree",
            "latest_workspace_sequence": 1
        },
        "result": {
            "schema": "podway.stage-transition-result/v2",
            "admission": {"admitted": false},
            "transition": "reset",
            "reset": true,
            "revision": 1
        },
        "warnings": []
    })
}

#[test]
fn v2plt006_v2_aware_response_codec_round_trips_output_v2() {
    let expected = diagnostics_output_v2();
    let input = serde_json::to_vec(&expected).unwrap();

    let decoded = decode_response_payload_v2(&input).expect("v2 output must decode");
    assert!(matches!(decoded, ResponseEnvelopeV2::OutputV2(_)));

    let encoded = encode_response_payload_v2(&decoded).expect("v2 output must encode");
    assert_eq!(serde_json::from_slice::<Value>(&encoded).unwrap(), expected);
}

#[test]
fn v2run006_reset_dry_run_round_trips_only_without_a_job() {
    let expected = reset_dry_run_output_v2();
    let decoded = decode_response_payload_v2(&serde_json::to_vec(&expected).unwrap())
        .expect("a non-admitted reset dry-run without a job must decode");
    assert_eq!(
        serde_json::from_slice::<Value>(&encode_response_payload_v2(&decoded).unwrap()).unwrap(),
        expected
    );

    let mut false_with_job = reset_dry_run_output_v2();
    false_with_job["job"] =
        shared_mutation_output_v2("session.reset", "podway.stage-transition-result/v2")["job"]
            .take();
    assert!(decode_response_payload_v2(&serde_json::to_vec(&false_with_job).unwrap()).is_err());

    let mut true_without_job = reset_dry_run_output_v2();
    true_without_job["result"]["admission"] = json!({
        "admitted": true,
        "job_id": "00000000-0000-4000-8000-000000000001",
        "workspace_sequence": 1
    });
    assert!(decode_response_payload_v2(&serde_json::to_vec(&true_without_job).unwrap()).is_err());
}

#[test]
fn v2run006_blocker_limit_details_accept_only_the_v2_limit() {
    let error = json!({
        "schema": "podway.error/v1",
        "request_id": "00000000-0000-4000-8000-000000000001",
        "command": "session.block",
        "generated_at": "2026-08-09T00:00:00.000Z",
        "code": "BLOCKER_LIMIT_REACHED",
        "message": "The active attempt reached the open blocker limit.",
        "retryable": false,
        "exit_code": 1,
        "details": {
            "schema": "podway.blocker-limit-details/v1",
            "maximum_open_blockers": 64
        }
    });
    assert!(decode_response_payload_v2(&serde_json::to_vec(&error).unwrap()).is_ok());

    let mut invalid = json!({
        "schema": "podway.error/v1",
        "request_id": "00000000-0000-4000-8000-000000000001",
        "command": "session.block",
        "generated_at": "2026-08-09T00:00:00.000Z",
        "code": "BLOCKER_LIMIT_REACHED",
        "message": "The active attempt reached the open blocker limit.",
        "retryable": false,
        "exit_code": 1,
        "details": {
            "schema": "podway.blocker-limit-details/v1",
            "maximum_open_blockers": 65
        }
    });
    assert!(decode_response_payload_v2(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid["details"]["maximum_open_blockers"] = json!(1_024);
    assert!(decode_response_payload_v2(&serde_json::to_vec(&invalid).unwrap()).is_err());
}

#[test]
fn v2run003_shared_mutation_commands_select_output_v2_result_families() {
    for (command, result_schema) in [
        ("item.set", "podway.item-mutation-result/v2"),
        ("session.complete", "podway.stage-transition-result/v2"),
    ] {
        let expected = shared_mutation_output_v2(command, result_schema);
        let decoded = decode_response_payload_v2(&serde_json::to_vec(&expected).unwrap())
            .unwrap_or_else(|error| panic!("{command} output/v3 must decode: {error}"));
        let ResponseEnvelopeV2::OutputV2(output) = &decoded else {
            panic!("{command} must select output/v3")
        };
        assert_eq!(output.command().as_str(), command);
        assert_eq!(output.result()["schema"], result_schema);
        assert_eq!(
            serde_json::from_slice::<Value>(&encode_response_payload_v2(&decoded).unwrap())
                .unwrap(),
            expected
        );
    }
}

#[test]
fn v2_only_codec_rejects_released_v1_output() {
    let input = serde_json::to_vec(&legacy_output_v1()).unwrap();
    assert!(matches!(
        decode_response_payload_v2(&input),
        Err(PayloadCodecErrorV1::UnsupportedResponseSchema { received, supported })
            if received == "podway.output/v1"
                && supported == ["podway.output/v3", "podway.error/v1"]
    ));
}

#[test]
fn v2plt006_v2_aware_codec_round_trips_released_error_v1() {
    let expected = json!({
        "schema": "podway.error/v1",
        "request_id": "00000000-0000-4000-8000-000000000001",
        "command": "session.status",
        "generated_at": "2026-08-09T00:00:00.000Z",
        "code": "GRAPH_NODE_NOT_FOUND",
        "message": "Graph node not found.",
        "retryable": false,
        "exit_code": 1,
        "details": {
            "schema": "podway.v2-runtime-error-details/v1",
            "kind": "GRAPH_NODE_NOT_FOUND",
            "graph_node_id": "work"
        }
    });
    let decoded = decode_response_payload_v2(&serde_json::to_vec(&expected).unwrap())
        .expect("error/v1 must remain compatible");
    assert!(matches!(decoded, ResponseEnvelopeV2::Error(_)));
    let encoded = encode_response_payload_v2(&decoded).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&encoded).unwrap(), expected);
}

#[test]
fn v2plt006_v2_aware_decoder_keeps_schema_dispatch_closed() {
    let mut value = diagnostics_output_v2();
    value["schema"] = json!("podway.output/v2");
    let input = serde_json::to_vec(&value).unwrap();

    assert!(matches!(
        decode_response_payload_v2(&input),
        Err(PayloadCodecErrorV1::UnsupportedResponseSchema { received, supported })
            if received == "podway.output/v2"
                && supported == ["podway.output/v3", "podway.error/v1"]
    ));
}

#[test]
fn v2plt006_compact_v2_decode_counts_the_json_newline() {
    let mut value = diagnostics_output_v2();
    value["command"] = json!("session.status");
    value["result"] = json!({"schema": "podway.compact-status-result/v2"});
    value["future_padding"] = json!("");
    let base_length = serde_json::to_vec(&value).unwrap().len() + 1;
    value["future_padding"] =
        json!("x".repeat(MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1 - base_length + 1));
    let input = serde_json::to_vec(&value).unwrap();

    assert!(matches!(
        decode_response_payload_v2(&input),
        Err(PayloadCodecErrorV1::JsonContract(
            ProtocolError::CompactStatusEnvelopeTooLarge { length, maximum }
        )) if length == MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1 + 1
            && maximum == MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1
    ));
}
