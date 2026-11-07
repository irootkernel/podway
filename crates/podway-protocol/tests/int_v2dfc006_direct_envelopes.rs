use podway_protocol::{
    CommandNameV1, ErrorCodeV1, ErrorEnvelopeInputV2, ErrorEnvelopeV2, ExitCodeV1,
    OutputEnvelopeInputV4, OutputEnvelopeV4, RecoveryRecipeV2, RequestIdV1, Rfc3339MillisV1,
    WorkspaceOutputV1,
};
use serde_json::{Value, json};

fn answers() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/protocol/current-contracts.json"
    ))
    .unwrap()
}

#[test]
fn parse003_output_constructor_moves_result_and_warning_trees() {
    let fixture = &answers()["fixtures"]["podway.output/v4"];
    let input = OutputEnvelopeInputV4 {
        request_id: RequestIdV1::new(fixture["request_id"].as_str().unwrap()).unwrap(),
        command: CommandNameV1::new("procedure.show").unwrap(),
        generated_at: Rfc3339MillisV1::new(fixture["generated_at"].as_str().unwrap()).unwrap(),
        workspace: None,
        session: None,
        result: serde_json::Map::from_iter([(
            "nested".to_owned(),
            json!({"values": ["payload".repeat(64)]}),
        )]),
        warnings: ["First warning", "Second warning"]
            .into_iter()
            .map(|message| {
                json!({"code":"ADVISORY", "path":"procedure", "message":message.repeat(4)})
                    .as_object()
                    .unwrap()
                    .clone()
            })
            .collect(),
    };
    let result_pointer = input.result["nested"]["values"][0]
        .as_str()
        .unwrap()
        .as_ptr();
    let warning_pointers: Vec<_> = input
        .warnings
        .iter()
        .map(|warning| warning["message"].as_str().unwrap().as_ptr())
        .collect();
    let mut expected = serde_json::to_value(&input).unwrap();
    expected["schema"] = json!("podway.output/v4");

    let output = OutputEnvelopeV4::new(input).unwrap();
    assert_eq!(output.as_value(), &expected);
    assert_eq!(
        serde_json::to_vec(&output).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    assert_eq!(
        output.as_value()["result"]["nested"]["values"][0]
            .as_str()
            .unwrap()
            .as_ptr(),
        result_pointer
    );
    for (index, pointer) in warning_pointers.into_iter().enumerate() {
        assert_eq!(
            output.as_value()["warnings"][index]["message"]
                .as_str()
                .unwrap()
                .as_ptr(),
            pointer
        );
    }
}

#[test]
fn direct_constructors_and_decoders_validate_the_complete_envelope() {
    let fixtures = &answers()["fixtures"];
    let success = &fixtures["podway.output/v4"];
    let output = OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
        request_id: RequestIdV1::new(success["request_id"].as_str().unwrap()).unwrap(),
        command: CommandNameV1::new("item.set").unwrap(),
        generated_at: Rfc3339MillisV1::new(success["generated_at"].as_str().unwrap()).unwrap(),
        workspace: Some(
            serde_json::from_value::<WorkspaceOutputV1>(success["workspace"].clone()).unwrap(),
        ),
        session: None,
        result: success["result"].as_object().unwrap().clone(),
        warnings: Vec::new(),
    })
    .unwrap();
    assert_eq!(output.as_value(), success);
    assert_eq!(
        serde_json::from_value::<OutputEnvelopeV4>(success.clone()).unwrap(),
        output
    );
    let error = &fixtures["podway.error/v2"];
    let failure = ErrorEnvelopeV2::new(ErrorEnvelopeInputV2 {
        request_id: RequestIdV1::new(error["request_id"].as_str().unwrap()).unwrap(),
        command: CommandNameV1::new("item.set").unwrap(),
        generated_at: Rfc3339MillisV1::new(error["generated_at"].as_str().unwrap()).unwrap(),
        code: ErrorCodeV1::new("WORKSPACE_BUSY").unwrap(),
        message: error["message"].as_str().unwrap().to_owned(),
        retryable: true,
        exit_code: ExitCodeV1::new(4).unwrap(),
        workspace: None,
        details: error["details"].as_object().unwrap().clone(),
    })
    .unwrap();
    assert_eq!(failure.as_value(), error);
    assert_eq!(
        serde_json::from_value::<ErrorEnvelopeV2>(error.clone()).unwrap(),
        failure
    );
    for (field, invalid) in [
        ("schema", json!("podway.output/v3")),
        ("command", json!("daemon.status")),
        ("request_id", json!("not-a-uuid")),
        ("generated_at", json!("2026-10-06T00:00:00Z")),
        ("job", json!({})),
    ] {
        let mut candidate = success.clone();
        candidate[field] = invalid;
        assert!(
            serde_json::from_value::<OutputEnvelopeV4>(candidate).is_err(),
            "{field}"
        );
    }
    let mut candidate = success.clone();
    candidate["result"]["item_id"] = Value::Null;
    assert!(OutputEnvelopeV4::from_value(candidate).is_err());
    for (field, invalid) in [
        ("schema", json!("podway.error/v1")),
        ("code", json!("DAEMON_UNAVAILABLE")),
        ("retryable", json!(false)),
        ("exit_code", json!(3)),
        ("job", json!({})),
    ] {
        let mut candidate = error.clone();
        candidate[field] = invalid;
        assert!(
            serde_json::from_value::<ErrorEnvelopeV2>(candidate).is_err(),
            "{field}"
        );
    }
    let mut candidate = error.clone();
    for boundary in [
        "sqlite_write_lock",
        "connection_gate",
        "sqlite_read_admission",
    ] {
        candidate["details"]["boundary"] = json!(boundary);
        let value = ErrorEnvelopeV2::from_value(candidate.clone()).unwrap();
        assert_eq!(
            serde_json::from_value::<ErrorEnvelopeV2>(candidate.clone()).unwrap(),
            value
        );
    }
    candidate["details"]["boundary"] = json!("unknown_lock");
    assert!(ErrorEnvelopeV2::from_value(candidate.clone()).is_err());
    assert!(serde_json::from_value::<ErrorEnvelopeV2>(candidate).is_err());
    let mut candidate = error.clone();
    candidate["details"]["recovery"]["argv"] = json!(["podway", "--json", "job", "lookup"]);
    assert!(ErrorEnvelopeV2::from_value(candidate).is_err());
}

