use std::collections::BTreeSet;

use podway_protocol::{
    DAEMON_FREE_DOCUMENT_SCHEMAS_V1, MAX_DIRECT_INPUT_BYTES_V1, MAX_DIRECT_OUTPUT_BYTES_V1,
    SUPPORTED_ERROR_SCHEMAS_V1, SUPPORTED_OUTPUT_SCHEMAS_V3, decode_result_schema_contract_v2,
    validate_daemon_free_document_v1,
};
use serde_json::{Value, json};

fn known_answers() -> Value {
    serde_json::from_str(include_str!(
        "../../../docs/examples/json/daemon-free-contract-reservation-v1.json"
    ))
    .unwrap()
}

#[test]
fn reserved_families_validate_without_enabling_runtime_decoders() {
    let answers = known_answers();
    let fixtures = answers["fixtures"].as_object().unwrap();
    let registered = DAEMON_FREE_DOCUMENT_SCHEMAS_V1
        .iter()
        .map(|contract| contract.schema)
        .collect::<BTreeSet<_>>();
    assert_eq!(registered.len(), DAEMON_FREE_DOCUMENT_SCHEMAS_V1.len());
    assert_eq!(registered, fixtures.keys().map(String::as_str).collect());
    for (schema, value) in fixtures {
        assert!(validate_daemon_free_document_v1(schema, value), "{schema}");
        assert!(
            decode_result_schema_contract_v2(value.as_object().unwrap()).is_none(),
            "reserved family {schema} must not enter the shipped decoder"
        );
    }
    for case in answers["cases"].as_array().unwrap() {
        assert!(
            validate_daemon_free_document_v1(case["schema"].as_str().unwrap(), &case["value"]),
            "{}",
            case["name"]
        );
    }
    assert_eq!(SUPPORTED_OUTPUT_SCHEMAS_V3, &["podway.output/v3"]);
    assert_eq!(SUPPORTED_ERROR_SCHEMAS_V1, &["podway.error/v1"]);
    assert_eq!(MAX_DIRECT_INPUT_BYTES_V1, 1_048_576);
    assert_eq!(MAX_DIRECT_OUTPUT_BYTES_V1, 1_048_576);
}

#[test]
fn daemon_metadata_and_invalid_reconciliation_fail_closed() {
    let answers = known_answers();
    for schema in [
        "podway.output/v4",
        "podway.error/v2",
        "podway.session-start-result/v4",
        "podway.status-result/v5",
        "podway.compact-status-result/v5",
        "podway.next-result/v5",
        "podway.observation-result/v5",
    ] {
        for field in ["job", "admission", "queue"] {
            let mut value = answers["fixtures"][schema].clone();
            value[field] = json!({});
            assert!(
                !validate_daemon_free_document_v1(schema, &value),
                "{schema}: {field}"
            );
        }
    }
    let mut lookup = answers["fixtures"]["podway.outcome-lookup-result/v1"].clone();
    lookup["found"] = json!(true);
    lookup["outcome"] = Value::Null;
    assert!(!validate_daemon_free_document_v1(
        "podway.outcome-lookup-result/v1",
        &lookup
    ));
    let mut error = answers["fixtures"]["podway.error/v2"].clone();
    error["code"] = json!("DAEMON_UNAVAILABLE");
    error["exit_code"] = json!(3);
    assert!(!validate_daemon_free_document_v1("podway.error/v2", &error));
    let mut recipe = answers["fixtures"]["podway.recovery-recipe/v2"].clone();
    recipe["command"] = json!("job.lookup");
    assert!(!validate_daemon_free_document_v1(
        "podway.recovery-recipe/v2",
        &recipe
    ));
    let mut marker = answers["fixtures"]["podway.workspace-reset-marker/v1"].clone();
    marker["request_digest"] = json!("foreign");
    assert!(!validate_daemon_free_document_v1(
        "podway.workspace-reset-marker/v1",
        &marker
    ));
    assert!(!validate_daemon_free_document_v1(
        "podway.unknown/v1",
        &json!({})
    ));
    let oversized = json!({"message": "x".repeat(MAX_DIRECT_OUTPUT_BYTES_V1)});
    assert!(!validate_daemon_free_document_v1(
        "podway.output/v4",
        &oversized
    ));
}

