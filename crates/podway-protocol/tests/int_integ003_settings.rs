use podway_protocol::{
    ErrorEnvelopeV2, OutputEnvelopeV4, validate_command_result_v2, validate_daemon_free_document_v1,
};
use serde_json::{Value, json};

fn result() -> Value {
    json!({"schema":"podway.integration-settings-result/v1", "integration":"dolgorae", "enabled":false, "source":"default"})
}
fn output() -> Value {
    json!({"schema":"podway.output/v4", "request_id":"00000000-0000-4000-8000-000000000001", "command":"integration.show", "generated_at":"2026-11-01T00:00:00.000Z", "result":result(), "warnings":[]})
}

#[test]
fn integ003_result_and_output_memberships_are_closed_and_runtime_neutral() {
    for source in ["project", "user", "default"] {
        for enabled in [true, false] {
            let mut value = result();
            value["source"] = json!(source);
            value["enabled"] = json!(enabled);
            assert!(validate_daemon_free_document_v1(
                "podway.integration-settings-result/v1",
                &value
            ));
            assert!(validate_command_result_v2("integration.show", &value).is_ok());
            let mut envelope = output();
            envelope["result"] = value;
            OutputEnvelopeV4::from_value(envelope).unwrap();
        }
    }
    for (key, invalid) in [
        ("schema", json!("podway.integration-settings-result/v2")),
        ("integration", json!("other")),
        ("enabled", json!("true")),
        ("enabled", Value::Null),
        ("source", json!("inherit")),
        ("extra", json!(true)),
    ] {
        let mut value = output();
        value["result"][key] = invalid;
        assert!(OutputEnvelopeV4::from_value(value).is_err(), "{key}");
    }
    for field in ["schema", "integration", "enabled", "source"] {
        let mut value = output();
        value["result"].as_object_mut().unwrap().remove(field);
        assert!(OutputEnvelopeV4::from_value(value).is_err(), "{field}");
    }
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/protocol/current-contracts.json"
    ))
    .unwrap();
    for field in ["workspace", "session"] {
        let mut value = output();
        value[field] = if field == "workspace" {
            fixtures["fixtures"]["podway.output/v4"][field].clone()
        } else {
            json!({"id":"00000000-0000-4000-8000-000000000001", "title":"session", "lifecycle":"running", "revision_before":1, "revision_after":1})
        };
        assert!(OutputEnvelopeV4::from_value(value).is_err(), "{field}");
    }
}

#[test]
fn integ003_user_config_error_has_exact_metadata_and_empty_details() {
    let value = json!({"schema":"podway.error/v2", "request_id":"00000000-0000-4000-8000-000000000001", "command":"integration.show", "generated_at":"2026-11-01T00:00:00.000Z", "code":"USER_CONFIG_INVALID", "message":"User configuration is invalid or unavailable.", "retryable":false, "exit_code":5, "details":{}});
    ErrorEnvelopeV2::from_value(value.clone()).unwrap();
    for (field, invalid) in [
        ("retryable", json!(true)),
        ("exit_code", json!(6)),
        ("details", json!({"path":"private-home"})),
    ] {
        let mut invalid_value = value.clone();
        invalid_value[field] = invalid;
        assert!(ErrorEnvelopeV2::from_value(invalid_value).is_err());
    }
}

#[test]
fn integ003_project_error_empty_details_does_not_change_other_route_guidance() {
    let mut value = json!({"schema":"podway.error/v2", "request_id":"00000000-0000-4000-8000-000000000001", "command":"integration.show", "generated_at":"2026-11-01T00:00:00.000Z", "code":"WORKSPACE_CONFIG_INVALID", "message":"Workspace configuration is invalid.", "retryable":false, "exit_code":5, "details":{}});
    ErrorEnvelopeV2::from_value(value.clone()).unwrap();
    value["details"] = json!({"guidance":"retained legacy guidance"});
    assert!(ErrorEnvelopeV2::from_value(value.clone()).is_err());
    value["command"] = json!("workspace.init");
    ErrorEnvelopeV2::from_value(value).unwrap();
}
