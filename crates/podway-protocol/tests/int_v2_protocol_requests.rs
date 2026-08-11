use podway_core::{Sha256Digest, WorkspaceId};
use podway_protocol::{
    ProcedureV2MutationCommandV1, ProcedureV2MutationRequestV1, ProcedureV2StartRequestV1,
    RequestEnvelopeV1, SliceRequestV1, canonical_mutation_identity_v1,
    canonical_procedure_v2_mutation_identity_v1, canonical_procedure_v2_start_identity_v1,
    canonical_start_mutation_identity_v1, decode_initial_goal_v2,
};
use serde_json::{Value, json};

const REQUEST_ID: &str = "00000000-0000-4000-8000-000000000001";
const SESSION_ID: &str = "00000000-0000-4000-8000-000000000002";
const ATTEMPT_ID: &str = "00000000-0000-4000-8000-000000000003";
const WORKSPACE_ID: &str = "00000000-0000-4000-8000-000000000004";
const MANIFEST_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn selector() -> Value {
    json!({
        "version": 1,
        "path_bytes_base64url": "L3RtcC9wb2R3YXktdjI",
        "display": "/tmp/podway-v2",
        "expected_uuid": WORKSPACE_ID,
    })
}

fn envelope(command: &str, preconditions: Value, mut payload: Value) -> RequestEnvelopeV1 {
    payload
        .as_object_mut()
        .expect("test payload is an object")
        .insert("selector".to_owned(), selector());
    serde_json::from_value(json!({
        "protocol":"podway.ipc/v1",
        "request_id":REQUEST_ID,
        "client":{
            "name":"podway",
            "version":"0.2.0-dev",
            "pid":123,
            "product":"podway",
            "contract_manifest_digest":MANIFEST_DIGEST,
        },
        "operation":"mutate",
        "command":command,
        "workspace":{"root":"/tmp/podway-v2","expected_uuid":WORKSPACE_ID},
        "idempotency_key":"v2-request-key",
        "preconditions":preconditions,
        "options":{"detach":false,"wait_timeout_ms":30_000},
        "payload":payload,
    }))
    .unwrap()
}

fn session_preconditions() -> Value {
    json!({"session_id":SESSION_ID,"session_revision":7,"attempt_id":ATTEMPT_ID})
}

fn criteria() -> Value {
    json!([{"criterion_id":"tests","statement":"The focused tests pass."}])
}

#[test]
fn v2plt006_decodes_every_reserved_mutation_with_closed_bounded_payloads() {
    let cases = [
        (
            "session.decide",
            session_preconditions(),
            json!({"option_id":"accept","reason":"Evidence supports this route.","actor":"agent"}),
        ),
        (
            "session.rework",
            session_preconditions(),
            json!({"target_graph_node_id":"implement","reason":"The implementation needs correction."}),
        ),
        (
            "goal.define",
            json!({"session_id":SESSION_ID,"session_revision":7}),
            json!({"goal":"Ship the bounded protocol.","criteria":criteria()}),
        ),
        (
            "goal.revise",
            json!({"session_id":SESSION_ID,"session_revision":7,"goal_revision":1}),
            json!({"goal":"Ship the bounded protocol safely.","criteria":criteria(),"target_graph_node_id":"implement","reason":"Safety is now explicit.","reactivate":false}),
        ),
        (
            "goal.assess_criterion",
            json!({"session_id":SESSION_ID,"session_revision":7,"attempt_id":ATTEMPT_ID,"goal_revision":1}),
            json!({"criterion_id":"tests","status":"satisfied","reason":"The tests passed.","evidence":["verify"],"items":["summary"]}),
        ),
    ];

    for (command, preconditions, payload) in cases {
        let request =
            ProcedureV2MutationRequestV1::from_envelope(&envelope(command, preconditions, payload))
                .unwrap();
        assert_eq!(request.command().command_name(), command);
    }
}

