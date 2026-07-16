use podway_core::{Revision, WorkspaceId};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, MAX_WORKTREE_SELECTOR_COMPONENT_BYTES_V1,
    NextResultV1, OperationV1, PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1,
    RequestIdV1, RequestOptionsV1, SliceRequestV1, StatusResultV1, WorktreeSelectorWireV1,
    canonical_mutation_identity_bytes_v1, canonical_mutation_identity_v1,
    decode_base64url_unpadded_v1, encode_base64url_unpadded_v1,
};
use serde_json::{Map, Value, json};

const REQUEST_ID: &str = "11111111-1111-4111-8111-111111111111";
const ATTEMPT_ID: &str = "22222222-2222-4222-8222-222222222222";
const BLOCKER_ID: &str = "33333333-3333-4333-8333-333333333333";
const WORKSPACE_ID: &str = "44444444-4444-4444-8444-444444444444";
const SESSION_ID: &str = "55555555-5555-4555-8555-555555555555";
const JOB_ID: &str = "66666666-6666-4666-8666-666666666666";

fn selector(path: &[u8], display: &str) -> WorktreeSelectorWireV1 {
    WorktreeSelectorWireV1::new(path, display, Some(WorkspaceId::new(WORKSPACE_ID).unwrap()))
        .unwrap()
}

fn payload(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn item_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(
        None,
        None,
        Some(ATTEMPT_ID.to_owned().try_into().unwrap()),
        Some(Revision::new(2)),
        None,
        None,
    )
    .unwrap()
}

fn session_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(
        None,
        Some(Revision::new(3)),
        Some(ATTEMPT_ID.to_owned().try_into().unwrap()),
        None,
        None,
        None,
    )
    .unwrap()
}

struct EnvelopeTransport<'a> {
    detach: bool,
    wait_timeout_ms: u64,
    workspace_root: &'a str,
    request_id: &'a str,
    client_name: &'a str,
}

fn envelope(
    command: &str,
    operation: OperationV1,
    command_payload: Value,
    preconditions: PreconditionsV1,
    transport: EnvelopeTransport<'_>,
) -> RequestEnvelopeV1 {
    let mutation = matches!(operation, OperationV1::Bootstrap | OperationV1::Mutate);
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(transport.request_id).unwrap(),
        client: ClientInfoV1::new(transport.client_name, "1.0.0", 42).unwrap(),
        operation,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(
            podway_protocol::WorkspaceContextV1::new(transport.workspace_root, None).unwrap(),
        ),
        idempotency_key: mutation.then(|| IdempotencyKeyV1::new("semantic-key").unwrap()),
        preconditions,
        options: RequestOptionsV1::new(transport.detach, transport.wait_timeout_ms).unwrap(),
        payload: payload(command_payload),
    })
    .unwrap()
}

fn selector_json(path: &[u8], display: &str) -> Value {
    serde_json::to_value(selector(path, display)).unwrap()
}

