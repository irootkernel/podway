use std::{collections::BTreeSet, path::Path};

use podway_protocol::{
    ContractGenerationV1, DAEMON_FREE_DOCUMENT_SCHEMAS_V1, ErrorEnvelopeV2, OutputEnvelopeV4,
    RELEASED_CONTRACT_MANIFESTS_V1, released_writer_release_v1, result_schema_top_level_fields_v2,
    validate_daemon_free_document_v1,
};
use serde_json::{Value, json};

const UUID: &str = "00000000-0000-4000-8000-000000000001";
const OTHER_UUID: &str = "00000000-0000-4000-8000-000000000002";
const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const V0_2_13_DIGEST: &str =
    "sha256:dc47bcc5449f7c2e60c920c5b820eb793405b5f2d2f573346ee326f90264d2aa";
const V0_2_14_DIGEST: &str =
    "sha256:c699977dd232891041f4063d66295b94004702875855baace876f8a7e0c6391d";

fn fixtures() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/protocol/current-contracts.json"
    ))
    .unwrap()
}

fn read_schema(relative: &str) -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    serde_json::from_slice(&std::fs::read(root.join("assets").join(relative)).unwrap()).unwrap()
}

fn output_envelope(result: &Value) -> Value {
    json!({
        "schema": "podway.output/v4",
        "request_id": UUID,
        "command": "session.observe",
        "generated_at": "2026-10-23T00:00:00.000Z",
        "workspace": {"uuid": UUID, "root": "/tmp/podway-example", "latest_workspace_sequence": 1},
        "result": result,
        "warnings": []
    })
}

fn citation_envelope(details: &Value) -> Value {
    json!({
        "schema": "podway.error/v2",
        "request_id": UUID,
        "command": "goal.assess_criterion",
        "generated_at": "2026-10-23T00:00:00.000Z",
        "code": "CRITERION_CITATION_INVALID",
        "message": "A criterion citation does not name resolved attempt state.",
        "retryable": false,
        "exit_code": 1,
        "details": details
    })
}

fn mismatch_envelope(details: &Value) -> Value {
    json!({
        "schema": "podway.error/v2",
        "request_id": UUID,
        "command": "session.observe",
        "generated_at": "2026-10-23T00:00:00.000Z",
        "code": "WORKSPACE_CONTRACT_MISMATCH",
        "message": "The Store writer contract is not supported by this binary.",
        "retryable": false,
        "exit_code": 5,
        "details": details
    })
}

/// One fenced lifecycle template of the reserved v6 windows.
fn lifecycle_template(command: &str, head: &[&str], revision: u64, authorized: bool) -> Value {
    let mut argv = vec![json!("podway")];
    argv.extend(head.iter().map(|token| json!(token)));
    argv.extend([
        json!("--if-workspace-uuid"),
        json!(UUID),
        json!("--if-session-id"),
        json!(UUID),
        json!("--if-session-revision"),
        json!(revision.to_string()),
        json!("--idempotency-key"),
        json!("<idempotency-key>"),
    ]);
    json!({
        "command": command,
        "argv": argv,
        "preconditions": {"workspace_uuid": UUID, "session_id": UUID, "session_revision": revision},
        "authority": "optimistic_concurrency_only",
        "idempotency_key_required": true,
        "requires_explicit_authorization": authorized,
    })
}

fn terminal_observation(base: &Value, lifecycle: &str, templates: &[Value]) -> Value {
    let mut document = base.clone();
    document["status"]["session"] = json!({"id": UUID, "lifecycle": lifecycle, "revision": 4});
    document["guidance"] = Value::Null;
    document["mutation_templates"] = json!(templates);
    document["mutation_templates_total"] = json!(templates.len());
    document
}

