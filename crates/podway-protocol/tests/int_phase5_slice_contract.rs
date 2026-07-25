use podway_core::{ItemId, Revision, Sha256Digest, WorkspaceId};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, DAEMON_COMMAND_NAMES_V1, ErrorCodeV1, ExitCodeV1,
    IdempotencyKeyV1, JobStateV1, OperationV1, PreconditionsV1, RequestEnvelopeInputV1,
    RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, SliceCommandV1, SliceErrorV1, SliceRequestV1,
    TerminalJobCancellationProjectionV1, TerminalJobErrorProjectionV1, TerminalJobResponseV1,
    TerminalJobSuccessProjectionV1, TerminalJobSuccessResultV1, WorkspaceContextV1,
    WorktreeSelectorWireV1, canonical_mutation_identity_v1, canonical_reset_all_identity_v1,
};
use serde_json::{Map, Value, json};

const REQUEST_ID: &str = "11111111-1111-4111-8111-111111111111";
const ATTEMPT_ID: &str = "22222222-2222-4222-8222-222222222222";
const BLOCKER_ID: &str = "33333333-3333-4333-8333-333333333333";
const WORKSPACE_ID: &str = "44444444-4444-4444-8444-444444444444";
const SESSION_ID: &str = "55555555-5555-4555-8555-555555555555";
const JOB_ID: &str = "66666666-6666-4666-8666-666666666666";
const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_WORKSPACE_ID: &str = "77777777-7777-4777-8777-777777777777";
const OTHER_SESSION_ID: &str = "88888888-8888-4888-8888-888888888888";

fn selector() -> Value {
    serde_json::to_value(
        WorktreeSelectorWireV1::new(
            b"/worktree",
            "/worktree",
            Some(WorkspaceId::new(WORKSPACE_ID).unwrap()),
        )
        .unwrap(),
    )
    .unwrap()
}

fn payload(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn item_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(
        Some(SESSION_ID.to_owned().try_into().unwrap()),
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
        Some(SESSION_ID.to_owned().try_into().unwrap()),
        Some(Revision::new(3)),
        Some(ATTEMPT_ID.to_owned().try_into().unwrap()),
        None,
        None,
        None,
    )
    .unwrap()
}

fn session_identity_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(
        Some(SESSION_ID.to_owned().try_into().unwrap()),
        Some(Revision::new(3)),
        None,
        None,
        None,
        None,
    )
    .unwrap()
}

fn session_revision_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(
        Some(SESSION_ID.to_owned().try_into().unwrap()),
        Some(Revision::new(3)),
        None,
        None,
        None,
        None,
    )
    .unwrap()
}

fn job_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(None, None, None, None, None, Some(JobStateV1::Queued)).unwrap()
}

fn envelope(
    command: &str,
    operation: OperationV1,
    durable: bool,
    command_payload: Value,
    preconditions: PreconditionsV1,
) -> RequestEnvelopeV1 {
    envelope_with_workspace_uuid(
        command,
        operation,
        durable,
        command_payload,
        preconditions,
        Some(WorkspaceId::new(WORKSPACE_ID).unwrap()),
    )
}

fn envelope_with_workspace_uuid(
    command: &str,
    operation: OperationV1,
    durable: bool,
    command_payload: Value,
    preconditions: PreconditionsV1,
    workspace_uuid: Option<WorkspaceId>,
) -> RequestEnvelopeV1 {
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        client: ClientInfoV1::new("podway-cli", "1.0.0", 42).unwrap(),
        operation,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(WorkspaceContextV1::new("/worktree", workspace_uuid).unwrap()),
        idempotency_key: durable.then(|| IdempotencyKeyV1::new("phase5-contract").unwrap()),
        preconditions,
        options: RequestOptionsV1::new(false, 30_000).unwrap(),
        payload: payload(command_payload),
    })
    .unwrap()
}