#[test]
fn direct_recovery_preserves_destructive_authority_and_original_lookup_key() {
    let fixture = answers()["fixtures"]["podway.recovery-recipe/v2"].clone();
    let recipe = serde_json::from_value::<RecoveryRecipeV2>(fixture.clone()).unwrap();
    assert_eq!(recipe.as_value(), &fixture);
    for argv in [
        json!(["podway", "--json", "job", "lookup"]),
        json!(["podway", "--json", "reset", "--all", "--yes"]),
    ] {
        let mut candidate = fixture.clone();
        candidate["argv"] = argv;
        assert!(RecoveryRecipeV2::from_value(candidate).is_err());
    }
    let reset = json!({"action":"reset_legacy_state","command":"workspace.reset_all","argv":["podway","--json","reset","--all","--yes","--idempotency-key","<new-idempotency-key>"],"reason":"Reset legacy state explicitly.","requires_explicit_authorization":true});
    assert!(RecoveryRecipeV2::from_value(reset.clone()).is_ok());
    let mut candidate = reset;
    candidate["requires_explicit_authorization"] = json!(false);
    assert!(RecoveryRecipeV2::from_value(candidate).is_err());
}

#[test]
fn agent001_mutation_observation_output_envelope_and_lookup_headroom_boundaries() {
    use podway_protocol::{
        MAX_DIRECT_OUTPUT_BYTES_V1, ProtocolError, serialized_json_len_v1,
        validate_daemon_free_document_v1, validate_direct_outcome_lookup_headroom_v1,
        validate_mutation_observation_command_result_v1,
        validate_mutation_observation_output_envelope_v1,
    };

    let fixtures = &answers()["fixtures"];
    let wrapper_result = fixtures["podway.mutation-observation-result/v1"].clone();
    let valid_envelope = json!({
        "schema": "podway.output/v4",
        "request_id": "00000000-0000-4000-8000-000000000001",
        "command": "item.record_many",
        "generated_at": "2026-10-06T00:00:00.000Z",
        "workspace": {
            "uuid": "00000000-0000-4000-8000-000000000001",
            "root": "/tmp/podway-example",
            "latest_workspace_sequence": 2,
        },
        "session": {
            "id": "00000000-0000-4000-8000-000000000002",
            "title": "Ship AGENT-001",
            "lifecycle": "running",
            "revision_before": 1,
            "revision_after": 2,
        },
        "result": wrapper_result,
        "warnings": [],
    });
    assert!(validate_mutation_observation_output_envelope_v1(&valid_envelope).is_ok());

    // Mismatched outer workspace.latest_workspace_sequence vs result.workspace_sequence fails closed.
    let mut mismatched_seq = valid_envelope.clone();
    mismatched_seq["workspace"]["latest_workspace_sequence"] = json!(3);
    assert!(validate_mutation_observation_output_envelope_v1(&mismatched_seq).is_err());

    // Missing session or workspace fails closed.
    let mut no_session = valid_envelope.clone();
    no_session.as_object_mut().unwrap().remove("session");
    assert!(validate_mutation_observation_output_envelope_v1(&no_session).is_err());
    let mut no_workspace = valid_envelope.clone();
    no_workspace.as_object_mut().unwrap().remove("workspace");
    assert!(validate_mutation_observation_output_envelope_v1(&no_workspace).is_err());

    // Extra top-level field, non-v4 schema, unsupported command, and malformed header/workspace fail closed.
    let mut extra_top = valid_envelope.clone();
    extra_top["extra"] = json!(true);
    assert!(validate_mutation_observation_output_envelope_v1(&extra_top).is_err());

    let mut bad_schema = valid_envelope.clone();
    bad_schema["schema"] = json!("podway.output/v3");
    assert!(validate_mutation_observation_output_envelope_v1(&bad_schema).is_err());

    for unsupported_cmd in [
        "session.skip",
        "session.start_replace",
        "goal.assess_criterion",
    ] {
        let mut bad_cmd = valid_envelope.clone();
        bad_cmd["command"] = json!(unsupported_cmd);
        assert!(validate_mutation_observation_output_envelope_v1(&bad_cmd).is_err());
    }

    let mut bad_req_id = valid_envelope.clone();
    bad_req_id["request_id"] = json!("not-a-uuid");
    assert!(validate_mutation_observation_output_envelope_v1(&bad_req_id).is_err());

    let mut bad_gen_at = valid_envelope.clone();
    bad_gen_at["generated_at"] = json!("2026-10-06T00:00:00Z");
    assert!(validate_mutation_observation_output_envelope_v1(&bad_gen_at).is_err());

    let mut bad_ws_uuid = valid_envelope.clone();
    bad_ws_uuid["workspace"]["uuid"] = json!("not-a-uuid");
    assert!(validate_mutation_observation_output_envelope_v1(&bad_ws_uuid).is_err());

    let mut bad_ws_root = valid_envelope.clone();
    bad_ws_root["workspace"]["root"] = json!("relative/path");
    assert!(validate_mutation_observation_output_envelope_v1(&bad_ws_root).is_err());

    let mut extra_ws_field = valid_envelope.clone();
    extra_ws_field["workspace"]["extra"] = json!(1);
    assert!(validate_mutation_observation_output_envelope_v1(&extra_ws_field).is_err());

    // Envelope-level lookup headroom rejection: valid mutation_result, valid observation v6
    // (schema + correlations), valid command wrapper, and envelope <= MAX_DIRECT_OUTPUT_BYTES_V1,
    // failing specifically on outcome.lookup wrapper headroom.
    assert!(OutputEnvelopeV4::from_value(valid_envelope.clone()).is_ok());

    let empty_stored = json!({"pad": ""});
    let max_root = format!("/{}", "\u{001f}".repeat(4095));
    let max_key = "\u{001f}".repeat(256);
    let base_wrapper = json!({
        "schema": "podway.output/v4",
        "request_id": "ffffffff-ffff-4fff-bfff-ffffffffffff",
        "command": "outcome.lookup",
        "generated_at": "9999-12-31T23:59:59.999Z",
        "workspace": {
            "uuid": "ffffffff-ffff-4fff-bfff-ffffffffffff",
            "root": max_root,
            "latest_workspace_sequence": u64::MAX,
        },
        "result": {
            "schema": "podway.outcome-lookup-result/v1",
            "idempotency_key": max_key,
            "found": true,
            "outcome": {
                "request_digest": "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "recorded_at": "9999-12-31T23:59:59.999Z",
                "scope": "workspace",
                "response": empty_stored,
            },
        },
        "warnings": [],
    });
    let base_wrapper_bytes = serialized_json_len_v1(&base_wrapper).unwrap();
    let empty_stored_bytes = serialized_json_len_v1(&empty_stored).unwrap();
    let lookup_overhead_bytes = base_wrapper_bytes - empty_stored_bytes;
    let max_stored_for_lookup = MAX_DIRECT_OUTPUT_BYTES_V1 - lookup_overhead_bytes;

    // Build a complete, schema- and correlation-legal OutputEnvelopeV4 mutation-observation
    // wrapper with exact target serialized UTF-8 byte length using Unicode control escaping
    // (\u001f -> 6 JSON bytes per scalar) and 1-byte ASCII adjustment.
    let build_legal_envelope_of_bytes = |target_bytes: usize| -> Value {
        let mut candidate = valid_envelope.clone();
        let full_block = "\u{001f}".repeat(4000);
        let mut probe_argv = vec![json!("podway"), json!("begin")];
        for _ in 0..41 {
            probe_argv.push(json!(full_block));
        }
        probe_argv.push(json!("a"));
        probe_argv.extend([
            json!("--if-workspace-uuid"),
            json!("00000000-0000-4000-8000-000000000001"),
            json!("--if-session-id"),
            json!("00000000-0000-4000-8000-000000000001"),
            json!("--if-session-revision"),
            json!("0"),
            json!("--idempotency-key"),
            json!("<idempotency-key>"),
        ]);
        candidate["result"]["observation"]["mutation_templates"][0]["argv"] =
            Value::Array(probe_argv);
        while serialized_json_len_v1(&candidate).unwrap() + 24_003 <= target_bytes {
            candidate["result"]["observation"]["mutation_templates"][0]["argv"]
                .as_array_mut()
                .unwrap()
                .insert(2, json!(full_block));
        }
        let base_len = serialized_json_len_v1(&candidate).unwrap();
        let extra = target_bytes
            .checked_sub(base_len)
            .expect("target_bytes must exceed 41-block base");
        // Final variable arg currently has content length 1 ("a"); target content length is 1 + extra.
        let content_bytes = 1 + extra;
        let control_count = content_bytes.saturating_sub(6) / 6;
        let ascii_count = content_bytes - control_count * 6;
        assert!(control_count + ascii_count <= 4096);
        let variable_arg = format!(
            "{}{}",
            "\u{001f}".repeat(control_count),
            "a".repeat(ascii_count)
        );
        let argv_mut = candidate["result"]["observation"]["mutation_templates"][0]["argv"]
            .as_array_mut()
            .unwrap();
        let var_idx = argv_mut.len() - 9;
        argv_mut[var_idx] = Value::String(variable_arg);
        assert_eq!(serialized_json_len_v1(&candidate).unwrap(), target_bytes);
        candidate
    };

    // Complete legal OutputEnvelopeV4 accepted at exact worst-case lookup boundary (lookup == 1,048,576).
    let exact_lookup_envelope = build_legal_envelope_of_bytes(max_stored_for_lookup);
    assert!(validate_daemon_free_document_v1(
        "podway.item-record-many-result/v2",
        &exact_lookup_envelope["result"]["mutation_result"]
    ));
    assert!(validate_daemon_free_document_v1(
        "podway.observation-result/v6",
        &exact_lookup_envelope["result"]["observation"]
    ));
    assert!(validate_daemon_free_document_v1(
        "podway.mutation-observation-result/v1",
        &exact_lookup_envelope["result"]
    ));
    assert!(
        validate_mutation_observation_command_result_v1(
            "item.record_many",
            &exact_lookup_envelope["result"]
        )
        .is_ok()
    );
    assert!(OutputEnvelopeV4::from_value(exact_lookup_envelope.clone()).is_ok());
    assert!(validate_mutation_observation_output_envelope_v1(&exact_lookup_envelope).is_ok());

    // 1-byte overhead proof: stored envelope + 1 byte (lookup == 1,048,577 while stored < 1,048,576) fails closed.
    let over_lookup_envelope = build_legal_envelope_of_bytes(max_stored_for_lookup + 1);
    assert!(serialized_json_len_v1(&over_lookup_envelope).unwrap() < MAX_DIRECT_OUTPUT_BYTES_V1);
    assert!(OutputEnvelopeV4::from_value(over_lookup_envelope.clone()).is_ok());
    assert_eq!(
        validate_mutation_observation_output_envelope_v1(&over_lookup_envelope),
        Err(ProtocolError::ValueTooLong {
            field: "outcome.lookup response",
            maximum: MAX_DIRECT_OUTPUT_BYTES_V1,
            actual: MAX_DIRECT_OUTPUT_BYTES_V1 + 1,
        })
    );

    // Stored envelope at 1,048,576 bytes passes standalone OutputEnvelopeV4 bound but fails combined lookup admission.
    let exact_stored_max_envelope = build_legal_envelope_of_bytes(MAX_DIRECT_OUTPUT_BYTES_V1);
    assert_eq!(
        serialized_json_len_v1(&exact_stored_max_envelope).unwrap(),
        MAX_DIRECT_OUTPUT_BYTES_V1
    );
    assert!(OutputEnvelopeV4::from_value(exact_stored_max_envelope.clone()).is_ok());
    assert_eq!(
        validate_mutation_observation_output_envelope_v1(&exact_stored_max_envelope),
        Err(ProtocolError::ValueTooLong {
            field: "outcome.lookup response",
            maximum: MAX_DIRECT_OUTPUT_BYTES_V1,
            actual: MAX_DIRECT_OUTPUT_BYTES_V1 + lookup_overhead_bytes,
        })
    );

    // Stored envelope over its own 1,048,576 limit (1,048,577 bytes) fails the stored limit check before lookup headroom.
    let over_stored_max_envelope = build_legal_envelope_of_bytes(MAX_DIRECT_OUTPUT_BYTES_V1 + 1);
    assert_eq!(
        serialized_json_len_v1(&over_stored_max_envelope).unwrap(),
        MAX_DIRECT_OUTPUT_BYTES_V1 + 1
    );
    assert!(OutputEnvelopeV4::from_value(over_stored_max_envelope.clone()).is_err());
    assert_eq!(
        validate_mutation_observation_output_envelope_v1(&over_stored_max_envelope),
        Err(ProtocolError::InvalidCommandResult {
            command: "item.record_many".to_owned(),
        })
    );

    // Exact outcome.lookup headroom boundary on synthetic object: 1,048,576 wrapper bytes accepted, 1,048,577 rejected.
    let pad_for_exact_max = MAX_DIRECT_OUTPUT_BYTES_V1 - base_wrapper_bytes;
    let exact_stored = json!({"pad": "x".repeat(pad_for_exact_max)});
    assert!(validate_direct_outcome_lookup_headroom_v1(&exact_stored).is_ok());

    let over_stored = json!({"pad": "x".repeat(pad_for_exact_max + 1)});
    assert_eq!(
        validate_direct_outcome_lookup_headroom_v1(&over_stored),
        Err(ProtocolError::ValueTooLong {
            field: "outcome.lookup response",
            maximum: MAX_DIRECT_OUTPUT_BYTES_V1,
            actual: MAX_DIRECT_OUTPUT_BYTES_V1 + 1,
        })
    );

    // Depth headroom: stored_envelope depth 61 -> wrapper depth 64 accepted; depth 62 -> 65 rejected.
    let make_depth = |depth: usize| {
        let mut v = Value::Null;
        for _ in 0..depth {
            v = json!({"a": v});
        }
        v
    };
    assert!(validate_direct_outcome_lookup_headroom_v1(&make_depth(61)).is_ok());
    assert_eq!(
        validate_direct_outcome_lookup_headroom_v1(&make_depth(62)),
        Err(ProtocolError::JsonDepthExceeded { maximum: 64 })
    );
    let mut over_depth_envelope = valid_envelope;
    over_depth_envelope["result"]["extra_depth"] = make_depth(63);
    assert_eq!(
        validate_mutation_observation_output_envelope_v1(&over_depth_envelope),
        Err(ProtocolError::JsonDepthExceeded { maximum: 64 })
    );
}