fn running_observation(documents: &Value) -> Value {
    let mut document = documents["fixtures"]["podway.observation-result/v6"].clone();
    document["status"]["session"] = json!({"id": UUID, "lifecycle": "running", "revision": 4});
    document["status"]["current"] = json!({
        "node": {"node_definition_id": "work", "graph_node_id": "work", "node_type": "action"},
        "attempt": {"attempt_id": UUID, "attempt_number": 1},
        "readiness": {"items_satisfied": true, "unblocked": true, "goal_ready": true, "can_advance": true},
        "missing_required_item_count": 0,
        "blockers_total": 0
    });
    document["status"]["delegation"] =
        documents["fixtures"]["podway.next-result/v5"]["delegation"].clone();
    document["status"]["trace_length"] = json!(1);
    document["guidance"] = documents["fixtures"]["podway.next-result/v5"].clone();
    document["mutation_templates"] = json!([lifecycle_template(
        "session.reset",
        &["reset", "--progress-summary", "<progress-summary>", "--yes"],
        4,
        true,
    )]);
    document["mutation_templates_total"] = json!(1);
    document
}

fn assert_v6(document: &Value, valid: bool) {
    assert_eq!(
        validate_daemon_free_document_v1("podway.observation-result/v6", document),
        valid
    );
    assert_eq!(
        OutputEnvelopeV4::from_value(output_envelope(document)).is_ok(),
        valid
    );
}