#[test]
fn casid003_rejects_mismatched_envelope_and_selector_workspace_uuids() {
    let request = envelope_with_workspace_uuid(
        "session.status",
        OperationV1::Query,
        false,
        json!({"selector": selector()}),
        PreconditionsV1::default(),
        Some(WorkspaceId::new(OTHER_WORKSPACE_ID).unwrap()),
    );

    assert_eq!(
        SliceRequestV1::from_envelope(&request),
        Err(SliceErrorV1::InvalidValue {
            field: "workspace.expected_uuid/selector.expected_uuid",
        }),
    );
}

struct RouteCase {
    command: &'static str,
    operation: OperationV1,
    durable: bool,
    payload: Value,
    preconditions: PreconditionsV1,
}

fn route_cases() -> Vec<RouteCase> {
    let selector = selector();
    vec![
        RouteCase {
            command: "workspace.init",
            operation: OperationV1::Bootstrap,
            durable: true,
            payload: json!({"selector": selector.clone(), "repair": false}),
            preconditions: PreconditionsV1::default(),
        },
        RouteCase {
            command: "workspace.doctor",
            operation: OperationV1::Query,
            durable: false,
            payload: json!({"selector": selector.clone(), "deep": true}),
            preconditions: PreconditionsV1::default(),
        },
        RouteCase {
            command: "workspace.show",
            operation: OperationV1::Query,
            durable: false,
            payload: json!({"selector": selector.clone()}),
            preconditions: PreconditionsV1::default(),
        },
        RouteCase {
            command: "workspace.repair",
            operation: OperationV1::Control,
            durable: false,
            payload: json!({"selector": selector.clone()}),
            preconditions: PreconditionsV1::default(),
        },
        RouteCase {
            command: "session.start",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "preset": "bug-fix", "task_title": "Fix the login"}),
            preconditions: PreconditionsV1::default(),
        },
        RouteCase {
            command: "session.start_replace",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "procedure": "procedures/custom.yaml", "task_title": "Replace the task", "confirmed": true}),
            preconditions: session_identity_preconditions(),
        },
        RouteCase {
            command: "session.status",
            operation: OperationV1::Query,
            durable: false,
            payload: json!({"selector": selector.clone(), "after_job_id": JOB_ID, "verbose": true}),
            preconditions: PreconditionsV1::default(),
        },
        RouteCase {
            command: "session.next",
            operation: OperationV1::Query,
            durable: false,
            payload: json!({"selector": selector.clone(), "wait_for_idle": true}),
            preconditions: PreconditionsV1::default(),
        },
        RouteCase {
            command: "session.complete",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone()}),
            preconditions: session_preconditions(),
        },
        RouteCase {
            command: "session.skip",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "reason": "No longer applicable"}),
            preconditions: session_preconditions(),
        },
        RouteCase {
            command: "session.retry",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "reason": "Repair the test"}),
            preconditions: session_preconditions(),
        },
        RouteCase {
            command: "session.return",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "destination_stage_id": "diagnose", "reason": "Need diagnosis"}),
            preconditions: session_preconditions(),
        },
        RouteCase {
            command: "session.block",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "reason": "Waiting for review"}),
            preconditions: session_preconditions(),
        },
        RouteCase {
            command: "session.unblock",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "blocker_id": BLOCKER_ID, "all": false}),
            preconditions: session_preconditions(),
        },
        RouteCase {
            command: "session.cancel",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "reason": "No longer needed"}),
            preconditions: session_preconditions(),
        },
        RouteCase {
            command: "session.reopen",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "destination_stage_id": "verify", "reason": "New failure"}),
            preconditions: session_revision_preconditions(),
        },
        RouteCase {
            command: "session.reset",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "confirmed": true}),
            preconditions: session_identity_preconditions(),
        },
        RouteCase {
            command: "workspace.reset_all",
            operation: OperationV1::Bootstrap,
            durable: true,
            payload: json!({"selector": selector.clone(), "confirmed": true, "expected_workspace_uuid": WORKSPACE_ID}),
            preconditions: PreconditionsV1::default(),
        },
        RouteCase {
            command: "item.check",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "item_id": "checked"}),
            preconditions: item_preconditions(),
        },
        RouteCase {
            command: "item.uncheck",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "item_id": "checked"}),
            preconditions: item_preconditions(),
        },
        RouteCase {
            command: "item.set",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "item_id": "note", "value": "verified"}),
            preconditions: item_preconditions(),
        },
        RouteCase {
            command: "item.add",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "item_id": "files", "value": "src/lib.rs"}),
            preconditions: item_preconditions(),
        },
        RouteCase {
            command: "item.remove",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "item_id": "files", "value": "src/lib.rs", "ignore_missing": true}),
            preconditions: item_preconditions(),
        },
        RouteCase {
            command: "item.attach",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "item_id": "artifact", "path": "proof/report.txt", "media_type": "text/plain"}),
            preconditions: item_preconditions(),
        },
        RouteCase {
            command: "item.clear",
            operation: OperationV1::Mutate,
            durable: true,
            payload: json!({"selector": selector.clone(), "item_id": "artifact"}),
            preconditions: item_preconditions(),
        },
        RouteCase {
            command: "job.list",
            operation: OperationV1::Query,
            durable: false,
            payload: json!({"selector": selector.clone(), "state": "queued"}),
            preconditions: PreconditionsV1::default(),
        },
        RouteCase {
            command: "job.status",
            operation: OperationV1::Query,
            durable: false,
            payload: json!({"selector": selector.clone(), "job_id": JOB_ID}),
            preconditions: PreconditionsV1::default(),
        },
        RouteCase {
            command: "job.wait",
            operation: OperationV1::Query,
            durable: false,
            payload: json!({"selector": selector.clone(), "job_id": JOB_ID}),
            preconditions: PreconditionsV1::default(),
        },
        RouteCase {
            command: "job.cancel",
            operation: OperationV1::Control,
            durable: false,
            payload: json!({"selector": selector, "job_id": JOB_ID}),
            preconditions: job_preconditions(),
        },
    ]
}

