use podway_protocol::{
    CommandNameV1, ErrorCodeV1, ErrorEnvelopeInputV1, ErrorEnvelopeV1, ExitCodeV1, RequestIdV1,
    Rfc3339MillisV1,
};
use serde_json::{Map, Value, json};

const REQUEST_ID: &str = "00000000-0000-4000-8000-000000000501";
const JOB_ID: &str = "00000000-0000-4000-8000-000000000502";
const FIRST_ID: &str = "00000000-0000-4000-8000-000000000503";
const SECOND_ID: &str = "00000000-0000-4000-8000-000000000504";
const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn details(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn error(
    code: &str,
    exit_code: u8,
    retryable: bool,
    details: Map<String, Value>,
) -> ErrorEnvelopeV1 {
    ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        command: CommandNameV1::new("status").unwrap(),
        generated_at: Rfc3339MillisV1::new("2026-08-15T00:00:00.000Z").unwrap(),
        code: ErrorCodeV1::new(code).unwrap(),
        message: "bounded recovery fixture".to_owned(),
        retryable,
        exit_code: ExitCodeV1::new(exit_code).unwrap(),
        workspace: None,
        details,
    })
    .unwrap()
}

fn fixtures() -> Vec<(&'static str, ErrorEnvelopeV1, &'static str, &'static str)> {
    vec![
        (
            "podway.endpoint-error-details/v2",
            error("DAEMON_UNAVAILABLE", 3, true, Map::new()),
            "inspect_daemon",
            "daemon.status",
        ),
        (
            "podway.daemon-contract-mismatch-details/v2",
            error(
                "DAEMON_CONTRACT_MISMATCH",
                3,
                false,
                details(json!({
                    "expected":{"product":"secret-product","contract_manifest_digest":DIGEST_A},
                    "actual":{"product":"podway","contract_manifest_digest":DIGEST_B},
                    "admission":{"admitted":false}
                })),
            ),
            "inspect_daemon",
            "daemon.status",
        ),
        (
            "podway.workspace-uuid-mismatch-details/v2",
            error(
                "WORKSPACE_UUID_MISMATCH",
                4,
                false,
                details(json!({
                    "expected_workspace_uuid":FIRST_ID,
                    "actual_workspace_uuid":SECOND_ID,
                    "admission":{"admitted":false}
                })),
            ),
            "diagnose_workspace",
            "workspace.doctor",
        ),
        (
            "podway.workspace-recovery-details/v1",
            error("WORKSPACE_STATE_UNREADABLE", 5, false, Map::new()),
            "diagnose_workspace",
            "workspace.doctor",
        ),
        (
            "podway.workspace-recovery-details/v1",
            error("WORKSPACE_SCHEMA_UNSUPPORTED", 5, false, Map::new()),
            "diagnose_workspace",
            "workspace.doctor",
        ),
        (
            "podway.procedure-digest-mismatch-details/v2",
            error(
                "PROCEDURE_DIGEST_MISMATCH",
                4,
                false,
                details(json!({
                    "expected_procedure_digest":DIGEST_A,
                    "actual_procedure_digest":DIGEST_B,
                    "admission":{"admitted":false}
                })),
            ),
            "refresh_state",
            "session.observe",
        ),
        (
            "podway.session-id-mismatch-details/v2",
            error(
                "SESSION_ID_MISMATCH",
                4,
                false,
                details(json!({
                    "expected_session_id":FIRST_ID,
                    "actual_session_id":SECOND_ID,
                    "admission":{"admitted":false}
                })),
            ),
            "refresh_state",
            "session.observe",
        ),
        (
            "podway.revision-conflict-details/v2",
            error(
                "SESSION_REVISION_CONFLICT",
                4,
                true,
                details(json!({"expected_revision":7,"current_revision":8})),
            ),
            "refresh_state",
            "session.observe",
        ),
        (
            "podway.attempt-conflict-details/v2",
            error(
                "ATTEMPT_NOT_CURRENT",
                4,
                true,
                details(json!({"expected_attempt_id":FIRST_ID,"actual_attempt_id":SECOND_ID})),
            ),
            "refresh_state",
            "session.observe",
        ),
        (
            "podway.revision-conflict-details/v2",
            error(
                "ITEM_REVISION_CONFLICT",
                4,
                true,
                details(json!({"expected_revision":2,"current_revision":3})),
            ),
            "refresh_state",
            "session.observe",
        ),
        (
            "podway.job-wait-timeout-details/v2",
            error(
                "JOB_WAIT_TIMEOUT",
                4,
                true,
                details(json!({
                    "job_id":JOB_ID,
                    "job_sequence":9,
                    "admission":{"admitted":true,"job_id":JOB_ID,"workspace_sequence":9}
                })),
            ),
            "wait_for_job",
            "job.wait",
        ),
        (
            "podway.mutation-outcome-unknown-details/v2",
            error(
                "MUTATION_OUTCOME_UNKNOWN",
                4,
                true,
                details(json!({
                    "outcome":"unknown",
                    "idempotency_key":"original-key",
                    "reconcile":{"command":"job.lookup","idempotency_key":"original-key"}
                })),
            ),
            "reconcile_mutation",
            "job.lookup",
        ),
        (
            "podway.recoverable-v2-runtime-error-details/v1",
            error(
                "EVIDENCE_REFERENCE_STALE",
                4,
                true,
                details(json!({
                    "kind":"EVIDENCE_REFERENCE_STALE",
                    "graph_node_id":"review",
                    "source_graph_node_id":"verify",
                    "expected_source_attempt_id":FIRST_ID,
                    "current_source_attempt_id":SECOND_ID,
                    "admission":{"admitted":false}
                })),
            ),
            "refresh_state",
            "session.observe",
        ),
        (
            "podway.recoverable-v2-runtime-error-details/v1",
            error(
                "GOAL_REVISION_STALE",
                4,
                true,
                details(json!({
                    "kind":"GOAL_REVISION_STALE",
                    "expected_goal_revision":1,
                    "actual_goal_revision":2,
                    "admission":{"admitted":false}
                })),
            ),
            "refresh_state",
            "session.observe",
        ),
    ]
}