#[test]
fn guide007_observation_v6_pins_every_lifecycle_window() {
    let documents = fixtures();
    let prepared = documents["fixtures"]["podway.observation-result/v6"].clone();
    assert_v6(&prepared, true);

    // Pinned counts: the prepared window offers exactly four templates.
    let mut wrong_total = prepared.clone();
    wrong_total["mutation_templates_total"] = json!(3);
    assert_v6(&wrong_total, false);
    let mut dropped = prepared.clone();
    dropped["mutation_templates"].as_array_mut().unwrap().pop();
    assert_v6(&dropped, false);

    // Pinned per-head authorization flags (D3).
    for (index, flipped) in [(0, true), (1, false), (2, false), (3, false)] {
        let mut mutated = prepared.clone();
        mutated["mutation_templates"][index]["requires_explicit_authorization"] = json!(flipped);
        assert_v6(&mutated, false);
    }

    // Variant discrimination (D4): two preset variants.
    let mut two_preset = prepared.clone();
    two_preset["mutation_templates"][3]["argv"] =
        two_preset["mutation_templates"][2]["argv"].clone();
    assert_v6(&two_preset, false);

    // A procedure variant without its digest flag matches neither variant shape.
    let mut missing_digest_flag = prepared.clone();
    let argv = missing_digest_flag["mutation_templates"][3]["argv"]
        .as_array()
        .unwrap()
        .clone();
    let pruned: Vec<Value> = argv
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 6 && *index != 7)
        .map(|(_, token)| token.clone())
        .collect();
    missing_digest_flag["mutation_templates"][3]["argv"] = json!(pruned);
    assert_v6(&missing_digest_flag, false);

    // A1 hybrid plus bare: one template carrying both discriminators, one carrying neither.
    let mut hybrid_bare = prepared.clone();
    hybrid_bare["mutation_templates"][2] = lifecycle_template(
        "session.start_replace",
        &[
            "start",
            "--on-existing",
            "delete",
            "--preset",
            "<preset>",
            "--procedure",
            "<procedure-file>",
            "--expect-procedure-digest",
            "<procedure-digest>",
            "--task",
            "<task>",
        ],
        0,
        true,
    );
    hybrid_bare["mutation_templates"][3] = lifecycle_template(
        "session.start_replace",
        &["start", "--on-existing", "delete", "--task", "<task>"],
        0,
        true,
    );
    assert_v6(&hybrid_bare, false);

    // Closed placeholder vocabulary (D5).
    let mut surprise = prepared.clone();
    surprise["mutation_templates"][2]["argv"][5] = json!("<surprise>");
    assert_v6(&surprise, false);

    // Missing fence token or duplicated key token on a lifecycle template.
    let mut bare = prepared.clone();
    bare["mutation_templates"][0]["argv"] = json!(["podway", "begin"]);
    assert_v6(&bare, false);
    let mut duplicated_key = prepared.clone();
    let mut argv = prepared["mutation_templates"][0]["argv"]
        .as_array()
        .unwrap()
        .clone();
    argv.push(json!("<idempotency-key>"));
    duplicated_key["mutation_templates"][0]["argv"] = json!(argv);
    assert_v6(&duplicated_key, false);

    // Undisposed terminal window: two discriminated disposition variants, no authorization.
    let undisposed = terminal_observation(
        &prepared,
        "completed",
        &[
            lifecycle_template(
                "session.terminal_disposition",
                &[
                    "disposition",
                    "handed-off",
                    "--summary",
                    "<summary>",
                    "--reference",
                    "<reference>",
                ],
                4,
                false,
            ),
            lifecycle_template(
                "session.terminal_disposition",
                &["disposition", "not-required", "--reason", "<reason>"],
                4,
                false,
            ),
        ],
    );
    assert_v6(&undisposed, true);

    // A1 hybrid plus bare: both variant counts stay at one, so only the
    // per-item oneOf rejects the hybrid carrying both discriminator sets.
    let mut undisposed_hybrid_bare = undisposed.clone();
    undisposed_hybrid_bare["mutation_templates"][0] = lifecycle_template(
        "session.terminal_disposition",
        &[
            "disposition",
            "handed-off",
            "--summary",
            "<summary>",
            "--reference",
            "<reference>",
            "not-required",
            "--reason",
            "<reason>",
        ],
        4,
        false,
    );
    undisposed_hybrid_bare["mutation_templates"][1] =
        lifecycle_template("session.terminal_disposition", &["disposition"], 4, false);
    assert_v6(&undisposed_hybrid_bare, false);

    // Pinned per-head authorization flags (D3): each disposition flips alone.
    for index in 0..2 {
        let mut flipped = undisposed.clone();
        flipped["mutation_templates"][index]["requires_explicit_authorization"] = json!(true);
        assert_v6(&flipped, false);
    }

    let mut wrong_terminal_total = undisposed.clone();
    wrong_terminal_total["mutation_templates_total"] = json!(4);
    assert_v6(&wrong_terminal_total, false);

    // Disposed terminal window: archive, reset and two discriminated fenced starts.
    let disposed = terminal_observation(
        &prepared,
        "cancelled",
        &[
            lifecycle_template("session.archive", &["archive"], 4, true),
            lifecycle_template("session.reset", &["reset"], 4, true),
            lifecycle_template(
                "session.start",
                &["start", "--preset", "<preset>", "--task", "<task>"],
                4,
                true,
            ),
            lifecycle_template(
                "session.start",
                &[
                    "start",
                    "--procedure",
                    "<procedure-file>",
                    "--expect-procedure-digest",
                    "<procedure-digest>",
                    "--task",
                    "<task>",
                ],
                4,
                true,
            ),
        ],
    );
    assert_v6(&disposed, true);

    // session.start_replace is not a disposed-window command.
    let mut wrong_command = disposed.clone();
    wrong_command["mutation_templates"][3] = lifecycle_template(
        "session.start_replace",
        &[
            "start",
            "--on-existing",
            "delete",
            "--preset",
            "<preset>",
            "--task",
            "<task>",
        ],
        4,
        true,
    );
    assert_v6(&wrong_command, false);

    let mut disposed_hybrid_bare = disposed.clone();
    disposed_hybrid_bare["mutation_templates"][2] = lifecycle_template(
        "session.start",
        &[
            "start",
            "--preset",
            "<preset>",
            "--procedure",
            "<procedure-file>",
            "--expect-procedure-digest",
            "<procedure-digest>",
            "--task",
            "<task>",
        ],
        4,
        true,
    );
    disposed_hybrid_bare["mutation_templates"][3] =
        lifecycle_template("session.start", &["start", "--task", "<task>"], 4, true);
    assert_v6(&disposed_hybrid_bare, false);

    // Pinned per-head authorization flags (D3): each disposed head flips alone.
    for index in 0..4 {
        let mut flipped = disposed.clone();
        flipped["mutation_templates"][index]["requires_explicit_authorization"] = json!(false);
        assert_v6(&flipped, false);
    }

    // The running window keeps the v5 running shapes, including the engine's
    // progress-summary reset template and its non-lifecycle placeholder.
    let running = running_observation(&documents);
    assert_v6(&running, true);

    // The running window also keeps the v5 template domain: the lifecycle-only
    // commands session.archive and session.start are rejected even when fenced,
    // because only the command domain fails for these single-template windows.
    let mut running_archive = running.clone();
    running_archive["mutation_templates"] =
        json!([lifecycle_template("session.archive", &["archive"], 4, true)]);
    running_archive["mutation_templates_total"] = json!(1);
    assert_v6(&running_archive, false);

    let mut running_start = running.clone();
    running_start["mutation_templates"] = json!([lifecycle_template(
        "session.start",
        &["start", "--preset", "<preset>", "--task", "<task>"],
        4,
        true,
    )]);
    running_start["mutation_templates_total"] = json!(1);
    assert_v6(&running_start, false);
}