#[test]
fn g006_exhaustively_admits_only_the_29_canonical_daemon_routes() {
    let cases = route_cases();
    assert_eq!(cases.len(), 29);
    assert_eq!(DAEMON_COMMAND_NAMES_V1.len(), 29);
    assert_eq!(
        cases.iter().map(|case| case.command).collect::<Vec<_>>(),
        DAEMON_COMMAND_NAMES_V1.to_vec(),
    );

    let workspace_id = WorkspaceId::new(WORKSPACE_ID).unwrap();
    for case in cases {
        let request = SliceRequestV1::from_envelope(&envelope(
            case.command,
            case.operation,
            case.durable,
            case.payload.clone(),
            case.preconditions.clone(),
        ))
        .unwrap();
        assert_eq!(request.command().command_name(), case.command);
        assert_eq!(request.command().operation(), case.operation);
        assert_eq!(request.command().is_durable_job(), case.durable);

        let mut unknown_payload = case.payload.as_object().unwrap().clone();
        unknown_payload.insert("unexpected".to_owned(), json!(true));
        assert!(
            SliceRequestV1::from_envelope(&envelope(
                case.command,
                case.operation,
                case.durable,
                Value::Object(unknown_payload),
                case.preconditions.clone(),
            ))
            .is_err()
        );

        if case.durable {
            let identity = canonical_mutation_identity_v1(&request, &workspace_id).unwrap();
            assert!(identity.contains(case.command));
        } else {
            assert!(canonical_mutation_identity_v1(&request, &workspace_id).is_err());
        }
    }
    let init_repair = SliceRequestV1::from_envelope(&envelope(
        "workspace.init",
        OperationV1::Bootstrap,
        true,
        json!({"selector": selector(), "repair": true}),
        PreconditionsV1::default(),
    ))
    .unwrap();
    match init_repair.command() {
        podway_protocol::SliceCommandV1::WorkspaceInit(input) => assert!(input.repair),
        command => panic!("unexpected command {}", command.command_name()),
    }
}