#[test]
fn error_recovery_is_bound_to_the_required_operation() {
    let answers = known_answers();
    let cases = answers["cases"].as_array().unwrap();
    let recipe =
        |name: &str| cases.iter().find(|case| case["name"] == name).unwrap()["value"].clone();
    for (schema, prohibited) in [
        (
            "podway.mutation-outcome-unknown-details/v3",
            "recovery-reset",
        ),
        (
            "podway.mutation-outcome-unknown-details/v3",
            "recovery-refresh",
        ),
        ("podway.workspace-busy-details/v1", "recovery-reset"),
        ("podway.workspace-busy-details/v1", "recovery-resume"),
        (
            "podway.legacy-runtime-state-unsupported-details/v1",
            "recovery-refresh",
        ),
        (
            "podway.legacy-runtime-state-unsupported-details/v1",
            "recovery-lookup",
        ),
    ] {
        let mut details = answers["fixtures"][schema].clone();
        details["recovery"] = recipe(prohibited);
        assert!(
            !validate_daemon_free_document_v1(schema, &details),
            "{schema}: {prohibited}"
        );
        let code = match schema {
            "podway.mutation-outcome-unknown-details/v3" => "MUTATION_OUTCOME_UNKNOWN",
            "podway.workspace-busy-details/v1" => "WORKSPACE_BUSY",
            _ => "LEGACY_RUNTIME_STATE_UNSUPPORTED",
        };
        let mut envelope = cases
            .iter()
            .find(|case| case["value"]["code"] == code)
            .map_or_else(
                || answers["fixtures"]["podway.error/v2"].clone(),
                |case| case["value"].clone(),
            );
        envelope["details"] = details;
        assert!(
            !validate_daemon_free_document_v1("podway.error/v2", &envelope),
            "{code}: {prohibited}"
        );
    }
    for (schema, code, exit_code, retryable, permitted) in [
        (
            "podway.revision-conflict-details/v3",
            "SESSION_REVISION_CONFLICT",
            4,
            true,
            "recovery-refresh",
        ),
        (
            "podway.attempt-conflict-details/v3",
            "ATTEMPT_NOT_CURRENT",
            4,
            true,
            "recovery-refresh",
        ),
        (
            "podway.session-id-mismatch-details/v3",
            "SESSION_ID_MISMATCH",
            4,
            false,
            "recovery-refresh",
        ),
        (
            "podway.procedure-digest-mismatch-details/v3",
            "PROCEDURE_DIGEST_MISMATCH",
            4,
            false,
            "recovery-refresh",
        ),
        (
            "podway.recoverable-v2-runtime-error-details/v2",
            "GOAL_REVISION_STALE",
            4,
            true,
            "recovery-refresh",
        ),
        (
            "podway.workspace-uuid-mismatch-details/v3",
            "WORKSPACE_UUID_MISMATCH",
            4,
            false,
            "recovery-doctor",
        ),
        (
            "podway.workspace-recovery-details/v2",
            "WORKSPACE_STATE_UNREADABLE",
            5,
            false,
            "recovery-doctor",
        ),
        (
            "podway.workspace-contract-mismatch-details/v1",
            "WORKSPACE_CONTRACT_MISMATCH",
            5,
            false,
            "recovery-doctor",
        ),
    ] {
        let mut details = answers["fixtures"][schema].clone();
        details["recovery"] = recipe(permitted);
        assert!(
            validate_daemon_free_document_v1(schema, &details),
            "{schema}: required recovery"
        );
        let mut envelope = answers["fixtures"]["podway.error/v2"].clone();
        envelope["code"] = json!(code);
        envelope["exit_code"] = json!(exit_code);
        envelope["retryable"] = json!(retryable);
        envelope["details"] = details.clone();
        assert!(
            validate_daemon_free_document_v1("podway.error/v2", &envelope),
            "{code}: required recovery"
        );
        for prohibited in [
            "recovery-reset",
            "recovery-resume",
            "recovery-lookup",
            "recovery-refresh",
            "recovery-doctor",
        ] {
            if prohibited == permitted {
                continue;
            }
            details["recovery"] = recipe(prohibited);
            assert!(
                !validate_daemon_free_document_v1(schema, &details),
                "{schema}: {prohibited}"
            );
            envelope["details"] = details.clone();
            assert!(
                !validate_daemon_free_document_v1("podway.error/v2", &envelope),
                "{code}: {prohibited}"
            );
        }
    }
    for boundary in ["maintenance_lock", "intent_marker", "predecessor_migration"] {
        let mut details = answers["fixtures"]["podway.workspace-maintenance-details/v1"].clone();
        details["boundary"] = json!(boundary);
        for permitted in ["recovery-refresh", "recovery-doctor"] {
            details["recovery"] = recipe(permitted);
            assert!(
                validate_daemon_free_document_v1(
                    "podway.workspace-maintenance-details/v1",
                    &details
                ),
                "{boundary}: {permitted}"
            );
        }
        details["recovery"] = recipe("recovery-reset");
        assert!(
            !validate_daemon_free_document_v1("podway.workspace-maintenance-details/v1", &details),
            "{boundary}: destructive recovery"
        );
    }
    let mut removal = answers["fixtures"]["podway.workspace-maintenance-details/v1"].clone();
    removal["boundary"] = json!("removal_marker");
    assert!(!validate_daemon_free_document_v1(
        "podway.workspace-maintenance-details/v1",
        &removal
    ));
    removal["recovery"]["command"] = json!("workspace.remove");
    removal["recovery"]["argv"] = json!([
        "podway",
        "--json",
        "workspace",
        "remove",
        "--yes",
        "--idempotency-key",
        "example-key"
    ]);
    assert!(validate_daemon_free_document_v1(
        "podway.workspace-maintenance-details/v1",
        &removal
    ));
    let mut reset = answers["fixtures"]["podway.workspace-maintenance-details/v1"].clone();
    reset["recovery"] = recipe("recovery-refresh");
    assert!(!validate_daemon_free_document_v1(
        "podway.workspace-maintenance-details/v1",
        &reset
    ));
}