#[test]
fn v2agt005_every_adopted_error_has_one_exact_bounded_read_only_recipe() {
    let forbidden = [
        "retry",
        "restart",
        "repair",
        "reset",
        "install",
        "uninstall",
        "start",
        "stop",
        "terminate",
        "cancel",
    ];
    for (schema, error, action, command) in fixtures() {
        assert_eq!(error.details()["schema"], schema);
        let recovery = error.details()["recovery"].as_object().unwrap();
        assert_eq!(recovery["action"], action);
        assert_eq!(recovery["command"], command);
        assert_eq!(recovery["requires_explicit_authorization"], false);
        let argv = recovery["argv"].as_array().unwrap();
        assert!((2..=8).contains(&argv.len()));
        assert_eq!(argv[0], "podway");
        assert!(argv.iter().all(|value| {
            value
                .as_str()
                .is_some_and(|value| !forbidden.contains(&value))
        }));
        let encoded = serde_json::to_value(&error).unwrap();
        serde_json::from_value::<ErrorEnvelopeV1>(encoded).unwrap();
    }
}

#[test]
fn v2agt005_recovery_rejects_mutating_open_overlong_or_authorization_tampering() {
    let base = serde_json::to_value(error(
        "SESSION_REVISION_CONFLICT",
        4,
        true,
        details(json!({"expected_revision":7,"current_revision":8})),
    ))
    .unwrap();
    let mut cases = Vec::new();

    let mut mutating = base.clone();
    mutating["details"]["recovery"]["command"] = json!("session.reset");
    mutating["details"]["recovery"]["argv"] = json!(["podway", "reset", "--yes"]);
    cases.push(mutating);
    let mut authorized = base.clone();
    authorized["details"]["recovery"]["requires_explicit_authorization"] = json!(true);
    cases.push(authorized);
    let mut open = base.clone();
    open["details"]["recovery"]["extra"] = json!(true);
    cases.push(open);
    let mut overlong = base;
    overlong["details"]["recovery"]["reason"] = json!("x".repeat(257));
    cases.push(overlong);

    for value in cases {
        assert!(serde_json::from_value::<ErrorEnvelopeV1>(value).is_err());
    }
}

#[test]
fn v2agt005_recovery_does_not_copy_unrelated_error_values() {
    let error = fixtures()
        .into_iter()
        .find(|(_, error, _, _)| error.code().as_str() == "DAEMON_CONTRACT_MISMATCH")
        .unwrap()
        .1;
    let recovery = serde_json::to_string(&error.details()["recovery"]).unwrap();
    assert!(!recovery.contains("secret-product"));
    assert!(!recovery.contains(DIGEST_A));
    assert!(!recovery.contains(DIGEST_B));
}