#[test]
fn v2plt006_goal_revision_is_positive_required_and_command_scoped() {
    let valid = envelope(
        "goal.revise",
        json!({"session_id":SESSION_ID,"session_revision":7,"goal_revision":1}),
        json!({"goal":"Ship safely.","criteria":criteria(),"target_graph_node_id":"implement","reason":"Clarified."}),
    );
    assert!(ProcedureV2MutationRequestV1::from_envelope(&valid).is_ok());

    let missing = envelope(
        "goal.revise",
        json!({"session_id":SESSION_ID,"session_revision":7}),
        json!({"goal":"Ship safely.","criteria":criteria(),"target_graph_node_id":"implement","reason":"Clarified."}),
    );
    assert!(ProcedureV2MutationRequestV1::from_envelope(&missing).is_err());

    let zero = serde_json::to_value(&valid).unwrap();
    let mut zero = zero;
    zero["preconditions"]["goal_revision"] = json!(0);
    assert!(serde_json::from_value::<RequestEnvelopeV1>(zero).is_err());

    let maximum = envelope(
        "goal.revise",
        json!({"session_id":SESSION_ID,"session_revision":7,"goal_revision":u64::MAX}),
        json!({"goal":"Ship safely.","criteria":criteria(),"target_graph_node_id":"implement","reason":"Clarified."}),
    );
    assert!(ProcedureV2MutationRequestV1::from_envelope(&maximum).is_ok());

    let legacy = envelope(
        "session.complete",
        json!({"session_id":SESSION_ID,"session_revision":7,"attempt_id":ATTEMPT_ID,"goal_revision":1}),
        json!({}),
    );
    assert!(SliceRequestV1::from_envelope(&legacy).is_err());
}

#[test]
fn v2gol003_decide_accepts_an_optional_positive_goal_revision_and_binds_identity() {
    let general = ProcedureV2MutationRequestV1::from_envelope(&envelope(
        "session.decide",
        session_preconditions(),
        json!({"option_id":"accept","reason":"Choose the general route."}),
    ))
    .unwrap();
    let assessed = ProcedureV2MutationRequestV1::from_envelope(&envelope(
        "session.decide",
        json!({"session_id":SESSION_ID,"session_revision":7,"attempt_id":ATTEMPT_ID,"goal_revision":1}),
        json!({"option_id":"accept","reason":"Choose the assessed outcome."}),
    ))
    .unwrap();
    let ProcedureV2MutationCommandV1::SessionDecide(general_command) = general.command() else {
        unreachable!()
    };
    let ProcedureV2MutationCommandV1::SessionDecide(assessed_command) = assessed.command() else {
        unreachable!()
    };
    assert_eq!(general_command.expected_goal_revision, None);
    assert_eq!(assessed_command.expected_goal_revision, Some(1));

    let workspace = WorkspaceId::new(WORKSPACE_ID).unwrap();
    assert_ne!(
        canonical_procedure_v2_mutation_identity_v1(&general, &workspace).unwrap(),
        canonical_procedure_v2_mutation_identity_v1(&assessed, &workspace).unwrap()
    );

    let mut zero = serde_json::to_value(envelope(
        "session.decide",
        session_preconditions(),
        json!({"option_id":"accept","reason":"Invalid fence."}),
    ))
    .unwrap();
    zero["preconditions"]["goal_revision"] = json!(0);
    assert!(serde_json::from_value::<RequestEnvelopeV1>(zero).is_err());
}

#[test]
fn v2plt006_rejects_over_bounds_open_payloads_and_invalid_citations() {
    let over_reason = envelope(
        "session.decide",
        session_preconditions(),
        json!({"option_id":"accept","reason":"x".repeat(2_001)}),
    );
    assert!(ProcedureV2MutationRequestV1::from_envelope(&over_reason).is_err());

    let open = envelope(
        "session.decide",
        session_preconditions(),
        json!({"option_id":"accept","reason":"valid","unknown":true}),
    );
    assert!(ProcedureV2MutationRequestV1::from_envelope(&open).is_err());

    let duplicate_criteria = envelope(
        "goal.define",
        json!({"session_id":SESSION_ID,"session_revision":7}),
        json!({"goal":"Ship safely.","criteria":[{"criterion_id":"tests","statement":"One."},{"criterion_id":"tests","statement":"Two."}]}),
    );
    assert!(ProcedureV2MutationRequestV1::from_envelope(&duplicate_criteria).is_err());

    let invalid_citations = envelope(
        "goal.assess_criterion",
        json!({"session_id":SESSION_ID,"session_revision":7,"attempt_id":ATTEMPT_ID,"goal_revision":1}),
        json!({"criterion_id":"tests","status":"not_applicable","reason":"Superseded.","evidence":["verify"]}),
    );
    assert!(ProcedureV2MutationRequestV1::from_envelope(&invalid_citations).is_err());
}

