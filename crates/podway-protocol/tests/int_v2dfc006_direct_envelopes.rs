use podway_protocol::{
    CommandNameV1, ErrorCodeV1, ErrorEnvelopeInputV2, ErrorEnvelopeV1, ErrorEnvelopeV2, ExitCodeV1,
    OutputEnvelopeInputV4, OutputEnvelopeV3, OutputEnvelopeV4, RecoveryRecipeV2, RequestIdV1,
    Rfc3339MillisV1, WorkspaceOutputV1,
};
use serde_json::{Value, json};

fn answers() -> Value {
    serde_json::from_str(include_str!(
        "../../../docs/examples/json/daemon-free-contract-reservation-v1.json"
    ))
    .unwrap()
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
    assert!(serde_json::from_value::<OutputEnvelopeV3>(success.clone()).is_err());
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(error.clone()).is_err());
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