#[test]
fn guide007_template_correlations_reject_fence_and_key_drift() {
    let documents = fixtures();
    let prepared = documents["fixtures"]["podway.observation-result/v6"].clone();
    assert_v6(&prepared, true);

    // R1: a fence value drifts from the preconditions while staying schema-shaped.
    let mut fence_drift = prepared.clone();
    fence_drift["mutation_templates"][0]["argv"][3] = json!(OTHER_UUID);
    assert_v6(&fence_drift, false);

    // R1: a fence flag repeated outside the tail.
    let mut duplicated_flag = prepared.clone();
    let mut argv = prepared["mutation_templates"][0]["argv"]
        .as_array()
        .unwrap()
        .clone();
    argv.insert(2, json!("--if-session-id"));
    duplicated_flag["mutation_templates"][0]["argv"] = json!(argv);
    assert_v6(&duplicated_flag, false);

    // R4: lifecycle preconditions disagree with the status session while argv
    // still agrees with the preconditions.
    let mut session_drift = prepared.clone();
    session_drift["mutation_templates"][0]["argv"][5] = json!(OTHER_UUID);
    session_drift["mutation_templates"][0]["preconditions"]["session_id"] = json!(OTHER_UUID);
    assert_v6(&session_drift, false);

    // R2 and R3 through a running item.record_many template whose key and
    // identities live inside stdin_template.
    let record_stdin = |session_id: &str, repeat_key: bool| {
        let mut stdin = json!({
            "schema": "podway.item-record-many-input/v1",
            "workspace_uuid": UUID,
            "session_id": session_id,
            "session_revision": 4,
            "attempt_id": UUID,
            "idempotency_key": "<idempotency-key>",
            "operations": []
        });
        if repeat_key {
            stdin["key_repeat"] = json!("<idempotency-key>");
        }
        serde_json::to_string(&stdin).unwrap()
    };
    let record_template = |stdin: String| {
        json!({
            "command": "item.record_many",
            "argv": ["podway", "record", "--stdin"],
            "preconditions": {
                "workspace_uuid": UUID,
                "session_id": UUID,
                "session_revision": 4,
                "attempt_id": UUID,
                "item_revision": 1
            },
            "authority": "optimistic_concurrency_only",
            "idempotency_key_required": true,
            "requires_explicit_authorization": false,
            "stdin_template": stdin
        })
    };
    let mut record = running_observation(&documents);
    record["mutation_templates"] = json!([record_template(record_stdin(UUID, false))]);
    assert_v6(&record, true);

    let mut key_twice = record.clone();
    key_twice["mutation_templates"] = json!([record_template(record_stdin(UUID, true))]);
    assert_v6(&key_twice, false);

    let mut identity_drift = record.clone();
    identity_drift["mutation_templates"] =
        json!([record_template(record_stdin(OTHER_UUID, false))]);
    assert_v6(&identity_drift, false);
}