#[test]
fn v2plt006_canonical_identity_excludes_transport_metadata_and_binds_semantics() {
    let first_envelope = envelope(
        "goal.revise",
        json!({"session_id":SESSION_ID,"session_revision":7,"goal_revision":1}),
        json!({"goal":"Ship safely.","criteria":criteria(),"target_graph_node_id":"implement","reason":"Clarified."}),
    );
    let mut replay_value = serde_json::to_value(&first_envelope).unwrap();
    replay_value["request_id"] = json!("00000000-0000-4000-8000-000000000099");
    replay_value["options"] = json!({"detach":true,"wait_timeout_ms":1});
    let replay_envelope: RequestEnvelopeV1 = serde_json::from_value(replay_value).unwrap();

    let first = ProcedureV2MutationRequestV1::from_envelope(&first_envelope).unwrap();
    let replay = ProcedureV2MutationRequestV1::from_envelope(&replay_envelope).unwrap();
    let workspace = WorkspaceId::new(WORKSPACE_ID).unwrap();
    let identity = canonical_procedure_v2_mutation_identity_v1(&first, &workspace).unwrap();
    assert_eq!(
        identity,
        canonical_procedure_v2_mutation_identity_v1(&replay, &workspace).unwrap()
    );

    let changed = ProcedureV2MutationRequestV1::from_envelope(&envelope(
        "goal.revise",
        json!({"session_id":SESSION_ID,"session_revision":7,"goal_revision":2}),
        json!({"goal":"Ship safely.","criteria":criteria(),"target_graph_node_id":"implement","reason":"Clarified."}),
    ))
    .unwrap();
    assert_ne!(
        identity,
        canonical_procedure_v2_mutation_identity_v1(&changed, &workspace).unwrap()
    );
}