#[test]
fn g006_rejects_aliases_optional_field_bags_and_incompatible_waits() {
    let no_preconditions = PreconditionsV1::default();
    let aliases = [
        (
            "preset.start",
            json!({"selector": selector(), "preset": "bug-fix", "task_title": "Fix it"}),
        ),
        (
            "item.attach_path",
            json!({"selector": selector(), "item_id": "artifact", "path": "proof/report.txt"}),
        ),
    ];
    for (alias, command_payload) in aliases {
        assert!(
            SliceRequestV1::from_envelope(&envelope(
                alias,
                OperationV1::Mutate,
                true,
                command_payload,
                no_preconditions.clone(),
            ))
            .is_err()
        );
    }

    let ambiguous_start = envelope(
        "session.start",
        OperationV1::Mutate,
        true,
        json!({
            "selector": selector(),
            "preset": "bug-fix",
            "procedure": "procedures/custom.yaml",
            "task_title": "Ambiguous",
        }),
        PreconditionsV1::default(),
    );
    assert!(SliceRequestV1::from_envelope(&ambiguous_start).is_err());

    let incomplete_reference = envelope(
        "item.attach",
        OperationV1::Mutate,
        true,
        json!({
            "selector": selector(),
            "item_id": "artifact",
            "reference": "artifact://proof",
            "digest": DIGEST,
            "media_type": "application/json",
        }),
        item_preconditions(),
    );
    assert!(SliceRequestV1::from_envelope(&incomplete_reference).is_err());
    let reference_attach = envelope(
        "item.attach",
        OperationV1::Mutate,
        true,
        json!({
            "selector": selector(),
            "item_id": "artifact",
            "reference": "artifact://proof",
            "digest": DIGEST,
            "size_bytes": 42,
            "media_type": "application/json",
        }),
        item_preconditions(),
    );
    assert_eq!(
        SliceRequestV1::from_envelope(&reference_attach)
            .unwrap()
            .command()
            .command_name(),
        "item.attach",
    );

    let conflicting_waits = envelope(
        "session.status",
        OperationV1::Query,
        false,
        json!({
            "selector": selector(),
            "wait_for_idle": true,
            "after_job_id": JOB_ID,
        }),
        PreconditionsV1::default(),
    );
    assert!(SliceRequestV1::from_envelope(&conflicting_waits).is_err());
}

#[test]
fn g006_enforces_route_specific_preconditions_and_confirmation() {
    let missing_item_precondition = envelope(
        "item.uncheck",
        OperationV1::Mutate,
        true,
        json!({"selector": selector(), "item_id": "checked"}),
        PreconditionsV1::default(),
    );
    assert!(SliceRequestV1::from_envelope(&missing_item_precondition).is_err());

    let missing_session_identity = envelope(
        "session.reset",
        OperationV1::Mutate,
        true,
        json!({"selector": selector(), "confirmed": true}),
        session_preconditions(),
    );
    assert!(SliceRequestV1::from_envelope(&missing_session_identity).is_err());

    let missing_job_state = envelope(
        "job.cancel",
        OperationV1::Control,
        false,
        json!({"selector": selector(), "job_id": JOB_ID}),
        PreconditionsV1::default(),
    );
    assert!(SliceRequestV1::from_envelope(&missing_job_state).is_err());

    let unconfirmed_replace = envelope(
        "session.start_replace",
        OperationV1::Mutate,
        true,
        json!({
            "selector": selector(),
            "preset": "bug-fix",
            "task_title": "Replace",
            "confirmed": false,
        }),
        session_identity_preconditions(),
    );
    assert!(SliceRequestV1::from_envelope(&unconfirmed_replace).is_err());

    let extra_query_precondition = envelope(
        "job.status",
        OperationV1::Query,
        false,
        json!({"selector": selector(), "job_id": JOB_ID}),
        job_preconditions(),
    );
    assert!(SliceRequestV1::from_envelope(&extra_query_precondition).is_err());
}