#[test]
fn uncertain_commit_lookup_uses_the_original_key() {
    let answers = known_answers();
    let schema = "podway.mutation-outcome-unknown-details/v3";
    let envelope = answers["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["value"]["code"] == "MUTATION_OUTCOME_UNKNOWN")
        .unwrap()["value"]
        .clone();
    for field in ["reconcile", "recovery"] {
        let mut details = answers["fixtures"][schema].clone();
        if field == "reconcile" {
            details["reconcile"]["idempotency_key"] = json!("different-key");
        } else {
            details["recovery"]["argv"][5] = json!("different-key");
        }
        assert!(
            !validate_daemon_free_document_v1(schema, &details),
            "{field}"
        );
        let mut error = envelope.clone();
        error["details"] = details;
        assert!(
            !validate_daemon_free_document_v1("podway.error/v2", &error),
            "{field}"
        );
    }
}

#[test]
fn generic_results_preserve_object_compatibility_and_document_bounds() {
    let mut envelope = known_answers()["fixtures"]["podway.output/v4"].clone();
    envelope["command"] = json!("procedure.show");
    let properties = (0..129)
        .map(|index| (format!("p{index}"), json!(index)))
        .collect::<serde_json::Map<_, _>>();
    for result in [
        json!({"canonical_json": "x".repeat(65_537)}),
        json!({"values": vec![Value::Null; 1_025]}),
        Value::Object(properties),
        json!({"": true, ("x".repeat(257)): false}),
    ] {
        envelope["result"] = result;
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            &envelope
        ));
    }
    for result in [Value::Null, json!([]), json!(true), json!("text")] {
        envelope["result"] = result;
        assert!(!validate_daemon_free_document_v1(
            "podway.output/v4",
            &envelope
        ));
    }
    envelope["result"] = json!({"value": ""});
    let overhead = serde_json::to_vec(&envelope).unwrap().len();
    envelope["result"]["value"] = json!("x".repeat(MAX_DIRECT_OUTPUT_BYTES_V1 - overhead));
    assert_eq!(
        serde_json::to_vec(&envelope).unwrap().len(),
        MAX_DIRECT_OUTPUT_BYTES_V1
    );
    assert!(validate_daemon_free_document_v1(
        "podway.output/v4",
        &envelope
    ));
    envelope["result"]["value"] = json!("x".repeat(MAX_DIRECT_OUTPUT_BYTES_V1 - overhead + 1));
    assert!(!validate_daemon_free_document_v1(
        "podway.output/v4",
        &envelope
    ));
    let mut nested = Value::Null;
    for _ in 0..podway_protocol::MAX_JSON_DEPTH_V1 - 1 {
        nested = json!({"nested": nested});
    }
    envelope["result"] = nested.clone();
    assert!(validate_daemon_free_document_v1(
        "podway.output/v4",
        &envelope
    ));
    envelope["result"] = json!({"nested": nested});
    assert!(!validate_daemon_free_document_v1(
        "podway.output/v4",
        &envelope
    ));
}