#[test]
fn guide007_error_details_successors_are_exclusive_and_closed() {
    let documents = fixtures();
    let successor = documents["fixtures"]["podway.criterion-citation-invalid-details/v1"].clone();
    let generation1 = json!({
        "schema": "podway.v2-runtime-error-details/v2",
        "kind": "CRITERION_CITATION_INVALID",
        "criterion_id": "done",
        "citation": {"local_item_id": "result"}
    });

    // Both generations' details validate under the same code.
    for details in [&generation1, &successor] {
        let envelope = citation_envelope(details);
        assert!(validate_daemon_free_document_v1(
            "podway.error/v2",
            &envelope
        ));
        assert!(ErrorEnvelopeV2::from_value(envelope).is_ok());
    }

    // The closed families are mutually exclusive.
    assert!(validate_daemon_free_document_v1(
        "podway.criterion-citation-invalid-details/v1",
        &successor
    ));
    assert!(!validate_daemon_free_document_v1(
        "podway.criterion-citation-invalid-details/v1",
        &generation1
    ));
    assert!(validate_daemon_free_document_v1(
        "podway.v2-runtime-error-details/v2",
        &generation1
    ));
    assert!(!validate_daemon_free_document_v1(
        "podway.v2-runtime-error-details/v2",
        &successor
    ));

    // Closed shape: unknown fields, duplicates and cap violations are rejected.
    let mut open = successor.clone();
    open["citable_source_nodes"] = json!([]);
    assert!(!validate_daemon_free_document_v1(
        "podway.criterion-citation-invalid-details/v1",
        &open
    ));
    let mut duplicate_items = successor.clone();
    duplicate_items["citable_item_ids"] = json!(["result", "result"]);
    assert!(!validate_daemon_free_document_v1(
        "podway.criterion-citation-invalid-details/v1",
        &duplicate_items
    ));
    let mut over_cap = successor.clone();
    over_cap["citable_evidence_source_nodes"] = json!([
        "node-1", "node-2", "node-3", "node-4", "node-5", "node-6", "node-7", "node-8", "node-9"
    ]);
    assert!(!validate_daemon_free_document_v1(
        "podway.criterion-citation-invalid-details/v1",
        &over_cap
    ));
    let mut missing = successor.clone();
    missing.as_object_mut().unwrap().remove("citable_item_ids");
    assert!(!validate_daemon_free_document_v1(
        "podway.criterion-citation-invalid-details/v1",
        &missing
    ));

    // Released-table agreement (A2): known digest with its exact release and
    // unknown digest with null pass; every other pairing fails.
    let mismatch_details = |digest: &str, version: Value| {
        json!({
            "schema": "podway.workspace-contract-mismatch-details/v2",
            "expected_contract_manifest_digest": DIGEST,
            "actual_contract_manifest_digest": digest,
            "actual_product_version": version,
            "recovery": documents["fixtures"]["podway.workspace-contract-mismatch-details/v1"]["recovery"]
        })
    };
    for (digest, version) in [
        (V0_2_13_DIGEST, json!("v0.2.13")),
        (V0_2_14_DIGEST, json!("v0.2.14")),
        (DIGEST, Value::Null),
    ] {
        let details = mismatch_details(digest, version);
        assert!(validate_daemon_free_document_v1(
            "podway.workspace-contract-mismatch-details/v2",
            &details
        ));
        let envelope = mismatch_envelope(&details);
        assert!(validate_daemon_free_document_v1(
            "podway.error/v2",
            &envelope
        ));
        assert!(ErrorEnvelopeV2::from_value(envelope).is_ok());
    }
    for (digest, version) in [
        (V0_2_14_DIGEST, Value::Null),
        (V0_2_14_DIGEST, json!("v0.2.13")),
        (DIGEST, json!("v0.2.14")),
    ] {
        let details = mismatch_details(digest, version);
        assert!(!validate_daemon_free_document_v1(
            "podway.workspace-contract-mismatch-details/v2",
            &details
        ));
        let envelope = mismatch_envelope(&details);
        assert!(!validate_daemon_free_document_v1(
            "podway.error/v2",
            &envelope
        ));
        assert!(ErrorEnvelopeV2::from_value(envelope).is_err());
    }

    // The generation-1 mismatch family stays unchanged and valid.
    assert!(validate_daemon_free_document_v1(
        "podway.workspace-contract-mismatch-details/v1",
        &documents["fixtures"]["podway.workspace-contract-mismatch-details/v1"]
    ));
}