#[test]
fn v2plt006_start_initial_goal_is_bounded_but_not_executable_before_dispatch_support() {
    let base = envelope(
        "session.start",
        json!({}),
        json!({"procedure":"workflow.yaml","task_title":"Implement protocol"}),
    );
    let base = SliceRequestV1::from_envelope(&base).unwrap();
    let workspace = WorkspaceId::new(WORKSPACE_ID).unwrap();
    let procedure_digest = Sha256Digest::new(MANIFEST_DIGEST).unwrap();
    let legacy_identity = canonical_mutation_identity_v1(&base, &workspace).unwrap();
    assert_eq!(
        legacy_identity,
        format!(
            "{{\"command\":\"session.start\",\"payload\":{{\"source\":{{\"procedure\":\"workflow.yaml\"}},\"task_title\":\"Implement protocol\"}},\"preconditions\":{{}},\"protocol_major\":1,\"workspace_id\":\"{WORKSPACE_ID}\"}}"
        )
    );

    let with_goal = envelope(
        "session.start",
        json!({}),
        json!({"procedure":"workflow.yaml","task_title":"Implement protocol","goal":"Ship safely.","criteria":criteria(),"actor":"agent"}),
    );
    let initial_goal = decode_initial_goal_v2(with_goal.payload())
        .unwrap()
        .unwrap();
    assert_eq!(initial_goal.goal, "Ship safely.");
    assert_eq!(initial_goal.criteria.len(), 1);
    assert!(SliceRequestV1::from_envelope(&with_goal).is_err());
    let typed = ProcedureV2StartRequestV1::from_envelope(&with_goal).unwrap();
    let goal_identity =
        canonical_procedure_v2_start_identity_v1(&typed, &workspace, &procedure_digest).unwrap();

    let no_goal = envelope(
        "session.start",
        json!({}),
        json!({"procedure":"workflow.yaml","task_title":"Implement protocol"}),
    );
    let typed_no_goal = ProcedureV2StartRequestV1::from_envelope(&no_goal).unwrap();
    assert_eq!(
        canonical_procedure_v2_start_identity_v1(&typed_no_goal, &workspace, &procedure_digest,)
            .unwrap(),
        canonical_start_mutation_identity_v1(
            &SliceRequestV1::from_envelope(&no_goal).unwrap(),
            &workspace,
            &procedure_digest,
        )
        .unwrap()
    );
    assert_ne!(
        goal_identity,
        canonical_procedure_v2_start_identity_v1(&typed_no_goal, &workspace, &procedure_digest,)
            .unwrap()
    );

    let mut replay_value = serde_json::to_value(&with_goal).unwrap();
    replay_value["request_id"] = json!("00000000-0000-4000-8000-000000000099");
    replay_value["options"] = json!({"detach":true,"wait_timeout_ms":1});
    let replay: RequestEnvelopeV1 = serde_json::from_value(replay_value).unwrap();
    assert_eq!(
        goal_identity,
        canonical_procedure_v2_start_identity_v1(
            &ProcedureV2StartRequestV1::from_envelope(&replay).unwrap(),
            &workspace,
            &procedure_digest,
        )
        .unwrap()
    );

    let open = envelope(
        "session.start",
        json!({}),
        json!({"procedure":"workflow.yaml","task_title":"Implement protocol","goal":"Ship safely.","criteria":criteria(),"unknown":true}),
    );
    assert!(ProcedureV2StartRequestV1::from_envelope(&open).is_err());

    let replace = envelope(
        "session.start_replace",
        json!({"session_id":SESSION_ID,"session_revision":7}),
        json!({"procedure":"workflow.yaml","task_title":"Implement protocol","goal":"Ship safely.","criteria":criteria(),"confirmed":true}),
    );
    assert!(SliceRequestV1::from_envelope(&replace).is_err());
    assert!(
        canonical_procedure_v2_start_identity_v1(
            &ProcedureV2StartRequestV1::from_envelope(&replace).unwrap(),
            &workspace,
            &procedure_digest,
        )
        .is_ok()
    );

    let incomplete = envelope(
        "session.start",
        json!({}),
        json!({"procedure":"workflow.yaml","task_title":"Implement protocol","goal":"Ship safely."}),
    );
    assert!(decode_initial_goal_v2(incomplete.payload()).is_err());
    assert!(SliceRequestV1::from_envelope(&incomplete).is_err());

    let overlong = envelope(
        "session.start",
        json!({}),
        json!({"procedure":"workflow.yaml","task_title":"Implement protocol","goal":"x".repeat(1_001),"criteria":criteria()}),
    );
    assert!(decode_initial_goal_v2(overlong.payload()).is_err());
    assert!(ProcedureV2StartRequestV1::from_envelope(&overlong).is_err());
}

#[test]
fn v2plt006_rework_accepts_attempt_only_when_observed() {
    for preconditions in [
        json!({"session_id":SESSION_ID,"session_revision":7}),
        session_preconditions(),
    ] {
        let request = ProcedureV2MutationRequestV1::from_envelope(&envelope(
            "session.rework",
            preconditions,
            json!({"target_graph_node_id":"implement","reason":"Rework."}),
        ))
        .unwrap();
        assert!(matches!(
            request.command(),
            ProcedureV2MutationCommandV1::SessionRework(_)
        ));
    }
}

#[test]
fn v2plt006_goal_revision_accepts_attempt_only_when_session_is_running() {
    let workspace = WorkspaceId::new(WORKSPACE_ID).unwrap();
    let mut identities = Vec::new();
    for preconditions in [
        json!({"session_id":SESSION_ID,"session_revision":7,"goal_revision":1}),
        json!({"session_id":SESSION_ID,"session_revision":7,"attempt_id":ATTEMPT_ID,"goal_revision":1}),
    ] {
        let request = ProcedureV2MutationRequestV1::from_envelope(&envelope(
            "goal.revise",
            preconditions,
            json!({"goal":"Ship safely.","criteria":criteria(),"target_graph_node_id":"implement","reason":"Clarified."}),
        ))
        .unwrap();
        identities.push(canonical_procedure_v2_mutation_identity_v1(&request, &workspace).unwrap());
    }
    assert_ne!(identities[0], identities[1]);
}