#[test]
fn g005_parses_every_admitted_command_with_exact_preconditions() {
    let selector = selector_json(b"/worktree", "/worktree");
    let no_preconditions = PreconditionsV1::default();

    let cases = [
        (
            envelope(
                "workspace.init",
                OperationV1::Bootstrap,
                json!({"selector": selector.clone()}),
                no_preconditions.clone(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "workspace.init",
        ),
        (
            envelope(
                "preset.start",
                OperationV1::Mutate,
                json!({"selector": selector.clone(), "preset": "bug-fix", "task_title": "Fix the login"}),
                no_preconditions.clone(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "preset.start",
        ),
        (
            envelope(
                "session.status",
                OperationV1::Query,
                json!({"selector": selector.clone()}),
                no_preconditions.clone(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "session.status",
        ),
        (
            envelope(
                "session.next",
                OperationV1::Query,
                json!({"selector": selector.clone()}),
                no_preconditions.clone(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "session.next",
        ),
        (
            envelope(
                "item.check",
                OperationV1::Mutate,
                json!({"selector": selector.clone(), "item_id": "check"}),
                item_preconditions(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "item.check",
        ),
        (
            envelope(
                "item.set",
                OperationV1::Mutate,
                json!({"selector": selector.clone(), "item_id": "note", "value": "verified"}),
                item_preconditions(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "item.set",
        ),
        (
            envelope(
                "item.add",
                OperationV1::Mutate,
                json!({"selector": selector.clone(), "item_id": "files", "value": "src/lib.rs"}),
                item_preconditions(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "item.add",
        ),
        (
            envelope(
                "item.attach_path",
                OperationV1::Mutate,
                json!({"selector": selector.clone(), "item_id": "artifact", "path": "proof/report.txt", "media_type": "text/plain"}),
                item_preconditions(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "item.attach_path",
        ),
        (
            envelope(
                "session.block",
                OperationV1::Mutate,
                json!({"selector": selector.clone(), "reason": "Waiting for review"}),
                session_preconditions(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "session.block",
        ),
        (
            envelope(
                "session.unblock",
                OperationV1::Mutate,
                json!({"selector": selector.clone(), "blocker_id": BLOCKER_ID, "all": false}),
                session_preconditions(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "session.unblock",
        ),
        (
            envelope(
                "session.retry",
                OperationV1::Mutate,
                json!({"selector": selector.clone(), "reason": "Repair test coverage"}),
                session_preconditions(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "session.retry",
        ),
        (
            envelope(
                "session.return",
                OperationV1::Mutate,
                json!({"selector": selector.clone(), "destination_stage_id": "diagnose", "reason": "Need a new diagnosis"}),
                session_preconditions(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "session.return",
        ),
        (
            envelope(
                "session.complete",
                OperationV1::Mutate,
                json!({"selector": selector}),
                session_preconditions(),
                EnvelopeTransport {
                    detach: false,
                    wait_timeout_ms: 0,
                    workspace_root: "/legacy-root",
                    request_id: REQUEST_ID,
                    client_name: "podway-cli",
                },
            ),
            "session.complete",
        ),
    ];

    for (envelope, expected_name) in cases {
        let request = SliceRequestV1::from_envelope(&envelope).unwrap();
        assert_eq!(request.command().command_name(), expected_name);
    }
}

#[test]
fn g005_rejects_wrong_operations_missing_or_extra_conditions_and_unknown_fields() {
    let selector = selector_json(b"/worktree", "/worktree");
    let wrong_operation = envelope(
        "session.status",
        OperationV1::Control,
        json!({"selector": selector.clone()}),
        PreconditionsV1::default(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/legacy-root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(SliceRequestV1::from_envelope(&wrong_operation).is_err());

    let missing_item_precondition = envelope(
        "item.check",
        OperationV1::Mutate,
        json!({"selector": selector.clone(), "item_id": "check"}),
        PreconditionsV1::default(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/legacy-root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(SliceRequestV1::from_envelope(&missing_item_precondition).is_err());

    let extra_query_precondition = envelope(
        "session.next",
        OperationV1::Query,
        json!({"selector": selector.clone()}),
        session_preconditions(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/legacy-root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(SliceRequestV1::from_envelope(&extra_query_precondition).is_err());

    let unknown_payload_field = envelope(
        "preset.start",
        OperationV1::Mutate,
        json!({
            "selector": selector.clone(),
            "preset": "bug-fix",
            "task_title": "Fix it",
            "unknown": true,
        }),
        PreconditionsV1::default(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/legacy-root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(SliceRequestV1::from_envelope(&unknown_payload_field).is_err());

    let ambiguous_unblock = envelope(
        "session.unblock",
        OperationV1::Mutate,
        json!({"selector": selector.clone(), "blocker_id": BLOCKER_ID, "all": true}),
        session_preconditions(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/legacy-root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(SliceRequestV1::from_envelope(&ambiguous_unblock).is_err());

    let unimplemented_command = envelope(
        "session.cancel",
        OperationV1::Mutate,
        json!({"selector": selector}),
        session_preconditions(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/legacy-root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(SliceRequestV1::from_envelope(&unimplemented_command).is_err());
}

#[test]
fn selector_round_trips_non_utf8_path_bytes_and_enforces_all_bounds() {
    let path = b"/worktree/non-utf8-\xff";
    let selector_value = selector(path, "/worktree/non-utf8-");
    let encoded = serde_json::to_value(&selector_value).unwrap();
    let decoded: WorktreeSelectorWireV1 = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.path_bytes().unwrap(), path.as_slice());
    assert_eq!(
        decode_base64url_unpadded_v1(&encode_base64url_unpadded_v1(path)).unwrap(),
        path.as_slice()
    );

    assert!(decode_base64url_unpadded_v1("a").is_err());
    assert!(decode_base64url_unpadded_v1("Lw=").is_err());
    assert!(WorktreeSelectorWireV1::new(b"relative", "relative", None).is_err());
    assert!(WorktreeSelectorWireV1::new(b"//host/share", "//host/share", None).is_err());
    assert!(WorktreeSelectorWireV1::new(b"/nul\0path", "nul", None).is_err());
    assert!(WorktreeSelectorWireV1::new(b"/", "", None).is_err());

    let mut too_long_path = vec![b'/'];
    too_long_path.extend(vec![b'a'; MAX_WORKTREE_SELECTOR_COMPONENT_BYTES_V1]);
    assert!(WorktreeSelectorWireV1::new(&too_long_path, "/long", None).is_err());
    let too_long_display = "x".repeat(MAX_WORKTREE_SELECTOR_COMPONENT_BYTES_V1 + 1);
    assert!(WorktreeSelectorWireV1::new(b"/valid", too_long_display, None).is_err());

    let mut selector_json = serde_json::to_value(selector(b"/valid", "/valid")).unwrap();
    selector_json
        .as_object_mut()
        .unwrap()
        .remove("expected_uuid");
    assert!(serde_json::from_value::<WorktreeSelectorWireV1>(selector_json).is_err());
    let mut unsupported_version = serde_json::to_value(selector(b"/valid", "/valid")).unwrap();
    unsupported_version["version"] = json!(2);
    assert!(serde_json::from_value::<WorktreeSelectorWireV1>(unsupported_version).is_err());
}
#[test]
fn g005_enforces_workspace_and_command_text_reason_and_path_bounds() {
    let no_workspace = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        client: ClientInfoV1::new("podway-cli", "1.0.0", 42).unwrap(),
        operation: OperationV1::Query,
        command: CommandNameV1::new("session.status").unwrap(),
        workspace: None,
        idempotency_key: None,
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).unwrap(),
        payload: payload(json!({"selector": selector_json(b"/worktree", "/worktree")})),
    })
    .unwrap();
    assert!(SliceRequestV1::from_envelope(&no_workspace).is_err());

    let selector = selector_json(b"/worktree", "/worktree");
    let overlong_title = envelope(
        "preset.start",
        OperationV1::Mutate,
        json!({"selector": selector.clone(), "preset": "bug-fix", "task_title": "x".repeat(501)}),
        PreconditionsV1::default(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/legacy-root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(SliceRequestV1::from_envelope(&overlong_title).is_err());

    let overlong_text = envelope(
        "item.set",
        OperationV1::Mutate,
        json!({"selector": selector.clone(), "item_id": "note", "value": "x".repeat(65_537)}),
        item_preconditions(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/legacy-root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(SliceRequestV1::from_envelope(&overlong_text).is_err());

    let overlong_list_value = envelope(
        "item.add",
        OperationV1::Mutate,
        json!({"selector": selector.clone(), "item_id": "files", "value": "x".repeat(4_001)}),
        item_preconditions(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/legacy-root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(SliceRequestV1::from_envelope(&overlong_list_value).is_err());

    let invalid_path = envelope(
        "item.attach_path",
        OperationV1::Mutate,
        json!({"selector": selector.clone(), "item_id": "artifact", "path": "../escape"}),
        item_preconditions(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/legacy-root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(SliceRequestV1::from_envelope(&invalid_path).is_err());

    let overlong_reason = envelope(
        "session.retry",
        OperationV1::Mutate,
        json!({"selector": selector, "reason": "x".repeat(4_001)}),
        session_preconditions(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/legacy-root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(SliceRequestV1::from_envelope(&overlong_reason).is_err());
}

#[test]
fn mutation_identity_excludes_transport_and_selector_identity_hints_but_keeps_semantics() {
    let first = envelope(
        "item.set",
        OperationV1::Mutate,
        json!({"selector": selector_json(b"/first-root", "/first diagnostic"), "item_id": "note", "value": "one"}),
        item_preconditions(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/first legacy root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    let mut moved_selector = selector_json(b"/moved-root", "/different diagnostic");
    moved_selector["expected_uuid"] = Value::Null;
    let second = envelope(
        "item.set",
        OperationV1::Mutate,
        json!({"selector": moved_selector, "item_id": "note", "value": "one"}),
        item_preconditions(),
        EnvelopeTransport {
            detach: true,
            wait_timeout_ms: 7,
            workspace_root: "/different legacy root",
            request_id: "77777777-7777-4777-8777-777777777777",
            client_name: "other-client",
        },
    );
    let changed_payload = envelope(
        "item.set",
        OperationV1::Mutate,
        json!({"selector": selector_json(b"/first-root", "/first diagnostic"), "item_id": "note", "value": "two"}),
        item_preconditions(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/first legacy root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    let changed_precondition = envelope(
        "item.set",
        OperationV1::Mutate,
        json!({"selector": selector_json(b"/first-root", "/first diagnostic"), "item_id": "note", "value": "one"}),
        PreconditionsV1::new(
            None,
            None,
            Some(ATTEMPT_ID.to_owned().try_into().unwrap()),
            Some(Revision::new(3)),
            None,
            None,
        )
        .unwrap(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/first legacy root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );

    let workspace_id = WorkspaceId::new(WORKSPACE_ID).unwrap();
    let first = SliceRequestV1::from_envelope(&first).unwrap();
    let second = SliceRequestV1::from_envelope(&second).unwrap();
    let changed_payload = SliceRequestV1::from_envelope(&changed_payload).unwrap();
    let changed_precondition = SliceRequestV1::from_envelope(&changed_precondition).unwrap();

    let first_identity = canonical_mutation_identity_v1(&first, &workspace_id).unwrap();
    assert_eq!(
        first_identity,
        canonical_mutation_identity_v1(&second, &workspace_id).unwrap()
    );
    assert_ne!(
        first_identity,
        canonical_mutation_identity_v1(&changed_payload, &workspace_id).unwrap()
    );
    assert_ne!(
        first_identity,
        canonical_mutation_identity_v1(&changed_precondition, &workspace_id).unwrap()
    );
    assert_eq!(
        canonical_mutation_identity_bytes_v1(&first, &workspace_id).unwrap(),
        first_identity.as_bytes(),
    );

    let query = envelope(
        "session.status",
        OperationV1::Query,
        json!({"selector": selector_json(b"/first-root", "/first diagnostic")}),
        PreconditionsV1::default(),
        EnvelopeTransport {
            detach: false,
            wait_timeout_ms: 0,
            workspace_root: "/first legacy root",
            request_id: REQUEST_ID,
            client_name: "podway-cli",
        },
    );
    assert!(
        canonical_mutation_identity_v1(
            &SliceRequestV1::from_envelope(&query).unwrap(),
            &workspace_id,
        )
        .is_err()
    );
}

#[test]
fn status_and_next_results_round_trip_with_active_item_values_and_redo_evidence() {
    let status = json!({
        "task": {
            "title": "Fix duplicate login session creation",
            "procedure": {
                "id": "bug-fix",
                "version": "1",
                "name": "Bug Fix",
                "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
            }
        },
        "session": {
            "id": SESSION_ID,
            "lifecycle": "running",
            "revision": 17,
            "created_at": "2026-07-13T03:00:00.000Z",
            "completed_at": null,
            "cancelled_at": null
        },
        "current": {
            "stage_id": "verify",
            "stage_index": 4,
            "title": "Verify the fix",
            "attempt_id": ATTEMPT_ID,
            "attempt_number": 2,
            "blocked": false,
            "ready_to_complete": false
        },
        "stages": [
            {"id": "verify", "index": 4, "title": "Verify the fix", "status": "current", "latest_attempt_number": 2},
            {"id": "review", "index": 5, "title": "Review", "status": "redo", "latest_attempt_number": 1}
        ],
        "items": [
            {"id": "confirmed", "type": "confirm", "prompt": "Confirmed", "required": true, "satisfied": true, "revision": 1, "value": true},
            {"id": "note", "type": "text", "prompt": "Note", "required": true, "satisfied": false, "revision": 0, "value": null}
        ],
        "blockers": [{"id": BLOCKER_ID, "attempt_id": ATTEMPT_ID, "reason": "Need review"}],
        "queue": {"pending_mutations": true, "queued_count": 2, "running_job_id": JOB_ID, "latest_workspace_sequence": 41}
    });
    let status_map = payload(status);
    let parsed_status = StatusResultV1::from_result_map(&status_map).unwrap();
    assert_eq!(parsed_status.to_result_map(), status_map);
    assert!(parsed_status.current.is_some());
    assert_eq!(parsed_status.items[0].value, json!(true));
    assert!(
        parsed_status
            .stages
            .iter()
            .any(|stage| { stage.status == podway_protocol::StageStatusResultV1::Redo })
    );

    let next = json!({
        "stage": {
            "id": "verify",
            "title": "Verify the fix",
            "attempt_id": ATTEMPT_ID,
            "attempt_number": 2,
            "instructions": ["Run the regression test."]
        },
        "missing_required_items": [{"id": "note", "type": "text", "prompt": "Note"}],
        "blockers": [{"id": BLOCKER_ID, "attempt_id": ATTEMPT_ID, "reason": "Need review"}],
        "allowed_actions": {"complete": false, "skip": false, "retry": true, "return_to": ["diagnose"], "cancel": true},
        "next_stage_after_completion": {"id": "review", "title": "Review"},
        "suggestions": [{"command": "item.set", "argv": ["podway", "set", "note", "<text>"], "item_id": "note"}]
    });
    let next_map = payload(next);
    let parsed_next = NextResultV1::from_result_map(&next_map).unwrap();
    assert_eq!(parsed_next.to_result_map(), next_map);
    assert_eq!(parsed_next.stage.unwrap().attempt_id.as_str(), ATTEMPT_ID);
    assert_eq!(
        parsed_next.suggestions[0]
            .item_id
            .as_ref()
            .unwrap()
            .as_str(),
        "note"
    );
}

#[test]
fn status_and_next_records_are_strict_schema_shaped_objects() {
    let malformed_status = json!({
        "task": {"title": "Task", "procedure": {"id": "p", "version": "1", "name": "P", "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"}},
        "session": {"id": SESSION_ID, "lifecycle": "running", "revision": 1, "created_at": "2026-07-13T03:00:00.000Z", "completed_at": null, "cancelled_at": null},
        "current": null,
        "stages": [], "items": [], "blockers": [],
        "queue": {"pending_mutations": false, "queued_count": 0, "running_job_id": null, "latest_workspace_sequence": 0},
        "unknown": true
    });
    let mut malformed_status = payload(malformed_status);
    assert!(StatusResultV1::from_result_map(&malformed_status).is_err());
    malformed_status.remove("unknown");
    malformed_status
        .get_mut("session")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("revision".to_owned(), json!(0));
    assert!(StatusResultV1::from_result_map(&malformed_status).is_err());

    let malformed_next = json!({
        "stage": null,
        "missing_required_items": [],
        "blockers": [],
        "allowed_actions": {"complete": false, "skip": false, "retry": false, "return_to": [], "cancel": false, "unknown": true},
        "next_stage_after_completion": null,
        "suggestions": []
    });
    assert!(NextResultV1::from_result_map(&payload(malformed_next)).is_err());
}