#[test]
fn guide007_output_v4_observation_selects_either_family() {
    let documents = fixtures();
    for family in [
        "podway.observation-result/v5",
        "podway.observation-result/v6",
    ] {
        let envelope = output_envelope(&documents["fixtures"][family]);
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            &envelope
        ));
        assert!(OutputEnvelopeV4::from_value(envelope).is_ok());
    }
    let mut foreign = output_envelope(&documents["fixtures"]["podway.observation-result/v5"]);
    foreign["result"] = documents["fixtures"]["podway.next-result/v5"].clone();
    assert!(!validate_daemon_free_document_v1(
        "podway.output/v4",
        &foreign
    ));
    assert!(OutputEnvelopeV4::from_value(foreign).is_err());

    // The new registry arms equal their schemas' top-level field sets.
    for (family, path) in [
        (
            "podway.observation-result/v6",
            "schemas/observation-result-v6.schema.json",
        ),
        (
            "podway.criterion-citation-invalid-details/v1",
            "schemas/criterion-citation-invalid-details-v1.schema.json",
        ),
        (
            "podway.workspace-contract-mismatch-details/v2",
            "schemas/workspace-contract-mismatch-details-v2.schema.json",
        ),
    ] {
        let schema = read_schema(path);
        let required: BTreeSet<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field.as_str().unwrap())
            .collect();
        let allowed: BTreeSet<&str> = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let (arm_required, arm_allowed) = result_schema_top_level_fields_v2(family).unwrap();
        assert_eq!(
            required,
            arm_required.iter().copied().collect::<BTreeSet<_>>()
        );
        assert_eq!(
            allowed,
            arm_allowed.iter().copied().collect::<BTreeSet<_>>()
        );
    }
}

#[test]
fn guide007_generation_type_and_released_manifests_table() {
    assert_eq!(
        ContractGenerationV1::default(),
        ContractGenerationV1::Generation1
    );
    assert_eq!(
        ContractGenerationV1::parse_generation_value_v1("1"),
        Some(ContractGenerationV1::Generation1)
    );
    assert_eq!(
        ContractGenerationV1::parse_generation_value_v1("2"),
        Some(ContractGenerationV1::Generation2)
    );
    for rejected in ["", "0", "3", "v2"] {
        assert_eq!(
            ContractGenerationV1::parse_generation_value_v1(rejected),
            None
        );
    }
    assert_eq!(ContractGenerationV1::Generation1.as_u64(), 1);
    assert_eq!(ContractGenerationV1::Generation2.as_u64(), 2);

    assert_eq!(RELEASED_CONTRACT_MANIFESTS_V1.len(), 2);
    assert_eq!(released_writer_release_v1(V0_2_13_DIGEST), Some("v0.2.13"));
    assert_eq!(released_writer_release_v1(V0_2_14_DIGEST), Some("v0.2.14"));
    assert_eq!(released_writer_release_v1(DIGEST), None);

    for (schema, path, commands) in [
        (
            "podway.observation-result/v6",
            "schemas/observation-result-v6.schema.json",
            &["session.observe"][..],
        ),
        (
            "podway.criterion-citation-invalid-details/v1",
            "schemas/criterion-citation-invalid-details-v1.schema.json",
            &[][..],
        ),
        (
            "podway.workspace-contract-mismatch-details/v2",
            "schemas/workspace-contract-mismatch-details-v2.schema.json",
            &[][..],
        ),
    ] {
        let contract = DAEMON_FREE_DOCUMENT_SCHEMAS_V1
            .iter()
            .find(|contract| contract.schema == schema)
            .unwrap_or_else(|| panic!("{schema} is registered"));
        assert_eq!(contract.schema_path, path);
        assert_eq!(contract.commands, commands);
    }
}