#[test]
fn casid002_preserves_session_identity_for_reads_and_canonical_mutations() {
    let guarded_read = SliceRequestV1::from_envelope(&envelope(
        "session.status",
        OperationV1::Query,
        false,
        json!({"selector": selector()}),
        PreconditionsV1::new(
            Some(SESSION_ID.to_owned().try_into().unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    ))
    .unwrap();
    match guarded_read.command() {
        podway_protocol::SliceCommandV1::SessionStatus(status) => assert_eq!(
            status
                .preconditions
                .expected_session_id
                .as_ref()
                .unwrap()
                .as_str(),
            SESSION_ID,
        ),
        command => panic!("unexpected command {}", command.command_name()),
    }

    let first = SliceRequestV1::from_envelope(&envelope(
        "session.complete",
        OperationV1::Mutate,
        true,
        json!({"selector": selector()}),
        session_preconditions(),
    ))
    .unwrap();
    let second = SliceRequestV1::from_envelope(&envelope(
        "session.complete",
        OperationV1::Mutate,
        true,
        json!({"selector": selector()}),
        PreconditionsV1::new(
            Some(OTHER_SESSION_ID.to_owned().try_into().unwrap()),
            Some(Revision::new(3)),
            Some(ATTEMPT_ID.to_owned().try_into().unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
    ))
    .unwrap();
    let workspace_id = WorkspaceId::new(WORKSPACE_ID).unwrap();
    let first: Value =
        serde_json::from_str(&canonical_mutation_identity_v1(&first, &workspace_id).unwrap())
            .unwrap();
    assert_eq!(first["preconditions"]["session_id"], SESSION_ID);
    assert_ne!(
        first,
        serde_json::from_str::<Value>(
            &canonical_mutation_identity_v1(&second, &workspace_id).unwrap(),
        )
        .unwrap(),
    );
}
#[test]
fn g006_dry_runs_are_query_only_and_excluded_from_mutation_identity() {
    let workspace_id = WorkspaceId::new(WORKSPACE_ID).unwrap();
    let cases = vec![
        (
            "session.start",
            PreconditionsV1::default(),
            json!({
                "selector": selector(),
                "preset": "bug-fix",
                "task_title": "Preview start",
                "dry_run": true,
            }),
            json!({
                "selector": selector(),
                "preset": "bug-fix",
                "task_title": "Preview start",
            }),
        ),
        (
            "session.start_replace",
            session_identity_preconditions(),
            json!({
                "selector": selector(),
                "preset": "bug-fix",
                "task_title": "Preview replace",
                "dry_run": true,
            }),
            json!({
                "selector": selector(),
                "preset": "bug-fix",
                "task_title": "Preview replace",
                "confirmed": true,
            }),
        ),
        (
            "session.return",
            session_preconditions(),
            json!({
                "selector": selector(),
                "destination_stage_id": "diagnose",
                "reason": "Preview return",
                "dry_run": true,
            }),
            json!({
                "selector": selector(),
                "destination_stage_id": "diagnose",
                "reason": "Preview return",
            }),
        ),
        (
            "session.reopen",
            session_revision_preconditions(),
            json!({
                "selector": selector(),
                "destination_stage_id": "verify",
                "reason": "Preview reopen",
                "dry_run": true,
            }),
            json!({
                "selector": selector(),
                "destination_stage_id": "verify",
                "reason": "Preview reopen",
            }),
        ),
        (
            "session.reset",
            session_identity_preconditions(),
            json!({"selector": selector(), "dry_run": true}),
            json!({"selector": selector(), "confirmed": true}),
        ),
    ];

    for (command, preconditions, dry_run_payload, mutation_payload) in cases {
        let dry_run = SliceRequestV1::from_envelope(&envelope(
            command,
            OperationV1::Query,
            false,
            dry_run_payload.clone(),
            preconditions.clone(),
        ))
        .unwrap();
        assert_eq!(dry_run.command().operation(), OperationV1::Query);
        assert!(!dry_run.command().is_durable_job());
        assert!(canonical_mutation_identity_v1(&dry_run, &workspace_id).is_err());

        assert!(
            SliceRequestV1::from_envelope(&envelope(
                command,
                OperationV1::Query,
                false,
                mutation_payload.clone(),
                preconditions.clone(),
            ))
            .is_err()
        );
        assert!(
            SliceRequestV1::from_envelope(&envelope(
                command,
                OperationV1::Mutate,
                true,
                dry_run_payload,
                preconditions.clone(),
            ))
            .is_err()
        );

        let mutation = SliceRequestV1::from_envelope(&envelope(
            command,
            OperationV1::Mutate,
            true,
            mutation_payload.clone(),
            preconditions.clone(),
        ))
        .unwrap();
        let mut explicit_false = mutation_payload.as_object().unwrap().clone();
        explicit_false.insert("dry_run".to_owned(), json!(false));
        let explicit_false = SliceRequestV1::from_envelope(&envelope(
            command,
            OperationV1::Mutate,
            true,
            Value::Object(explicit_false),
            preconditions,
        ))
        .unwrap();
        assert_eq!(
            canonical_mutation_identity_v1(&mutation, &workspace_id).unwrap(),
            canonical_mutation_identity_v1(&explicit_false, &workspace_id).unwrap(),
        );
    }

    let reset_with_confirmation = envelope(
        "session.reset",
        OperationV1::Query,
        false,
        json!({"selector": selector(), "dry_run": true, "confirmed": true}),
        session_identity_preconditions(),
    );
    assert!(SliceRequestV1::from_envelope(&reset_with_confirmation).is_err());
    let replace_with_confirmation = envelope(
        "session.start_replace",
        OperationV1::Query,
        false,
        json!({
            "selector": selector(),
            "preset": "bug-fix",
            "task_title": "Preview replace",
            "dry_run": true,
            "confirmed": true,
        }),
        session_identity_preconditions(),
    );
    assert!(SliceRequestV1::from_envelope(&replace_with_confirmation).is_err());

    let malformed_dry_run = envelope(
        "session.return",
        OperationV1::Query,
        false,
        json!({
            "selector": selector(),
            "destination_stage_id": "diagnose",
            "reason": "Preview return",
            "dry_run": "true",
        }),
        session_preconditions(),
    );
    assert!(SliceRequestV1::from_envelope(&malformed_dry_run).is_err());
}

#[test]
fn pstrt001_parses_only_canonical_procedure_digest_guards() {
    let guarded = envelope(
        "session.start",
        OperationV1::Mutate,
        true,
        json!({
            "selector": selector(),
            "procedure": "procedures/custom.yaml",
            "expected_procedure_digest": DIGEST,
            "task_title": "Guarded start",
        }),
        PreconditionsV1::default(),
    );
    let guarded = SliceRequestV1::from_envelope(&guarded).expect("guarded start must parse");
    let SliceCommandV1::SessionStart(start) = guarded.command() else {
        panic!("guarded request must remain a session start");
    };
    assert_eq!(
        start
            .expected_procedure_digest
            .as_ref()
            .expect("guard must be preserved")
            .as_str(),
        DIGEST,
    );
    for payload in [
        json!({
            "selector": selector(),
            "preset": "bug-fix",
            "expected_procedure_digest": DIGEST,
            "task_title": "Preset guard is unsupported",
        }),
        json!({
            "selector": selector(),
            "procedure": "procedures/custom.yaml",
            "expected_procedure_digest": "sha256:ABC",
            "task_title": "Malformed guard",
        }),
    ] {
        assert!(
            SliceRequestV1::from_envelope(&envelope(
                "session.start",
                OperationV1::Mutate,
                true,
                payload,
                PreconditionsV1::default(),
            ))
            .is_err()
        );
    }
}
#[test]
fn g006_reset_all_preserves_workspace_uuid_preconditions_and_selector_consistency() {
    let request = SliceRequestV1::from_envelope(&envelope(
        "workspace.reset_all",
        OperationV1::Bootstrap,
        true,
        json!({
            "selector": selector(),
            "confirmed": true,
            "expected_workspace_uuid": WORKSPACE_ID,
        }),
        PreconditionsV1::default(),
    ))
    .unwrap();
    match request.command() {
        podway_protocol::SliceCommandV1::WorkspaceResetAll(command) => {
            assert_eq!(
                command
                    .preconditions
                    .expected_workspace_id
                    .as_ref()
                    .unwrap()
                    .as_str(),
                WORKSPACE_ID,
            );
        }
        command => panic!("unexpected command {}", command.command_name()),
    }
    let canonical: Value = serde_json::from_str(
        &canonical_mutation_identity_v1(&request, &WorkspaceId::new(WORKSPACE_ID).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        canonical["preconditions"],
        json!({"workspace_id": WORKSPACE_ID})
    );
    let mut unreadable_selector = selector();
    unreadable_selector["expected_uuid"] = Value::Null;
    let unreadable_request = SliceRequestV1::from_envelope(&envelope_with_workspace_uuid(
        "workspace.reset_all",
        OperationV1::Bootstrap,
        true,
        json!({"selector": unreadable_selector, "confirmed": true}),
        PreconditionsV1::default(),
        None,
    ))
    .unwrap();
    match unreadable_request.command() {
        podway_protocol::SliceCommandV1::WorkspaceResetAll(command) => {
            assert!(command.preconditions.expected_workspace_id.is_none());
        }
        command => panic!("unexpected command {}", command.command_name()),
    }
    let unreadable_canonical: Value = serde_json::from_str(
        &canonical_mutation_identity_v1(
            &unreadable_request,
            &WorkspaceId::new(WORKSPACE_ID).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        unreadable_canonical["preconditions"],
        json!({"workspace_id": null})
    );

    let missing_semantic_uuid = envelope(
        "workspace.reset_all",
        OperationV1::Bootstrap,
        true,
        json!({"selector": selector(), "confirmed": true}),
        PreconditionsV1::default(),
    );
    assert!(SliceRequestV1::from_envelope(&missing_semantic_uuid).is_err());

    let mut mismatched_selector = selector();
    mismatched_selector["expected_uuid"] = json!(OTHER_WORKSPACE_ID);
    let mismatch = envelope(
        "workspace.reset_all",
        OperationV1::Bootstrap,
        true,
        json!({
            "selector": mismatched_selector,
            "confirmed": true,
            "expected_workspace_uuid": WORKSPACE_ID,
        }),
        PreconditionsV1::default(),
    );
    assert!(SliceRequestV1::from_envelope(&mismatch).is_err());

    let explicit_null = envelope(
        "workspace.reset_all",
        OperationV1::Bootstrap,
        true,
        json!({
            "selector": selector(),
            "confirmed": true,
            "expected_workspace_uuid": null,
        }),
        PreconditionsV1::default(),
    );
    assert!(SliceRequestV1::from_envelope(&explicit_null).is_err());
}
#[test]
fn g006_reset_all_identity_binds_stable_git_fingerprints_not_workspace_uuids() {
    let common_dir_identity = Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap();
    let worktree_admin_identity = Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap();
    let changed_common_dir_identity =
        Sha256Digest::new(format!("sha256:{}", "c".repeat(64))).unwrap();
    let changed_worktree_admin_identity =
        Sha256Digest::new(format!("sha256:{}", "d".repeat(64))).unwrap();

    let first = SliceRequestV1::from_envelope(&envelope(
        "workspace.reset_all",
        OperationV1::Bootstrap,
        true,
        json!({
            "selector": selector(),
            "confirmed": true,
            "expected_workspace_uuid": WORKSPACE_ID,
        }),
        PreconditionsV1::default(),
    ))
    .unwrap();

    let mut replacement_selector = selector();
    replacement_selector["expected_uuid"] = json!(OTHER_WORKSPACE_ID);
    let replacement = SliceRequestV1::from_envelope(&envelope_with_workspace_uuid(
        "workspace.reset_all",
        OperationV1::Bootstrap,
        true,
        json!({
            "selector": replacement_selector,
            "confirmed": true,
            "expected_workspace_uuid": OTHER_WORKSPACE_ID,
        }),
        PreconditionsV1::default(),
        Some(WorkspaceId::new(OTHER_WORKSPACE_ID).unwrap()),
    ))
    .unwrap();

    let first_identity =
        canonical_reset_all_identity_v1(&first, &common_dir_identity, &worktree_admin_identity)
            .unwrap();
    assert_eq!(
        first_identity,
        canonical_reset_all_identity_v1(
            &replacement,
            &common_dir_identity,
            &worktree_admin_identity,
        )
        .unwrap(),
    );
    assert_ne!(
        first_identity,
        canonical_reset_all_identity_v1(
            &first,
            &changed_common_dir_identity,
            &worktree_admin_identity,
        )
        .unwrap(),
    );
    assert_ne!(
        first_identity,
        canonical_reset_all_identity_v1(
            &first,
            &common_dir_identity,
            &changed_worktree_admin_identity,
        )
        .unwrap(),
    );

    let canonical: Value = serde_json::from_str(&first_identity).unwrap();
    assert_eq!(canonical["protocol_major"], 1);
    assert_eq!(canonical["command"], "workspace.reset_all");
    assert_eq!(
        canonical["common_dir_identity"],
        common_dir_identity.as_str()
    );
    assert_eq!(
        canonical["worktree_admin_identity"],
        worktree_admin_identity.as_str()
    );
    assert_eq!(canonical["payload"], json!({"confirmed": true}));
    assert!(canonical.get("workspace_id").is_none());
    assert!(!first_identity.contains(WORKSPACE_ID));
    assert!(!first_identity.contains(OTHER_WORKSPACE_ID));

    let other_mutation = SliceRequestV1::from_envelope(&envelope(
        "session.reset",
        OperationV1::Mutate,
        true,
        json!({"selector": selector(), "confirmed": true}),
        session_identity_preconditions(),
    ))
    .unwrap();
    assert!(
        canonical_reset_all_identity_v1(
            &other_mutation,
            &common_dir_identity,
            &worktree_admin_identity,
        )
        .is_err()
    );

    let query = SliceRequestV1::from_envelope(&envelope(
        "session.status",
        OperationV1::Query,
        false,
        json!({"selector": selector()}),
        PreconditionsV1::default(),
    ))
    .unwrap();
    assert!(
        canonical_reset_all_identity_v1(&query, &common_dir_identity, &worktree_admin_identity)
            .is_err()
    );

    let unconfirmed = envelope(
        "workspace.reset_all",
        OperationV1::Bootstrap,
        true,
        json!({
            "selector": selector(),
            "confirmed": false,
            "expected_workspace_uuid": WORKSPACE_ID,
        }),
        PreconditionsV1::default(),
    );
    assert!(SliceRequestV1::from_envelope(&unconfirmed).is_err());
}
#[test]
fn g006_terminal_job_responses_are_protocol_owned_typed_projections() {
    let success = TerminalJobResponseV1::Success(TerminalJobSuccessProjectionV1 {
        result: TerminalJobSuccessResultV1::ItemChanged {
            item_id: ItemId::new("proof").unwrap(),
            changed: true,
            revision_before: Revision::new(4),
            revision_after: Revision::new(5),
        },
        session: None,
    });
    assert_eq!(
        serde_json::to_value(success).unwrap(),
        json!({
            "kind": "success",
            "payload": {
                "result": {
                    "kind": "item_changed",
                    "item_id": "proof",
                    "changed": true,
                    "revision_before": 4,
                    "revision_after": 5
                },
                "session": null
            }
        })
    );

    let error = TerminalJobResponseV1::Error(TerminalJobErrorProjectionV1 {
        code: ErrorCodeV1::new("ITEM_REVISION_CONFLICT").unwrap(),
        message: "The item changed after it was observed.".to_owned(),
        retryable: true,
        exit_code: ExitCodeV1::new(4).unwrap(),
        details: Map::from_iter([
            ("expected_revision".to_owned(), json!(4)),
            ("current_revision".to_owned(), json!(5)),
        ]),
    });
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        json!({
            "kind": "error",
            "payload": {
                "code": "ITEM_REVISION_CONFLICT",
                "message": "The item changed after it was observed.",
                "retryable": true,
                "exit_code": 4,
                "details": {
                    "expected_revision": 4,
                    "current_revision": 5
                }
            }
        })
    );

    let cancelled =
        TerminalJobResponseV1::Cancelled(TerminalJobCancellationProjectionV1 { cancelled: true });
    assert_eq!(
        serde_json::to_value(cancelled).unwrap(),
        json!({"kind": "cancelled", "payload": {"cancelled": true}})
    );
}
