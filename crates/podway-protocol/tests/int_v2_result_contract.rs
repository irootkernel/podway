use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use jsonschema::{Retrieve, Uri};
use podway_core::{AuthoringDiagnostic, AuthoringDiagnosticCode, SourceLocation};
use podway_protocol::{
    CommandNameV1, DAEMON_FREE_DOCUMENT_SCHEMAS_V1, EXISTING_ROUTE_RESULT_SCHEMAS_V2,
    ErrorEnvelopeV2, MAX_DIRECT_OUTPUT_BYTES_V1, MAX_V2_OUTPUT_WARNINGS,
    NEW_ROUTE_RESULT_SCHEMAS_V1, OUTPUT_SCHEMA_V4, OutputEnvelopeInputV4, OutputEnvelopeV4,
    PROCEDURE_DIAGNOSTICS_RESULT_SCHEMA_V1, ProtocolError, RequestIdV1, Rfc3339MillisV1,
    decode_result_schema_contract_v2, result_schema_top_level_fields_v2,
    validate_command_result_v2, validate_delegation_document_v1,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const SCHEMA_BASE: &str = "https://podway.invalid/schemas/";
const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const UUID: &str = "00000000-0000-4000-8000-000000000001";

#[derive(Clone)]
struct LocalSchemas(Arc<HashMap<String, Value>>);

impl Retrieve for LocalSchemas {
    fn retrieve(&self, uri: &Uri<String>) -> Result<Value, Box<dyn Error + Send + Sync>> {
        self.0.get(uri.as_str()).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("unregistered schema URI: {uri}"),
            )
            .into()
        })
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_schema(relative: &str) -> Value {
    serde_json::from_slice(&fs::read(root().join("assets").join(relative)).unwrap()).unwrap()
}

fn graph_counters(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "graph_node_id": format!("node-{index}"),
                    "attempt_count": 0,
                    "rework_traversal_count": 0,
                })
            })
            .collect(),
    )
}
fn local_schemas() -> LocalSchemas {
    let mut resources = HashMap::new();
    for entry in fs::read_dir(root().join("assets/schemas")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let schema: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let id = schema["$id"].as_str().unwrap().to_owned();
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(resources.insert(id, schema.clone()).is_none());
        assert!(
            resources
                .insert(format!("{SCHEMA_BASE}{filename}"), schema)
                .is_none()
        );
    }
    LocalSchemas(Arc::new(resources))
}
fn assert_valid(schema_path: &str, value: &Value) {
    let mut schema = read_schema(schema_path);
    schema.as_object_mut().unwrap().remove("$id");
    let filename = Path::new(schema_path)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let validator = jsonschema::draft202012::options()
        .with_base_uri(format!("{SCHEMA_BASE}{filename}"))
        .with_retriever(local_schemas())
        .should_validate_formats(true)
        .build(&schema)
        .unwrap_or_else(|error| panic!("{schema_path} does not compile: {error}"));
    let errors: Vec<_> = validator
        .iter_errors(value)
        .map(|error| error.to_string())
        .collect();
    assert!(
        errors.is_empty(),
        "{schema_path} rejected fixture: {errors:#?}"
    );
}
fn assert_invalid(schema_path: &str, value: &Value) {
    let mut schema = read_schema(schema_path);
    schema.as_object_mut().unwrap().remove("$id");
    let filename = Path::new(schema_path)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let validator = jsonschema::draft202012::options()
        .with_base_uri(format!("{SCHEMA_BASE}{filename}"))
        .with_retriever(local_schemas())
        .should_validate_formats(true)
        .build(&schema)
        .unwrap();
    assert!(
        !validator.is_valid(value),
        "{schema_path} accepted an invalid fixture"
    );
}
fn assert_ref_valid(reference: &str, value: &Value) {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$ref": reference
    });
    let validator = jsonschema::draft202012::options()
        .with_retriever(local_schemas())
        .should_validate_formats(true)
        .build(&schema)
        .unwrap();
    let errors = validator
        .iter_errors(value)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "{reference} rejected fixture: {errors:#?}"
    );
}
fn assert_ref_invalid(reference: &str, value: &Value) {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$ref": reference
    });
    let validator = jsonschema::draft202012::options()
        .with_retriever(local_schemas())
        .should_validate_formats(true)
        .build(&schema)
        .unwrap();
    assert!(
        !validator.is_valid(value),
        "{reference} accepted invalid fixture"
    );
}
fn maximum_identifier(index: usize) -> String {
    let suffix = format!("-{index}");
    format!("a{}{}", "a".repeat(63 - suffix.len()), suffix)
}
fn escape_heavy(characters: usize) -> String {
    "\0".repeat(characters)
}
fn result_map(value: &Value) -> Map<String, Value> {
    value.as_object().expect("result object").clone()
}
fn warning(code: &str) -> Map<String, Value> {
    result_map(&json!({
        "code": code, "path": "workflow.yaml", "message": "Review this field."
    }))
}
fn authoring_input(command: &str, result: &Value) -> OutputEnvelopeInputV4 {
    OutputEnvelopeInputV4 {
        request_id: RequestIdV1::new(UUID).unwrap(),
        command: CommandNameV1::new(command).unwrap(),
        generated_at: Rfc3339MillisV1::new("2026-08-04T00:00:00.000Z").unwrap(),
        workspace: None,
        session: None,
        result: result_map(result),
        warnings: Vec::new(),
    }
}
fn diagnostics_result(operation: &str) -> Value {
    let mut result = examples()["podway.procedure-diagnostics-result/v1"].clone();
    result["operation"] = json!(operation);
    result
}
fn representative_authoring_diagnostic() -> AuthoringDiagnostic {
    AuthoringDiagnostic::new(
        AuthoringDiagnosticCode::EvidenceSourceDoesNotDominateConsumer,
        "workflow.yaml",
        SourceLocation::new(1, 1, 1, 8),
        "graph.nodes[review].evidence_from[build]",
        "Evidence does not dominate.",
        "Use a dominating source.",
    )
    .with_node_definition_id("review")
    .with_graph_node_id("review")
    .with_related_graph_node_ids(["build".to_owned()])
}
fn check_result_value() -> Value {
    json!({
        "operation_id": "make-test",
        "operation_digest": DIGEST,
        "input_basis": {"descriptor": "HEAD and dirty-tree snapshot", "digest": DIGEST},
        "executor": {"name": "gaori", "version": "1.0.0"},
        "outcome": "pass",
        "summary": "The complete development gate passed.",
        "output_digest": DIGEST
    })
}
fn check_result_preview() -> Value {
    json!({
        "operation_id": "make-test",
        "outcome": "pass",
        "operation_digest": DIGEST,
        "input_basis": {"digest": DIGEST},
        "output_digest": DIGEST
    })
}
fn typed_guard_predicate_status() -> Value {
    json!({
        "source": {"kind": "evidence", "graph_node_id": "review", "item_id": "verdict"},
        "operator": "equals",
        "expected": "approved",
        "actual": "approved",
        "state": "met"
    })
}
fn examples() -> BTreeMap<&'static str, Value> {
    let mut examples = BTreeMap::from([
        (
            "podway.procedure-validation-result/v2",
            json!({"schema":"podway.procedure-validation-result/v2","file":"workflow.yaml","procedure_schema":"podway.procedure/v2","digest":DIGEST,"valid":true}),
        ),
        (
            "podway.procedure-source-result/v1",
            json!({"schema":"podway.procedure-source-result/v1","operation":"format","target_schema":"podway.procedure/v2","document":"schema: podway.procedure/v2\n","target_digest":DIGEST,"file":"workflow.yaml","mode":"stdout","changed":false}),
        ),
        (
            "podway.procedure-diagnostics-result/v1",
            json!({"schema":"podway.procedure-diagnostics-result/v1","operation":"vet","procedure_schema":"podway.procedure/v2","file":"workflow.yaml","valid":true,"diagnostics":[],"diagnostics_truncated":false,"diagnostics_total":0}),
        ),
        (
            "podway.procedure-graph-result/v1",
            json!({"schema":"podway.procedure-graph-result/v1","procedure_schema":"podway.procedure/v2","procedure_digest":DIGEST,"format":"mermaid","projection_digest":DIGEST,"projection":"flowchart TD"}),
        ),
        (
            "podway.procedure-preview-result/v1",
            json!({"schema":"podway.procedure-preview-result/v1","file":"workflow.yaml","admissible":true,"procedure_schema":"podway.procedure/v2","procedure_id":"workflow","procedure_version":"1","purpose":"test","procedure_digest":DIGEST,"goal_tracking":false,"goal_assessment_graph_node_ids":[],"summary":{"definition_count":1,"graph_node_count":1,"action_node_count":1,"decision_node_count":0,"route_count":0,"cycle_count":0,"evidence_reference_count":0,"skippable_node_count":0,"manual_rework_target_count":0},"checks":{"validate":true,"vet":true,"lint":true},"graph":{"entry_graph_node_id":"work","terminal_graph_node_ids":["work"],"nodes":[{"graph_node_id":"work","node_definition_id":"work","node_type":"action","terminal":true,"skippable":false}],"edges":[]},"mermaid":"flowchart TD\n  work","start_suggestion":{"command":"session.start","argv":["podway","start","--procedure","workflow.yaml","--expect-procedure-digest",DIGEST,"--task","<title>"]},"diagnostics":[],"diagnostics_truncated":false,"diagnostics_total":0}),
        ),
    ]);
    let current: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/protocol/current-contracts.json"
    ))
    .unwrap();
    for contract in DAEMON_FREE_DOCUMENT_SCHEMAS_V1
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
    {
        if let Some(value) = current["fixtures"].get(contract.schema) {
            examples.insert(contract.schema, value.clone());
        }
    }
    examples
}
#[test]
fn v2grf005_graph_result_uses_plantuml_as_the_machine_format_name() {
    let mut result = examples()["podway.procedure-graph-result/v1"].clone();
    result["format"] = json!("plantuml");
    assert_valid("schemas/procedure-graph-result-v1.schema.json", &result);

    result["format"] = json!("puml");
    assert_invalid("schemas/procedure-graph-result-v1.schema.json", &result);
}
#[test]
fn v2grf006_graph_result_admits_the_dot_machine_format() {
    let mut result = examples()["podway.procedure-graph-result/v1"].clone();
    result["format"] = json!("dot");
    result["projection"] = json!("digraph podway {}");
    assert_valid("schemas/procedure-graph-result-v1.schema.json", &result);
}
#[test]
fn v2grf_preview_uses_one_closed_result_family_for_every_document_outcome() {
    let diagnostics_contract = NEW_ROUTE_RESULT_SCHEMAS_V1
        .iter()
        .find(|contract| contract.schema == PROCEDURE_DIAGNOSTICS_RESULT_SCHEMA_V1)
        .expect("the diagnostics result family is registered");
    assert!(!diagnostics_contract.commands.contains(&"procedure.preview"));

    let diagnostics = diagnostics_result("preview");
    assert_invalid(
        "schemas/procedure-diagnostics-result-v1.schema.json",
        &diagnostics,
    );
    let output = json!({
        "schema":OUTPUT_SCHEMA_V4,"request_id":UUID,"command":"procedure.preview",
        "generated_at":"2026-08-04T00:00:00.000Z","result":diagnostics,"warnings":[]
    });
    assert_invalid("schemas/output-v4.schema.json", &output);
}
#[test]
fn v2ctr003_registry_top_level_fields_match_every_canonical_schema() {
    for contract in EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
    {
        let schema = read_schema(contract.schema_path);
        let schema_required: BTreeSet<_> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field.as_str().unwrap())
            .collect();
        let schema_allowed: BTreeSet<_> = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let (required, allowed) = result_schema_top_level_fields_v2(contract.schema).unwrap();
        assert_eq!(schema_required, required.iter().copied().collect());
        assert_eq!(schema_allowed, allowed.iter().copied().collect());
    }
}
#[test]
fn v2sdg002_delegation_documents_reject_unbounded_and_mismatched_input() {
    let fixtures = examples();
    let documents: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/protocol/current-contracts.json"
    ))
    .unwrap();
    let package = fixtures["podway.delegation-package-result/v1"]["package"].clone();
    let acceptance = documents["fixtures"]["podway.delegation-accept-input/v1"].clone();
    let receipt = fixtures["podway.delegation-receipt-result/v1"]["receipt"].clone();
    for (schema, value) in [
        ("podway.delegation-work-package/v1", &package),
        ("podway.delegation-accept-input/v1", &acceptance),
        ("podway.delegation-receipt/v1", &receipt),
    ] {
        assert!(validate_delegation_document_v1(schema, value), "{schema}");
    }

    let mut unknown = acceptance.clone();
    unknown["credential"] = json!("secret");
    assert!(!validate_delegation_document_v1(
        "podway.delegation-accept-input/v1",
        &unknown
    ));
    let mut wrong_type = acceptance.clone();
    wrong_type["rounds_used"] = json!("1");
    assert!(!validate_delegation_document_v1(
        "podway.delegation-accept-input/v1",
        &wrong_type
    ));
    for field in ["session_revision", "rounds_used"] {
        let mut float_form = acceptance.clone();
        float_form[field] = json!(1.0);
        assert!(
            !validate_delegation_document_v1("podway.delegation-accept-input/v1", &float_form),
            "{field} must use an unsigned integer JSON representation"
        );
    }
    let mut over_bound = acceptance.clone();
    over_bound["summary"] = json!("가".repeat(1500));
    assert!(!validate_delegation_document_v1(
        "podway.delegation-accept-input/v1",
        &over_bound
    ));
    let mut secret_shaped = package.clone();
    secret_shaped["prompt_transcript"] = json!("private");
    assert!(!validate_delegation_document_v1(
        "podway.delegation-work-package/v1",
        &secret_shaped
    ));
    let mut duplicate_selectors = package.clone();
    let evidence = json!({"selector":{"node":"source"},"resolution":"resolved","source_attempt_id":UUID,"source_attempt_number":1,"source_items_digest":DIGEST,"items":[]});
    duplicate_selectors["evidence"] = json!([evidence.clone(), evidence]);
    assert_valid(
        "schemas/delegation-work-package-v1.schema.json",
        &duplicate_selectors,
    );
    assert!(!validate_delegation_document_v1(
        "podway.delegation-work-package/v1",
        &duplicate_selectors
    ));
    let mut optional_unresolved = package.clone();
    optional_unresolved["evidence"] = json!([{
        "selector": {"node": "source", "required": false},
        "resolution": "unresolved",
        "items": [{"item_id": "result", "state": "missing"}]
    }]);
    assert!(validate_delegation_document_v1(
        "podway.delegation-work-package/v1",
        &optional_unresolved
    ));
    optional_unresolved["evidence"][0]["source_attempt_id"] = json!(UUID);
    assert!(!validate_delegation_document_v1(
        "podway.delegation-work-package/v1",
        &optional_unresolved
    ));
    let mut all_items = package.clone();
    all_items["evidence"] = json!([{
        "selector": {"node": "source"},
        "resolution": "resolved",
        "source_attempt_id": UUID,
        "source_attempt_number": 1,
        "source_items_digest": DIGEST,
        "items": (0..128).map(|index| json!({
            "item_id": format!("item-{index}"),
            "state": "missing"
        })).collect::<Vec<_>>()
    }]);
    assert!(validate_delegation_document_v1(
        "podway.delegation-work-package/v1",
        &all_items
    ));
    for field in [
        "source_attempt_id",
        "source_attempt_number",
        "source_items_digest",
    ] {
        let mut missing_source_field = all_items.clone();
        missing_source_field["evidence"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(!validate_delegation_document_v1(
            "podway.delegation-work-package/v1",
            &missing_source_field
        ));
    }
    let duplicate_item = json!({"item_id": "same", "state": "missing"});
    all_items["evidence"][0]["items"] = json!([duplicate_item.clone(), duplicate_item]);
    assert_valid("schemas/delegation-work-package-v1.schema.json", &all_items);
    assert!(!validate_delegation_document_v1(
        "podway.delegation-work-package/v1",
        &all_items
    ));
    let mut skipped_present = package.clone();
    skipped_present["evidence"] = json!([{
        "selector": {"node": "source"},
        "resolution": "skipped",
        "source_attempt_id": UUID,
        "source_attempt_number": 1,
        "source_items_digest": DIGEST,
        "items": [{
            "item_id": "result", "state": "present", "item_revision": 1,
            "value_digest": DIGEST, "value_type": "text", "size_bytes": 5
        }]
    }]);
    assert!(!validate_delegation_document_v1(
        "podway.delegation-work-package/v1",
        &skipped_present
    ));
    let mut mismatched = receipt.clone();
    mismatched["execution"]["role_ref"] = json!("other-role");
    assert_valid("schemas/delegation-receipt-v1.schema.json", &mismatched);
    assert!(!validate_delegation_document_v1(
        "podway.delegation-receipt/v1",
        &mismatched
    ));
    for field in ["binding_ref", "role_snapshot_digest"] {
        let mut mismatched = receipt.clone();
        mismatched["execution"][field] = if field == "binding_ref" {
            json!("other-binding")
        } else {
            json!("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        };
        assert!(
            !validate_delegation_document_v1("podway.delegation-receipt/v1", &mismatched),
            "{field}"
        );
    }
    let mut excessive_rounds = receipt.clone();
    excessive_rounds["rounds_used"] =
        json!(receipt["delegation"]["max_rounds"].as_u64().unwrap() + 1);
    assert!(!validate_delegation_document_v1(
        "podway.delegation-receipt/v1",
        &excessive_rounds
    ));
    let mut released = receipt.clone();
    released["delegation"]["after_accept"] = json!("release");
    released["worker_disposition"] = json!("released");
    assert!(validate_delegation_document_v1(
        "podway.delegation-receipt/v1",
        &released
    ));
    released["worker_disposition"] = json!("idle");
    assert!(!validate_delegation_document_v1(
        "podway.delegation-receipt/v1",
        &released
    ));

    let check_utf8_bound = |schema: &str, baseline: &Value, paths: &[&str], limit: usize| {
        let exactly = format!("{}{}", "가".repeat(limit / 3), "a".repeat(limit % 3));
        assert_eq!(exactly.len(), limit);
        let mut at_limit = baseline.clone();
        for path in paths {
            *at_limit.pointer_mut(path).unwrap() = json!(exactly);
        }
        assert!(
            validate_delegation_document_v1(schema, &at_limit),
            "{schema} {paths:?} at limit"
        );
        let mut over_limit = at_limit;
        for path in paths {
            *over_limit.pointer_mut(path).unwrap() = json!(format!("{exactly}가"));
        }
        assert!(
            !validate_delegation_document_v1(schema, &over_limit),
            "{schema} {paths:?} over limit"
        );
    };
    check_utf8_bound(
        "podway.delegation-work-package/v1",
        &package,
        &["/delegation/role_ref"],
        256,
    );
    check_utf8_bound(
        "podway.delegation-accept-input/v1",
        &acceptance,
        &["/idempotency_key"],
        256,
    );
    check_utf8_bound(
        "podway.delegation-accept-input/v1",
        &acceptance,
        &["/summary"],
        4096,
    );
    check_utf8_bound(
        "podway.delegation-receipt/v1",
        &receipt,
        &["/summary"],
        4096,
    );
    check_utf8_bound(
        "podway.delegation-receipt/v1",
        &receipt,
        &["/delegation/role_ref", "/execution/role_ref"],
        256,
    );
    for field in [
        "scope_ref",
        "supervisor_ref",
        "worker_ref",
        "task_ref",
        "role_ref",
    ] {
        check_utf8_bound(
            "podway.delegation-accept-input/v1",
            &acceptance,
            &[&format!("/execution/{field}")],
            256,
        );
        if field != "role_ref" {
            check_utf8_bound(
                "podway.delegation-receipt/v1",
                &receipt,
                &[&format!("/execution/{field}")],
                256,
            );
        }
    }
}
#[test]
fn v2sdg002_work_package_json_limit_rejects_schema_valid_oversize() {
    let fixtures = examples();
    let mut package = fixtures["podway.delegation-package-result/v1"]["package"].clone();
    let item = |index: usize| {
        json!({
            "id": format!("item-{index}"),
            "type": "confirm",
            "prompt": "\u{0001}".repeat(300),
            "help": "\u{0001}".repeat(1000),
            "required": true
        })
    };
    package["action"]["items"] = json!([]);
    let mut below_limit = package.clone();
    for index in 0..128 {
        package["action"]["items"]
            .as_array_mut()
            .unwrap()
            .push(item(index));
        if serde_json::to_vec(&package).unwrap().len() > 950_000 {
            break;
        }
        below_limit = package.clone();
    }
    assert!(serde_json::to_vec(&below_limit).unwrap().len() <= 950_000);
    assert!(serde_json::to_vec(&package).unwrap().len() > 950_000);
    assert_valid(
        "schemas/delegation-work-package-v1.schema.json",
        &below_limit,
    );
    assert_valid("schemas/delegation-work-package-v1.schema.json", &package);
    assert!(validate_delegation_document_v1(
        "podway.delegation-work-package/v1",
        &below_limit
    ));
    assert!(!validate_delegation_document_v1(
        "podway.delegation-work-package/v1",
        &package
    ));
}
#[test]
fn v2sdg002_delegation_result_correlations_reject_mismatched_identities() {
    let fixtures = examples();
    for (schema, fields) in [
        (
            "podway.delegation-package-result/v1",
            &["workspace_uuid", "session_id", "attempt_id"][..],
        ),
        (
            "podway.delegation-receipt-result/v1",
            &["session_id", "attempt_id"][..],
        ),
    ] {
        let result = &fixtures[schema];
        assert!(decode_result_schema_contract_v2(result).is_some());
        for field in fields {
            let mut mismatched = result.clone();
            mismatched[field] = json!("00000000-0000-4000-8000-000000000002");
            assert_valid(
                match schema {
                    "podway.delegation-package-result/v1" => {
                        "schemas/delegation-package-result-v1.schema.json"
                    }
                    _ => "schemas/delegation-receipt-result-v1.schema.json",
                },
                &mismatched,
            );
            assert!(
                decode_result_schema_contract_v2(&mismatched).is_none(),
                "{schema} {field}"
            );
        }
    }
    let mut absent_receipt = fixtures["podway.delegation-receipt-result/v1"].clone();
    absent_receipt["receipt"] = Value::Null;
    assert_invalid(
        "schemas/delegation-receipt-result-v1.schema.json",
        &absent_receipt,
    );
    assert!(decode_result_schema_contract_v2(&absent_receipt).is_none());
}
#[test]
fn v2sdg002_delegation_views_bind_current_receipt_to_current_attempt() {
    let fixtures = examples();
    let documents: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/protocol/current-contracts.json"
    ))
    .unwrap();
    let mut next = fixtures["podway.next-result/v5"].clone();
    next["delegation"]["receipt"]["attempt_id"] = json!("00000000-0000-4000-8000-000000000002");
    assert_valid("schemas/next-result-v5.schema.json", &next);
    assert!(decode_result_schema_contract_v2(&next).is_none());

    for (schema, fixture) in [
        (
            "podway.status-result/v5",
            &documents["fixtures"]["podway.status-result/v5"],
        ),
        (
            "podway.compact-status-result/v5",
            &documents["fixtures"]["podway.compact-status-result/v5"],
        ),
    ] {
        assert!(fixture["current"].is_null());
        assert_eq!(fixture["delegation"]["state"], "inactive", "{schema}");
        let mut inconsistent = fixture.clone();
        inconsistent["delegation"] =
            documents["fixtures"]["podway.delegation-projection/v1"].clone();
        assert!(
            decode_result_schema_contract_v2(&inconsistent).is_none(),
            "{schema}"
        );

        let active = &documents["fixtures"]["podway.next-result/v5"];
        let mut running = fixture.clone();
        running["session"]["lifecycle"] = json!("running");
        running["session"]["revision"] = json!(1);
        running["trace_length"] = json!(1);
        running["counters"] = active["counters"].clone();
        running["current"] = json!({
            "node": active["node"], "attempt": active["attempt"],
            "readiness": active["readiness"],
            "missing_required_item_count": 0, "blockers_total": 0
        });
        running["delegation"] = documents["fixtures"]["podway.delegation-projection/v1"].clone();
        if schema == "podway.status-result/v5" {
            running["terminal"] = json!(true);
            running["receipt_summaries"] = json!([running["delegation"]["receipt"]]);
            running["receipt_summaries_total"] = json!(1);
        }
        let schema_path = if schema == "podway.status-result/v5" {
            "schemas/status-result-v5.schema.json"
        } else {
            "schemas/compact-status-result-v5.schema.json"
        };
        assert_valid(schema_path, &running);
        assert!(decode_result_schema_contract_v2(&running).is_some());
        let mut inactive = running.clone();
        inactive["delegation"] = fixture["delegation"].clone();
        assert_valid(schema_path, &inactive);
        assert!(decode_result_schema_contract_v2(&inactive).is_none());
        running["delegation"]["receipt"]["attempt_id"] =
            json!("00000000-0000-4000-8000-000000000002");
        assert_valid(schema_path, &running);
        assert!(decode_result_schema_contract_v2(&running).is_none());
    }
    let mut observation = fixtures["podway.observation-result/v5"].clone();
    observation["status"]["delegation"] =
        documents["fixtures"]["podway.delegation-projection/v1"].clone();
    assert!(decode_result_schema_contract_v2(&observation).is_none());
    let mut archive = fixtures["podway.session-archive-show-result/v3"].clone();
    archive["state"]["delegation"] =
        documents["fixtures"]["podway.delegation-projection/v1"].clone();
    assert!(decode_result_schema_contract_v2(&archive).is_none());
}
#[test]
fn v2sdg002_delegation_projection_and_receipt_states_are_closed() {
    let fixtures = examples();
    let documents: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/protocol/current-contracts.json"
    ))
    .unwrap();
    let projection = documents["fixtures"]["podway.delegation-projection/v1"].clone();
    let mut inconsistent = projection.clone();
    inconsistent["safe_next_action"] = json!("continue_direct");
    assert_invalid(
        "schemas/delegation-projection-v1.schema.json",
        &inconsistent,
    );
    let mut receipt_result = fixtures["podway.delegation-receipt-result/v1"].clone();
    receipt_result["reason"] = json!("historical_attempt");
    assert_invalid(
        "schemas/delegation-receipt-result-v1.schema.json",
        &receipt_result,
    );
    let receipt_summary = projection["receipt"].clone();
    for (state, reason, action, has_receipt) in [
        (
            "not_applicable",
            "direct_placement",
            "continue_direct",
            false,
        ),
        (
            "awaiting_acceptance",
            "items_incomplete",
            "record_items",
            false,
        ),
        (
            "awaiting_acceptance",
            "blockers_open",
            "clear_blockers",
            false,
        ),
        ("awaiting_acceptance", "receipt_required", "accept", false),
        ("accepted", "current_receipt", "complete", true),
        (
            "stale_acceptance",
            "package_changed",
            "retry_or_rework",
            true,
        ),
        ("stale_acceptance", "items_changed", "retry_or_rework", true),
        ("inactive", "historical_attempt", "inspect_history", false),
        ("inactive", "no_active_attempt", "begin_or_inspect", false),
    ] {
        let mut value = json!({
            "state": state, "reason": reason, "safe_next_action": action,
        });
        if has_receipt {
            value["receipt"] = receipt_summary.clone();
        }
        assert_valid("schemas/delegation-projection-v1.schema.json", &value);
        let mut wrong_action = value.clone();
        wrong_action["safe_next_action"] = json!("continue_direct");
        if action != "continue_direct" {
            assert_invalid(
                "schemas/delegation-projection-v1.schema.json",
                &wrong_action,
            );
        }
        let mut wrong_reason = value.clone();
        wrong_reason["reason"] = json!("current_receipt");
        if reason != "current_receipt" {
            assert_invalid(
                "schemas/delegation-projection-v1.schema.json",
                &wrong_reason,
            );
        }
        let mut wrong_receipt = value.clone();
        wrong_receipt["receipt"] = if has_receipt {
            Value::Null
        } else {
            receipt_summary.clone()
        };
        assert_invalid(
            "schemas/delegation-projection-v1.schema.json",
            &wrong_receipt,
        );
    }
    for (applicability, reason) in [
        ("accepted", "current_receipt"),
        ("stale_acceptance", "package_changed"),
        ("stale_acceptance", "items_changed"),
        ("historical", "historical_attempt"),
    ] {
        let mut value = fixtures["podway.delegation-receipt-result/v1"].clone();
        value["applicability"] = json!(applicability);
        value["reason"] = json!(reason);
        assert_valid("schemas/delegation-receipt-result-v1.schema.json", &value);
        value["reason"] = json!("receipt_required");
        assert_invalid("schemas/delegation-receipt-result-v1.schema.json", &value);
    }
}
#[test]
fn v2sdg002_delegated_procedure_cannot_declare_skip() {
    let mut procedure: Value = serde_norway::from_str(include_str!(
        "../../../tests/fixtures/v2/compatibility/supervised-delegation-direct-control.yaml"
    ))
    .unwrap();
    assert_valid("schemas/procedure-v2.schema.json", &procedure);
    let documents: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/protocol/current-contracts.json"
    ))
    .unwrap();
    procedure["graph"]["nodes"][0]["delegation"] =
        documents["fixtures"]["podway.delegation-work-package/v1"]["delegation"].clone();
    assert_valid("schemas/procedure-v2.schema.json", &procedure);
    procedure["graph"]["nodes"][0]["skip"] = json!({"allowed":true,"reason_required":true});
    assert_invalid("schemas/procedure-v2.schema.json", &procedure);
}
#[test]
fn v2sdg002_mapping_known_answers_keep_distinct_immutable_domains() {
    let document: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/protocol/current-contracts.json"
    ))
    .unwrap();
    let answers = &document["mapping_known_answers"];
    let digest = |value: Value| {
        let encoded = serde_json::to_vec(&value).unwrap();
        format!("sha256:{:x}", Sha256::digest(encoded))
    };
    for (name, first, second) in [
        (
            "role_same_label_changed_content",
            json!({"role_ref":"reviewer","permissions":["read"]}),
            json!({"role_ref":"reviewer","permissions":["read","write"]}),
        ),
        (
            "configuration_changed",
            json!({"model":"one","tools":["read"]}),
            json!({"model":"two","tools":["read"]}),
        ),
        (
            "task_versus_envelope",
            json!({"task_id":"task-1","objective":"Review"}),
            json!({"task_id":"task-1","objective":"Review","delivery_state":"collected"}),
        ),
        (
            "complete_result_versus_summary",
            json!({"review":"accepted","patch":"diff-bytes"}),
            json!({"review":"accepted"}),
        ),
    ] {
        assert_eq!(answers[name][0], digest(first), "{name} first domain");
        assert_eq!(answers[name][1], digest(second), "{name} second domain");
        assert_ne!(answers[name][0], answers[name][1], "{name} distinction");
    }
    assert_eq!(
        answers["lost_acknowledgement"]["prepared_task_digest"],
        digest(json!({"objective":"Review"}))
    );
    assert!(answers["lost_acknowledgement"]["task_ref"].is_null());
}
#[test]
fn v2ctr003_authoring_diagnostic_is_standalone_closed_and_bounded() {
    let mut diagnostic = json!({
        "code":"EVIDENCE_SOURCE_DOES_NOT_DOMINATE_CONSUMER", "severity":"error",
        "schema":"podway.procedure/v2", "source_path":"workflow.yaml",
        "location":{"line":1,"column":1,"end_line":1,"end_column":8},
        "field":"graph.nodes[review].evidence_from[build]", "graph_node_id":"review",
        "related_graph_node_ids":["build"], "message":"Evidence does not dominate.",
        "hint":"Use a dominating source."
    });
    assert_valid("schemas/authoring-diagnostic-v1.schema.json", &diagnostic);
    let mut oversized = diagnostic.clone();
    oversized["message"] = json!("x".repeat(513));
    let schema = read_schema("schemas/authoring-diagnostic-v1.schema.json");
    let mut schema_without_id = schema.clone();
    schema_without_id.as_object_mut().unwrap().remove("$id");
    let validator = jsonschema::draft202012::options()
        .with_base_uri(format!("{SCHEMA_BASE}authoring-diagnostic-v1.schema.json"))
        .with_retriever(local_schemas())
        .build(&schema_without_id)
        .unwrap();
    assert!(!validator.is_valid(&oversized));

    let catalog: Value = serde_json::from_slice(
        &fs::read(root().join("assets/specifications/authoring-diagnostics.json")).unwrap(),
    )
    .unwrap();
    let catalog_pairs = catalog["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["code"].as_str().unwrap().to_owned(),
                entry["severity"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    let schema_pairs = schema["allOf"][0]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|branch| {
            let severity = branch["properties"]["severity"]["const"]
                .as_str()
                .unwrap()
                .to_owned();
            branch["properties"]["code"]["enum"]
                .as_array()
                .unwrap()
                .iter()
                .map(move |code| (code.as_str().unwrap().to_owned(), severity.clone()))
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(schema_pairs, catalog_pairs);
    assert_eq!(schema_pairs.len(), 59);

    for (code, severity) in &catalog_pairs {
        diagnostic["code"] = json!(code);
        diagnostic["severity"] = json!(severity);
        assert!(
            validator.is_valid(&diagnostic),
            "catalog diagnostic {code}/{severity} was rejected"
        );
    }

    diagnostic["code"] = json!("UNKNOWN_AUTHORING_DIAGNOSTIC");
    diagnostic["severity"] = json!("error");
    assert!(!validator.is_valid(&diagnostic));

    diagnostic["code"] = json!("AUTHORING_SCHEMA_INVALID");
    diagnostic["severity"] = json!("warning");
    assert!(!validator.is_valid(&diagnostic));
    diagnostic["code"] = json!("UNUSED_NODE_DEFINITION");
    diagnostic["severity"] = json!("error");
    assert!(!validator.is_valid(&diagnostic));

    diagnostic["severity"] = json!("warning");
    diagnostic["unknown"] = json!(true);
    assert!(!validator.is_valid(&diagnostic));
    diagnostic.as_object_mut().unwrap().remove("unknown");
    diagnostic.as_object_mut().unwrap().remove("message");
    assert!(!validator.is_valid(&diagnostic));
    diagnostic["message"] = json!("Unused node definition.");
    diagnostic.as_object_mut().unwrap().remove("source_path");
    assert!(!validator.is_valid(&diagnostic));
}
#[test]
fn v2scl002_evidence_schema_bounds_equal_the_domain_scale_envelope() {
    // ADR-0024 requires executable tests proving exact equality between a schema literal and the
    // domain constant it repeats, because a JSON Schema cannot import one. The evidence schemas
    // were registered without that binding, so a drift in any of these literals caught nothing.
    let result: serde_json::Value = serde_json::from_str(include_str!(
        "../../../assets/schemas/evidence-read-result-v1.schema.json"
    ))
    .expect("the evidence read result schema parses");
    let data = &result["properties"]["page"]["properties"]["data"]["oneOf"];

    // Page data is measured in JSON characters here and in encoded bytes in the daemon. The
    // schema bound is the weaker of the two, so stating the relation is what keeps them honest:
    // a character can never encode to fewer than one byte, so a payload inside the byte bound is
    // always inside this character bound.
    assert_eq!(
        data[2]["maxLength"],
        podway_core::MAX_EVIDENCE_PAGE_BYTES_V2,
        "text page data must repeat the page byte bound as its character ceiling"
    );
    assert_eq!(data[3]["maxItems"], podway_core::MAX_LIST_ENTRIES_V2);
    assert_eq!(
        data[3]["items"]["maxLength"],
        podway_core::MAX_LIST_ENTRY_SCALARS_V2
    );

    let components: serde_json::Value = serde_json::from_str(include_str!(
        "../../../assets/schemas/v2-result-components-v3.schema.json"
    ))
    .expect("the v3 result components schema parses");
    assert_eq!(
        components["$defs"]["pageToken"]["maxLength"],
        podway_core::MAX_EVIDENCE_PAGE_TOKEN_CHARS_V2
    );
    assert_eq!(
        components["$defs"]["pageTokenVersion"]["minimum"],
        u64::from(podway_protocol::EVIDENCE_PAGE_TOKEN_VERSION_V1)
    );
    assert_eq!(
        read_schema("schemas/v2-result-components-v4.schema.json")["$defs"]["evidenceItemMetadata"]
            ["properties"]["total_size"]["$ref"],
        "urn:podway:schema:v2-result-components:v4#/$defs/logicalSize"
    );
    // The suggestion window is driven by per-item work, so its schema ceiling and the projection's
    // truncation are one number. Binding them here is what the epic kept having to learn.
    let next: serde_json::Value = serde_json::from_str(include_str!(
        "../../../assets/schemas/next-result-v5.schema.json"
    ))
    .expect("the next result schema parses");
    assert_eq!(
        next["properties"]["suggestions"]["maxItems"],
        podway_core::MAX_SUGGESTION_WINDOW_V2
    );
    assert_eq!(
        next["properties"]["suggestions_total"]["maximum"],
        u64::MAX,
        "the exact total is unbounded; only the window is capped"
    );

    assert_eq!(
        components["$defs"]["logicalSize"]["maximum"],
        podway_core::MAX_TEXT_SCALARS_V2,
        "a logical size is measured in scalars, entries, or one value, so the widest is text"
    );
}
#[test]
fn v2scl002_evidence_read_result_conditional_branches_are_satisfiable_and_closed() {
    let base = examples()["podway.evidence-read-result/v1"].clone();

    let mut terminal_page = base.clone();
    terminal_page["page"] = json!({"offset": 7, "size": 5, "data": "final"});
    terminal_page["truncated"] = json!(false);
    terminal_page["next_page_token"] = Value::Null;
    assert_valid(
        "schemas/evidence-read-result-v1.schema.json",
        &terminal_page,
    );

    let mut token_without_truncation = terminal_page.clone();
    token_without_truncation["next_page_token"] = json!("AQIDBAUGBwgJCgsMDQ4PEA");
    assert_invalid(
        "schemas/evidence-read-result-v1.schema.json",
        &token_without_truncation,
    );

    let mut truncated_without_token = base.clone();
    truncated_without_token["next_page_token"] = Value::Null;
    assert_invalid(
        "schemas/evidence-read-result-v1.schema.json",
        &truncated_without_token,
    );

    let mut list_page = base.clone();
    list_page["item"] = json!({"item_id":"values","type":"list","revision":1});
    list_page["size_unit"] = json!("entries");
    list_page["total_size"] = json!(1000);
    list_page["page"] = json!({"offset": 0, "size": 2, "data": ["first", "second"]});
    assert_valid("schemas/evidence-read-result-v1.schema.json", &list_page);

    let mut list_page_with_text = list_page.clone();
    list_page_with_text["page"]["data"] = json!("not a list");
    assert_invalid(
        "schemas/evidence-read-result-v1.schema.json",
        &list_page_with_text,
    );

    let mut confirm_page = base.clone();
    confirm_page["item"] = json!({"item_id":"reviewed","type":"confirm","revision":1});
    confirm_page["size_unit"] = json!("value");
    confirm_page["total_size"] = json!(1);
    confirm_page["page"] = json!({"offset": 0, "size": 1, "data": true});
    confirm_page["truncated"] = json!(false);
    confirm_page["next_page_token"] = Value::Null;
    assert_valid("schemas/evidence-read-result-v1.schema.json", &confirm_page);

    let mut truncated_scalar = confirm_page.clone();
    truncated_scalar["truncated"] = json!(true);
    truncated_scalar["next_page_token"] = json!("AQIDBAUGBwgJCgsMDQ4PEA");
    assert_invalid(
        "schemas/evidence-read-result-v1.schema.json",
        &truncated_scalar,
    );

    for (item_type, data, invalid) in [
        ("integer", json!(42), json!("42")),
        ("choice", json!("accepted"), json!(1)),
        (
            "artifact",
            json!({
                "location_type": "reference",
                "location": "reports/review.md",
                "sha256_digest": DIGEST,
                "size_bytes": 1024,
                "media_type": "text/markdown"
            }),
            json!("reports/review.md"),
        ),
    ] {
        let mut page = base.clone();
        page["item"] = json!({"item_id":"value","type":item_type,"revision":1});
        page["size_unit"] = json!("value");
        page["total_size"] = json!(1);
        page["truncated"] = json!(false);
        page["next_page_token"] = Value::Null;
        page["page"] = json!({"offset": 0, "size": 1, "data": data});
        assert_valid("schemas/evidence-read-result-v1.schema.json", &page);
        page["page"]["data"] = invalid;
        assert_invalid("schemas/evidence-read-result-v1.schema.json", &page);
    }

    let mut unknown_field = base.clone();
    unknown_field["page_bytes"] = json!(64);
    assert_invalid(
        "schemas/evidence-read-result-v1.schema.json",
        &unknown_field,
    );
}
#[test]
fn v2scl002_next_result_v3_evidence_window_is_exact_and_never_truncated() {
    let mut next = examples()["podway.next-result/v5"].clone();
    next["readback"] = json!([{
        "source_graph_node_id": "source",
        "source_title": "Source",
        "source_attempt_id": UUID,
        "source_attempt_number": 1,
        "items_digest": DIGEST,
        "state": "resolved",
        "items": [{
            "item_id": "notes", "type": "text", "revision": 1,
            "value_digest": DIGEST, "total_size": 12, "size_unit": "scalars",
            "preview": "first", "preview_truncated": true,
            "next_page_token": "AQIDBAUGBwgJCgsMDQ4PEA"
        }],
        "items_total": 1,
        "items_truncated": false
    }]);
    next["readback_items_total"] = json!(1);
    assert_valid("schemas/next-result-v5.schema.json", &next);

    let mut truncated_window = next.clone();
    truncated_window["readback_items_truncated"] = json!(true);
    assert_invalid("schemas/next-result-v5.schema.json", &truncated_window);

    let mut truncated_reference = next.clone();
    truncated_reference["readback"][0]["items_truncated"] = json!(true);
    assert_invalid("schemas/next-result-v5.schema.json", &truncated_reference);

    let mut token_without_preview = next.clone();
    token_without_preview["readback"][0]["items"][0]
        .as_object_mut()
        .unwrap()
        .remove("preview");
    assert_invalid("schemas/next-result-v5.schema.json", &token_without_preview);

    let mut oversized_window = next.clone();
    oversized_window["readback_items_total"] = json!(129);
    assert_invalid("schemas/next-result-v5.schema.json", &oversized_window);

    let mut stale_state = next.clone();
    stale_state["readback"][0]["state"] = json!("stale");
    assert_invalid("schemas/next-result-v5.schema.json", &stale_state);

    let mut missing_truncation_flag = next.clone();
    missing_truncation_flag
        .as_object_mut()
        .unwrap()
        .remove("missing_required_items_truncated");
    assert_invalid(
        "schemas/next-result-v5.schema.json",
        &missing_truncation_flag,
    );
}
#[test]
fn v2scl002_observation_result_v3_pins_every_lifecycle_window() {
    let prepared = examples()["podway.observation-result/v5"].clone();
    assert_valid("schemas/observation-result-v5.schema.json", &prepared);

    let mut wrong_prepared_total = prepared.clone();
    wrong_prepared_total["mutation_templates_total"] = json!(2);
    assert_invalid(
        "schemas/observation-result-v5.schema.json",
        &wrong_prepared_total,
    );

    let mut status_with_values = prepared.clone();
    status_with_values["status"]["item_values"] = json!([]);
    assert_invalid(
        "schemas/observation-result-v5.schema.json",
        &status_with_values,
    );

    let mut terminal = prepared.clone();
    terminal["status"]["session"] = json!({"id":UUID,"lifecycle":"completed","revision":4});
    terminal["guidance"] = Value::Null;
    terminal["mutation_templates"] = json!([{
        "command": "session.terminal_disposition",
        "argv": ["podway", "disposition"],
        "preconditions": {"workspace_uuid": UUID, "session_id": UUID, "session_revision": 4},
        "authority": "optimistic_concurrency_only",
        "idempotency_key_required": true,
        "requires_explicit_authorization": false
    }]);
    terminal["mutation_templates_total"] = json!(1);
    assert_valid("schemas/observation-result-v5.schema.json", &terminal);

    let mut terminal_total_mismatch = terminal.clone();
    terminal_total_mismatch["mutation_templates_total"] = json!(2);
    assert_invalid(
        "schemas/observation-result-v5.schema.json",
        &terminal_total_mismatch,
    );

    let mut terminal_with_guidance = terminal.clone();
    terminal_with_guidance["guidance"] = examples()["podway.next-result/v5"].clone();
    assert_invalid(
        "schemas/observation-result-v5.schema.json",
        &terminal_with_guidance,
    );

    let mut oversized_argv = terminal.clone();
    oversized_argv["mutation_templates"][0]["argv"] = json!(["podway", "x".repeat(4_097)]);
    assert_invalid("schemas/observation-result-v5.schema.json", &oversized_argv);

    let disposed_template = |command: &str| {
        json!({
            "command": command,
            "argv": ["podway", "reset"],
            "preconditions": {"workspace_uuid": UUID, "session_id": UUID, "session_revision": 4},
            "authority": "optimistic_concurrency_only",
            "idempotency_key_required": true,
            "requires_explicit_authorization": true
        })
    };
    let mut cancelled = terminal.clone();
    cancelled["status"]["session"] = json!({"id":UUID,"lifecycle":"cancelled","revision":4});
    cancelled["mutation_templates"] = json!([
        disposed_template("session.reset"),
        disposed_template("session.start_replace")
    ]);
    cancelled["mutation_templates_total"] = json!(2);
    assert_valid("schemas/observation-result-v5.schema.json", &cancelled);

    let mut cancelled_total_mismatch = cancelled.clone();
    cancelled_total_mismatch["mutation_templates_total"] = json!(1);
    assert_invalid(
        "schemas/observation-result-v5.schema.json",
        &cancelled_total_mismatch,
    );

    let mut running = prepared.clone();
    running["status"]["session"] = json!({"id":UUID,"lifecycle":"running","revision":4});
    running["status"]["current"] = json!({
        "node": {"node_definition_id":"work","graph_node_id":"work","node_type":"action"},
        "attempt": {"attempt_id":UUID,"attempt_number":1},
        "readiness": {"items_satisfied":true,"unblocked":true,"goal_ready":true,"can_advance":true},
        "missing_required_item_count": 0,
        "blockers_total": 0
    });
    running["status"]["delegation"] = examples()["podway.next-result/v5"]["delegation"].clone();
    running["status"]["trace_length"] = json!(1);
    running["guidance"] = examples()["podway.next-result/v5"].clone();
    running["mutation_templates"] = json!([disposed_template("session.reset")]);
    running["mutation_templates_total"] = json!(1);
    assert_valid("schemas/observation-result-v5.schema.json", &running);

    let mut guidance_with_preview = running.clone();
    guidance_with_preview["guidance"]["readback"] = json!([{
        "source_graph_node_id": "source",
        "source_title": "Source",
        "source_attempt_id": UUID,
        "source_attempt_number": 1,
        "items_digest": DIGEST,
        "state": "resolved",
        "items": [{
            "item_id": "notes", "type": "text", "revision": 1,
            "value_digest": DIGEST, "total_size": 12, "size_unit": "scalars",
            "preview": "first", "preview_truncated": false
        }],
        "items_total": 1,
        "items_truncated": false
    }]);
    guidance_with_preview["guidance"]["readback_items_total"] = json!(1);
    assert_invalid(
        "schemas/observation-result-v5.schema.json",
        &guidance_with_preview,
    );

    let mut guidance_metadata_only = guidance_with_preview.clone();
    let item = &mut guidance_metadata_only["guidance"]["readback"][0]["items"][0];
    item.as_object_mut().unwrap().remove("preview");
    item.as_object_mut().unwrap().remove("preview_truncated");
    assert_valid(
        "schemas/observation-result-v5.schema.json",
        &guidance_metadata_only,
    );
}
#[test]
fn v2gsc004_current_and_frozen_counter_schemas_accept_their_exact_boundaries() {
    let fixtures = examples();
    let mut current_fixtures = vec![
        (
            "schemas/next-result-v5.schema.json",
            fixtures["podway.next-result/v5"].clone(),
        ),
        (
            "schemas/status-result-v5.schema.json",
            fixtures["podway.status-result/v5"].clone(),
        ),
        (
            "schemas/compact-status-result-v5.schema.json",
            fixtures["podway.compact-status-result/v5"].clone(),
        ),
    ];
    let active = &fixtures["podway.next-result/v5"];
    for (schema, value) in &mut current_fixtures[1..] {
        // Prepared sessions have no traversed graph counters. Exercise their zero bound separately.
        let mut prepared_with_counter = value.clone();
        prepared_with_counter["counters"] = graph_counters(1);
        assert_invalid(schema, &prepared_with_counter);
        value["session"]["lifecycle"] = json!("running");
        value["session"]["revision"] = json!(1);
        value["trace_length"] = json!(1);
        value["current"] = json!({
            "node":active["node"], "attempt":active["attempt"], "readiness":active["readiness"],
            "missing_required_item_count":0, "blockers_total":0
        });
        value["delegation"] = active["delegation"].clone();
        if *schema == "schemas/status-result-v5.schema.json" {
            value["terminal"] = json!(true);
            value["receipt_summaries"] = json!([value["delegation"]["receipt"]]);
            value["receipt_summaries_total"] = json!(1);
        }
    }
    for (schema, mut at_limit) in current_fixtures {
        at_limit["counters"] = graph_counters(1_024);
        assert_valid(schema, &at_limit);
        at_limit["counters"] = graph_counters(1_025);
        assert_invalid(schema, &at_limit);
    }

    let mut running_observation = fixtures["podway.observation-result/v5"].clone();
    running_observation["status"]["session"] =
        json!({"id":UUID,"lifecycle":"running","revision":4});
    running_observation["status"]["current"] = json!({
        "node": {"node_definition_id":"work","graph_node_id":"work","node_type":"action"},
        "attempt": {"attempt_id":UUID,"attempt_number":1},
        "readiness": {"items_satisfied":true,"unblocked":true,"goal_ready":true,"can_advance":true},
        "missing_required_item_count": 0,
        "blockers_total": 0
    });
    running_observation["status"]["delegation"] =
        fixtures["podway.next-result/v5"]["delegation"].clone();
    running_observation["status"]["trace_length"] = json!(1);
    running_observation["guidance"] = fixtures["podway.next-result/v5"].clone();
    running_observation["mutation_templates"] = json!([{
        "command": "session.reset",
        "argv": ["podway", "reset"],
        "preconditions": {
            "workspace_uuid": UUID,
            "session_id": UUID,
            "session_revision": 4
        },
        "authority": "optimistic_concurrency_only",
        "idempotency_key_required": true,
        "requires_explicit_authorization": true
    }]);
    running_observation["mutation_templates_total"] = json!(1);
    running_observation["status"]["counters"] = graph_counters(1_024);
    running_observation["guidance"]["counters"] = graph_counters(1_024);
    assert_valid(
        "schemas/observation-result-v5.schema.json",
        &running_observation,
    );
    running_observation["guidance"]["counters"] = graph_counters(1_025);
    assert_invalid(
        "schemas/observation-result-v5.schema.json",
        &running_observation,
    );
}
#[test]
fn v2lif002_prepared_result_families_enforce_the_reserved_lifecycle_boundary() {
    let fixtures = examples();

    let mut dry_run = fixtures["podway.session-start-result/v4"].clone();
    dry_run["dry_run"] = json!(true);
    for field in ["admission", "session_id", "revision"] {
        dry_run.as_object_mut().unwrap().remove(field);
    }
    assert_valid("schemas/session-start-result-v4.schema.json", &dry_run);
    let mut started = fixtures["podway.session-start-result/v4"].clone();
    started["active_attempt"] = json!({"attempt_id":UUID,"attempt_number":1});
    assert_invalid("schemas/session-start-result-v4.schema.json", &started);

    let mut begin_without_goal = fixtures["podway.session-begin-result/v2"].clone();
    begin_without_goal["goal_defined"] = json!(false);
    begin_without_goal["goal_revision"] = Value::Null;
    assert_valid(
        "schemas/session-begin-result-v2.schema.json",
        &begin_without_goal,
    );
    begin_without_goal["goal_revision"] = json!(1);
    assert_invalid(
        "schemas/session-begin-result-v2.schema.json",
        &begin_without_goal,
    );

    let mut disposition = fixtures["podway.terminal-disposition-result/v2"].clone();
    disposition["disposition"]["summary"] = json!("x".repeat(4001));
    assert_invalid(
        "schemas/terminal-disposition-result-v2.schema.json",
        &disposition,
    );
    let mut mismatched_disposition = fixtures["podway.terminal-disposition-result/v2"].clone();
    mismatched_disposition["disposition"]["session_revision"] = json!(3);
    assert_valid(
        "schemas/terminal-disposition-result-v2.schema.json",
        &mismatched_disposition,
    );
    assert_eq!(
        decode_result_schema_contract_v2(&mismatched_disposition),
        None
    );

    let mut prepared_reset = fixtures["podway.session-reset-result/v2"].clone();
    prepared_reset["session_revision"] = json!(1);
    assert_invalid(
        "schemas/session-reset-result-v2.schema.json",
        &prepared_reset,
    );
    let mut reset_dry_run = fixtures["podway.session-reset-result/v2"].clone();
    reset_dry_run["dry_run"] = json!(true);
    reset_dry_run["reset"] = json!(false);
    reset_dry_run.as_object_mut().unwrap().remove("admission");
    assert_valid(
        "schemas/session-reset-result-v2.schema.json",
        &reset_dry_run,
    );

    let reset_not_eligible = json!({
        "schema":"podway.session-reset-not-eligible-details/v2",
        "lifecycle":"completed",
        "current_terminal_disposition":false,
        "required_action":"record_disposition"
    });
    assert_valid(
        "schemas/session-reset-not-eligible-details-v2.schema.json",
        &reset_not_eligible,
    );
    let mut mismatched_reset_details = reset_not_eligible;
    mismatched_reset_details["required_action"] = json!("force");
    assert_invalid(
        "schemas/session-reset-not-eligible-details-v2.schema.json",
        &mismatched_reset_details,
    );

    let mut status = fixtures["podway.status-result/v5"].clone();
    status["current"] = json!({"graph_node_id":"work", "attempt":fixtures["podway.next-result/v5"]["attempt"], "readiness":"ready"});
    assert_invalid("schemas/status-result-v5.schema.json", &status);
    let mut compact = fixtures["podway.compact-status-result/v5"].clone();
    compact["trace_length"] = json!(1);
    assert_invalid("schemas/compact-status-result-v5.schema.json", &compact);

    let mut next = fixtures["podway.prepared-next-result/v3"].clone();
    let duplicate_suggestion = next["suggestions"][1].clone();
    next["suggestions"][0] = duplicate_suggestion;
    assert_invalid("schemas/prepared-next-result-v3.schema.json", &next);

    let mut observation = fixtures["podway.observation-result/v5"].clone();
    observation["mutation_templates"][0]["preconditions"]
        .as_object_mut()
        .unwrap()
        .remove("session_revision");
    assert_invalid("schemas/observation-result-v5.schema.json", &observation);
    let mut duplicate_template = fixtures["podway.observation-result/v5"].clone();
    let duplicate_reset = duplicate_template["mutation_templates"][1].clone();
    duplicate_template["mutation_templates"][0] = duplicate_reset;
    assert_invalid(
        "schemas/observation-result-v5.schema.json",
        &duplicate_template,
    );
}
#[test]
fn v2ctr003_decision_result_projections_match_the_immutable_record() {
    let decision = examples()["podway.decision-result/v2"].clone();
    assert_valid("schemas/decision-result-v2.schema.json", &decision);
    assert!(decode_result_schema_contract_v2(&decision).is_some());

    for (field, mismatched) in [
        ("graph_node_id", json!("other-node")),
        ("attempt_id", json!("00000000-0000-4000-8000-000000000002")),
        ("attempt_number", json!(2)),
        ("option_id", json!("retry")),
        ("effect", json!("rework")),
        ("target_graph_node_id", json!("other-target")),
    ] {
        let mut result = decision.clone();
        result[field] = mismatched;
        assert_valid("schemas/decision-result-v2.schema.json", &result);
        assert_eq!(
            decode_result_schema_contract_v2(&result),
            None,
            "decoder accepted mismatched decision projection {field}"
        );
    }
}
#[test]
fn v2aut001_output_v2_envelope_emits_schema_valid_authoring_results() {
    for (command, result) in [
        (
            "procedure.format",
            examples()["podway.procedure-source-result/v1"].clone(),
        ),
        ("procedure.lint", diagnostics_result("lint")),
    ] {
        let mut input = authoring_input(command, &result);
        input.warnings = vec![warning("ADVISORY")];
        let envelope = OutputEnvelopeV4::new(input).unwrap();
        assert_eq!(envelope.as_value()["command"], command);
        assert_eq!(envelope.as_value()["result"], result);
        assert_eq!(
            envelope.as_value()["warnings"],
            json!([warning("ADVISORY")])
        );

        let line = serde_json::to_string(&envelope).unwrap();
        assert!(line.len() <= MAX_DIRECT_OUTPUT_BYTES_V1);

        let value: Value = serde_json::from_str(&line).unwrap();
        assert_valid("schemas/output-v4.schema.json", &value);
        for absent in ["workspace", "job", "session"] {
            assert!(
                value.get(absent).is_none(),
                "local authoring output must omit {absent}"
            );
        }
        assert_eq!(
            serde_json::from_str::<OutputEnvelopeV4>(&line).unwrap(),
            envelope
        );
    }
}
#[test]
fn v2aut001_output_v2_envelope_embeds_a_real_authoring_diagnostic() {
    let diagnostic = representative_authoring_diagnostic();
    let diagnostic_value = serde_json::to_value(&diagnostic).expect("diagnostics serialize");
    for optional in [
        "node_definition_id",
        "graph_node_id",
        "related_graph_node_ids",
    ] {
        assert!(
            diagnostic_value.get(optional).is_some(),
            "the representative diagnostic must exercise {optional}"
        );
    }
    // The lone diagnostic object validates on its own terms, independent of any result it rides in.
    assert_valid(
        "schemas/authoring-diagnostic-v1.schema.json",
        &diagnostic_value,
    );

    let mut result = diagnostics_result("lint");
    result["valid"] = json!(false);
    result["diagnostics"] = json!([diagnostic_value.clone()]);
    result["diagnostics_total"] = json!(1);

    let envelope = OutputEnvelopeV4::new(authoring_input("procedure.lint", &result)).unwrap();
    let value = serde_json::to_value(&envelope).unwrap();
    // And it round-trips intact through the full v2 output envelope, resolving the schema's
    // cross-file `$ref` from `procedure-diagnostics-result-v1` into `authoring-diagnostic-v1`.
    assert_valid("schemas/output-v4.schema.json", &value);
    assert_eq!(value["result"]["diagnostics"], json!([diagnostic_value]));
}
#[test]
fn v2aut001_output_v2_envelope_rejects_unbound_results_and_open_warnings() {
    let source = examples()["podway.procedure-source-result/v1"].clone();
    let valid = serde_json::to_value(
        OutputEnvelopeV4::new(authoring_input("procedure.format", &source)).unwrap(),
    )
    .unwrap();
    assert_valid("schemas/output-v4.schema.json", &valid);

    let mut legacy = valid.clone();
    legacy["schema"] = json!("podway.output/v1");
    assert_invalid("schemas/output-v4.schema.json", &legacy);
    assert!(serde_json::from_value::<OutputEnvelopeV4>(legacy).is_err());

    let mut unknown_family = source.clone();
    unknown_family["schema"] = json!("podway.procedure-source-result/v2");
    let mut extra_field = source.clone();
    extra_field["unknown"] = json!(true);
    let mut released_v1_family = source.clone();
    released_v1_family["schema"] = json!("podway.procedure-validation-result/v1");
    for (command, result) in [
        ("procedure.lint", source.clone()),
        ("session.status", source.clone()),
        (
            "procedure.format",
            examples()["podway.status-result/v5"].clone(),
        ),
        ("procedure.format", unknown_family),
        ("procedure.format", extra_field),
        ("procedure.format", released_v1_family),
    ] {
        assert_eq!(
            OutputEnvelopeV4::new(authoring_input(command, &result)),
            Err(ProtocolError::InvalidCommandResult {
                command: command.to_owned()
            }),
            "{command} accepted {result}"
        );
        let mut rejected = valid.clone();
        rejected["command"] = json!(command);
        rejected["result"] = result;
        assert_invalid("schemas/output-v4.schema.json", &rejected);
    }

    let mut oversized = authoring_input("procedure.format", &source);
    oversized.warnings = vec![warning("ADVISORY"); MAX_V2_OUTPUT_WARNINGS + 1];
    let mut open = authoring_input("procedure.format", &source);
    let mut open_warning = warning("ADVISORY");
    open_warning.insert("unknown".to_owned(), json!(true));
    open.warnings = vec![open_warning];
    for input in [oversized, open] {
        let warnings = Value::Array(input.warnings.iter().cloned().map(Value::Object).collect());
        assert_eq!(
            OutputEnvelopeV4::new(input),
            Err(ProtocolError::InvalidCommandResult {
                command: "procedure.format".to_owned()
            })
        );
        let mut rejected = valid.clone();
        rejected["warnings"] = warnings;
        assert_invalid("schemas/output-v4.schema.json", &rejected);
    }
}
#[test]
fn v2aut001_validate_command_result_v2_binds_registered_families_to_their_routes() {
    let examples = examples();
    for contract in EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
    {
        let result = examples[contract.schema].clone();
        for command in contract.commands {
            assert!(
                validate_command_result_v2(command, &result).is_ok(),
                "{command} rejected {}",
                contract.schema
            );
        }
    }

    let source = examples["podway.procedure-source-result/v1"].clone();
    let diagnostics = diagnostics_result("format");
    assert!(validate_command_result_v2("procedure.format", &source).is_ok());
    assert!(validate_command_result_v2("procedure.format", &diagnostics).is_ok());
    assert!(validate_command_result_v2("procedure.lint", &diagnostics).is_ok());
    assert!(validate_command_result_v2("procedure.lint", &source).is_err());
    assert!(validate_command_result_v2("procedure.graph", &source).is_err());

    let mut released_v1_family = source.clone();
    released_v1_family.as_object_mut().unwrap().insert(
        "schema".to_owned(),
        json!("podway.procedure-validation-result/v1"),
    );
    assert!(validate_command_result_v2("procedure.validate", &released_v1_family).is_err());

    let mut missing = source;
    missing.as_object_mut().unwrap().remove("schema");
    assert!(validate_command_result_v2("procedure.format", &missing).is_err());
}
#[test]
fn v2ast002_reserves_closed_check_result_declaration_and_record_shapes() {
    let procedure = json!({
        "schema": "podway.procedure/v2",
        "id": "check-workflow",
        "version": "1",
        "name": "Check workflow",
        "purpose": "Reserve an external check result.",
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "Work",
                "intent": "Record the external check.",
                "items": [{
                    "id": "verification",
                    "type": "check_result",
                    "prompt": "Record the current external verification result.",
                    "required": true,
                    "operation_id": "make-test",
                    "operation_digest": DIGEST,
                    "accepted_outcomes": ["pass"]
                }]
            }
        },
        "graph": {"entry": "work", "nodes": [{"id": "work", "use": "work", "terminal": true}]}
    });
    assert_valid("schemas/procedure-v2.schema.json", &procedure);

    let mut duplicate_outcome = procedure.clone();
    duplicate_outcome["node_definitions"]["work"]["items"][0]["accepted_outcomes"] =
        json!(["pass", "pass"]);
    assert_invalid("schemas/procedure-v2.schema.json", &duplicate_outcome);
    let mut unknown_declaration_field = procedure;
    unknown_declaration_field["node_definitions"]["work"]["items"][0]["command"] =
        json!("make test");
    assert_invalid(
        "schemas/procedure-v2.schema.json",
        &unknown_declaration_field,
    );

    let mut record = check_result_value();
    record["type"] = json!("check_result");
    let input = json!({
        "schema": "podway.item-record-many-input/v1",
        "workspace_uuid": UUID,
        "session_id": UUID,
        "session_revision": 1,
        "attempt_id": UUID,
        "idempotency_key": "v2ast-002",
        "operations": [{"item_id": "verification", "expected_item_revision": 0, "record": record}]
    });
    assert_valid("schemas/item-record-many-input-v1.schema.json", &input);
    let mut missing_executor = input.clone();
    missing_executor["operations"][0]["record"]
        .as_object_mut()
        .unwrap()
        .remove("executor");
    assert_invalid(
        "schemas/item-record-many-input-v1.schema.json",
        &missing_executor,
    );
}
#[test]
fn v2ast002_complete_preview_readback_and_artifact_branches_are_disjoint() {
    let complete = check_result_value();
    assert_ref_valid(
        "urn:podway:schema:v2-result-components:v4#/$defs/checkResultValue",
        &complete,
    );
    assert_ref_valid(
        "urn:podway:schema:v2-result-components:v4#/$defs/itemValue",
        &complete,
    );
    assert_ref_invalid(
        "urn:podway:schema:v2-result-components:v4#/$defs/artifactValue",
        &complete,
    );

    let artifact = json!({
        "location_type": "reference", "location": "artifact:1", "sha256_digest": DIGEST,
        "size_bytes": 1, "media_type": "application/json"
    });
    assert_ref_valid(
        "urn:podway:schema:v2-result-components:v4#/$defs/artifactValue",
        &artifact,
    );
    assert_ref_invalid(
        "urn:podway:schema:v2-result-components:v4#/$defs/checkResultValue",
        &artifact,
    );

    let preview = check_result_preview();
    assert_ref_valid(
        "urn:podway:schema:v2-result-components:v4#/$defs/checkResultPreview",
        &preview,
    );
    assert_ref_invalid(
        "urn:podway:schema:v2-result-components:v4#/$defs/checkResultPreview",
        &complete,
    );
    let readback = json!({"item_id":"verification", "type":"check_result", "revision":1, "value_digest":DIGEST, "total_size":1, "size_unit":"bytes", "preview":check_result_preview(), "preview_truncated":false});
    assert_ref_valid(
        "urn:podway:schema:v2-result-components:v4#/$defs/evidenceItemMetadata",
        &readback,
    );
}
#[test]
fn v2ast002_reserves_observation_constraints_and_stdin_template_only_for_record_many() {
    let constraints = json!({
        "choices_total": 0,
        "choices_truncated": false,
        "operation_id": "make-test",
        "operation_digest": DIGEST,
        "accepted_outcomes": ["pass"]
    });
    assert_ref_valid(
        "urn:podway:schema:v2-result-components:v4#/$defs/itemConstraints",
        &constraints,
    );

    let template = json!({
        "command": "item.record_many",
        "argv": ["podway", "record", "--stdin"],
        "preconditions": {
            "workspace_uuid": UUID,
            "session_id": UUID,
            "attempt_id": UUID,
            "item_revision": 0
        },
        "authority": "optimistic_concurrency_only",
        "idempotency_key_required": true,
        "requires_explicit_authorization": false,
        "stdin_template": "{\"schema\":\"podway.item-record-many-input/v1\"}"
    });
    assert_ref_valid(
        "urn:podway:schema:v2-result-components:v4#/$defs/mutationTemplate",
        &template,
    );
    let mut wrong_command = template;
    wrong_command["command"] = json!("item.set");
    assert_ref_invalid(
        "urn:podway:schema:v2-result-components:v4#/$defs/mutationTemplate",
        &wrong_command,
    );

    let result = json!({
        "schema": "podway.item-record-many-result/v2",

        "changed": true,
        "graph_node_id": "work",
        "attempt_id": UUID,
        "attempt_number": 1,
        "revision": 2,
        "items": [{
            "item_id": "verification",
            "expected_item_revision": 0,
            "changed": true,
            "item_revision": 1
        }]
    });
    assert_valid("schemas/item-record-many-result-v2.schema.json", &result);
    // The result family stays record-kind agnostic. V2AST-002 reserved a disjoint check-result
    // entry carrying `type` and `value_digest`, but no producer ever emitted it, so the reserved
    // branch was withdrawn instead of remaining an unreachable published shape.
    let mut typed_entry = result.clone();
    typed_entry["items"][0]["type"] = json!("check_result");
    assert_invalid(
        "schemas/item-record-many-result-v2.schema.json",
        &typed_entry,
    );
}
#[test]
fn unsatisfied_check_result_reason_is_closed_and_check_result_only() {
    let mut item = json!({
        "item_id": "verification",
        "type": "check_result",
        "prompt": "Record the verification result.",
        "required": true,
        "required_now": true,
        "satisfied": false,
        "unsatisfied_reason": "operation_digest_mismatch",
        "revision": 1,
        "constraints": {
            "operation_id": "make-test",
            "operation_digest": DIGEST,
            "accepted_outcomes": ["pass"],
            "choices_total": 0,
            "choices_truncated": false
        },
        "value": check_result_preview(),
        "value_truncated": false
    });
    for reason in [
        "operation_id_mismatch",
        "operation_digest_mismatch",
        "outcome_not_accepted",
    ] {
        item["unsatisfied_reason"] = json!(reason);
        assert_ref_valid(
            "urn:podway:schema:observation-result:v5#/$defs/activeItem",
            &item,
        );
    }

    let mut open_vocabulary = item.clone();
    open_vocabulary["unsatisfied_reason"] = json!("input_basis_mismatch");
    assert_ref_invalid(
        "urn:podway:schema:observation-result:v5#/$defs/activeItem",
        &open_vocabulary,
    );

    // A reason only accompanies a stored, unsatisfied check result.
    let mut satisfied = item.clone();
    satisfied["satisfied"] = json!(true);
    assert_ref_invalid(
        "urn:podway:schema:observation-result:v5#/$defs/activeItem",
        &satisfied,
    );
    let mut empty_slot = item.clone();
    empty_slot["value"] = Value::Null;
    assert_ref_invalid(
        "urn:podway:schema:observation-result:v5#/$defs/activeItem",
        &empty_slot,
    );
    let mut unsatisfied_without_reason = item.clone();
    unsatisfied_without_reason
        .as_object_mut()
        .unwrap()
        .remove("unsatisfied_reason");
    assert_ref_valid(
        "urn:podway:schema:observation-result:v5#/$defs/activeItem",
        &unsatisfied_without_reason,
    );

    let text_item = json!({
        "item_id": "notes",
        "type": "text",
        "prompt": "Record notes.",
        "required": true,
        "required_now": true,
        "satisfied": false,
        "unsatisfied_reason": "outcome_not_accepted",
        "revision": 0,
        "constraints": {"min_length": 0, "max_length": 4000, "multiline": true, "choices_total": 0, "choices_truncated": false},
        "value": null,
        "value_truncated": false
    });
    assert_ref_invalid(
        "urn:podway:schema:observation-result:v5#/$defs/activeItem",
        &text_item,
    );
}
#[test]
fn v2grd002_reserves_closed_typed_condition_authoring_and_preview_shapes() {
    let procedure = json!({
        "schema": "podway.procedure/v2",
        "id": "guarded-workflow",
        "version": "1",
        "name": "Guarded workflow",
        "purpose": "Reserve bounded typed conditions.",
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "Work",
                "intent": "Record conditional evidence.",
                "items": [
                    {"id": "mode", "type": "choice", "prompt": "Select a mode.", "required": true, "choices": ["strict", "relaxed"]},
                    {"id": "notes", "type": "text", "prompt": "Record notes.", "required": false, "required_when": [{"item": "mode", "equals": "strict"}]}
                ]
            },
            "decide": {
                "type": "decision",
                "title": "Decide",
                "objective": "Select an available outcome.",
                "prompt": "What is the outcome?",
                "options": [
                    {"id": "approved", "label": "Approved", "criteria": "The recorded verdict is approved.", "guards": [{"evidence": {"node": "work", "item": "mode"}, "equals": "strict"}]},
                    {"id": "rework", "label": "Rework", "criteria": "More work is required."}
                ],
                "reason": {"required": true}
            }
        },
        "graph": {
            "entry": "work",
            "nodes": [
                {"id": "work", "use": "work", "next": "decide"},
                {"id": "decide", "use": "decide", "evidence_from": [{"node": "work", "required": true, "items": ["mode"]}], "routes": {"approved": {"to": "work", "effect": "rework"}, "rework": {"to": "work", "effect": "rework"}}}
            ]
        }
    });
    assert_valid("schemas/procedure-v2.schema.json", &procedure);

    let mut two_operators = procedure.clone();
    two_operators["node_definitions"]["work"]["items"][1]["required_when"][0]["not_equals"] =
        json!("relaxed");
    assert_invalid("schemas/procedure-v2.schema.json", &two_operators);
    let mut nested_expression = procedure.clone();
    nested_expression["node_definitions"]["decide"]["options"][0]["guards"][0]["or"] = json!([]);
    assert_invalid("schemas/procedure-v2.schema.json", &nested_expression);

    let preview = json!({
        "schema": "podway.procedure-preview-result/v1",
        "file": "workflow.yaml",
        "admissible": true,
        "procedure_schema": "podway.procedure/v2",
        "procedure_id": "guarded-workflow",
        "procedure_version": "1",
        "purpose": "Reserve bounded typed conditions.",
        "procedure_digest": DIGEST,
        "goal_tracking": false,
        "goal_assessment_graph_node_ids": [],
        "summary": {"definition_count": 2, "graph_node_count": 2, "action_node_count": 1, "decision_node_count": 1, "route_count": 2, "cycle_count": 1, "evidence_reference_count": 1, "skippable_node_count": 0, "manual_rework_target_count": 0},
        "checks": {"validate": true, "vet": true, "lint": true},
        "graph": {
            "entry_graph_node_id": "work",
            "terminal_graph_node_ids": ["work"],
            "nodes": [
                {"graph_node_id": "work", "node_definition_id": "work", "node_type": "action", "terminal": false, "skippable": false, "items": [{"item_id": "mode", "required": true}, {"item_id": "notes", "required": false, "required_when": [{"item": "mode", "equals": "strict"}]}]},
                {"graph_node_id": "decide", "node_definition_id": "decide", "node_type": "decision", "terminal": false, "skippable": false}
            ],
            "edges": [{"from_graph_node_id": "decide", "to_graph_node_id": "work", "effect": "rework", "option_id": "approved", "guards": [{"evidence": {"node": "work", "item": "mode"}, "equals": "strict"}]}]
        },
        "mermaid": "flowchart TD",
        "start_suggestion": {"command": "session.start", "argv": ["podway", "start", "--procedure", "workflow.yaml"]},
        "diagnostics": [],
        "diagnostics_truncated": false,
        "diagnostics_total": 0
    });
    assert_valid("schemas/procedure-preview-result-v1.schema.json", &preview);
}
#[test]
fn v2grd002_reserves_bounded_condition_result_and_error_shapes() {
    let predicate = typed_guard_predicate_status();
    assert_ref_valid(
        "urn:podway:schema:v2-result-components:v4#/$defs/predicateStatus",
        &predicate,
    );
    let mut text_echo = predicate.clone();
    text_echo["recorded_text"] = json!("secret");
    assert_ref_invalid(
        "urn:podway:schema:v2-result-components:v4#/$defs/predicateStatus",
        &text_echo,
    );

    let mut next = examples()["podway.next-result/v5"].clone();
    next["node"]["node_type"] = json!("decision");
    next.as_object_mut().unwrap().remove("intent");
    next.as_object_mut().unwrap().remove("instructions");
    next.as_object_mut().unwrap().remove("terminal");
    next["objective"] = json!("Choose an available outcome.");
    next["prompt"] = json!("What is the outcome?");
    next["reason_policy"] = json!({"required": true});
    next["options"] = json!([
        {"option_id": "approved", "label": "Approved", "criteria": "The verdict is approved.", "guards": [{"evidence": {"node": "review", "item": "verdict"}, "equals": "approved"}]},
        {"option_id": "rework", "label": "Rework", "criteria": "More work is required."}
    ]);
    next["allowed_option_ids"] = json!(["approved", "rework"]);
    next["option_guard_statuses"] = json!([
        {"option_id": "approved", "state": "met", "predicates": [predicate.clone()]},
        {"option_id": "rework", "state": "met", "predicates": []}
    ]);
    assert_valid("schemas/next-result-v5.schema.json", &next);

    let mut unguarded_next = next.clone();
    for option in unguarded_next["options"].as_array_mut().unwrap() {
        option.as_object_mut().unwrap().remove("guards");
    }
    unguarded_next
        .as_object_mut()
        .unwrap()
        .remove("option_guard_statuses");
    assert_valid("schemas/next-result-v5.schema.json", &unguarded_next);
    unguarded_next
        .as_object_mut()
        .unwrap()
        .remove("allowed_option_ids");
    assert_invalid("schemas/next-result-v5.schema.json", &unguarded_next);

    let active_item = json!({
        "item_id": "notes", "type": "text", "prompt": "Record notes.",
        "required": false, "required_now": true, "satisfied": false, "revision": 0,
        "condition_status": {"state": "met", "predicates": [{
            "source": {"kind": "item", "item_id": "mode"},
            "operator": "equals", "expected": "strict", "actual": "strict", "state": "met"
        }]},
        "constraints": {"min_length": 0, "max_length": 4000, "multiline": true, "choices_total": 0, "choices_truncated": false},
        "value": null, "value_truncated": false
    });
    assert_ref_valid(
        "urn:podway:schema:observation-result:v5#/$defs/activeItem",
        &active_item,
    );

    let mut unconditional_item = active_item.clone();
    unconditional_item["required"] = json!(true);
    unconditional_item
        .as_object_mut()
        .unwrap()
        .remove("condition_status");
    assert_ref_valid(
        "urn:podway:schema:observation-result:v5#/$defs/activeItem",
        &unconditional_item,
    );
    unconditional_item
        .as_object_mut()
        .unwrap()
        .remove("required_now");
    assert_ref_invalid(
        "urn:podway:schema:observation-result:v5#/$defs/activeItem",
        &unconditional_item,
    );

    let details = json!({
        "schema": "podway.v2-runtime-error-details/v2",
        "kind": "OPTION_GUARD_UNSATISFIED",

        "graph_node_id": "decide",
        "option_id": "approved",
        "state": "unmet",
        "predicates": [{
            "source": {"kind": "evidence", "graph_node_id": "review", "item_id": "verdict"},
            "operator": "equals", "expected": "approved", "actual": "changes-requested", "state": "unmet"
        }]
    });
    assert_valid("schemas/v2-runtime-error-details-v2.schema.json", &details);
    let envelope = json!({
        "schema": "podway.error/v2",
        "request_id": UUID,
        "command": "session.decide",
        "generated_at": "2026-08-22T00:00:00.000Z",
        "code": "OPTION_GUARD_UNSATISFIED",
        "message": "The selected decision option has an unmet or unevaluable guard.",
        "retryable": false,
        "exit_code": 1,
        "details": details.clone()
    });
    assert!(serde_json::from_value::<ErrorEnvelopeV2>(envelope).is_ok());

    let mut retryable_shape = details;
    retryable_shape["recovery"] = json!({});
    assert_invalid(
        "schemas/v2-runtime-error-details-v2.schema.json",
        &retryable_shape,
    );
}
#[test]
fn v2grd002_reserves_guard_response_budgets() {
    let maximal = json!({
        "source": {"kind": "evidence", "graph_node_id": maximum_identifier(0), "item_id": maximum_identifier(1)},
        "operator": "equals",
        "expected": escape_heavy(120),
        "actual": escape_heavy(120),
        "state": "met"
    });
    assert_ref_valid(
        "urn:podway:schema:v2-result-components:v4#/$defs/predicateStatus",
        &maximal,
    );
    let statuses = (0..8)
        .map(|index| json!({"option_id": maximum_identifier(index), "state": "met", "predicates": [maximal.clone(), maximal.clone(), maximal.clone(), maximal.clone()]}))
        .collect::<Vec<_>>();
    let guard_bytes = serde_json::to_vec(&statuses).unwrap().len();
    assert!(
        guard_bytes <= 96 * 1_024,
        "guard statuses used {guard_bytes} bytes"
    );

    let condition = json!({"state": "met", "predicates": [maximal.clone(), maximal.clone(), maximal.clone(), maximal]});
    let condition_bytes = serde_json::to_vec(&condition).unwrap().len();
    assert!(
        condition_bytes < 128 * 1_024,
        "condition status used {condition_bytes} bytes"
    );
}

fn current_runtime_error_fields(code: &str) -> Map<String, Value> {
    let fields = match code {
        "PROCEDURE_V2_SCHEMA_INVALID" => json!({"diagnostic_codes":["AUTHORING_SCHEMA_INVALID"]}),
        "GRAPH_NODE_NOT_FOUND" | "DECISION_REASON_MISSING" => json!({"graph_node_id":"review"}),
        "NODE_DEFINITION_NOT_FOUND" => json!({"node_definition_id":"review"}),
        "GRAPH_NODE_TYPE_MISMATCH" => {
            json!({"graph_node_id":"review","expected_node_type":"decision","actual_node_type":"action"})
        }
        "OPTION_NOT_ALLOWED" => {
            json!({"graph_node_id":"review","option_id":"reject","allowed_option_ids":["accept"]})
        }
        "OPTION_GUARD_UNSATISFIED" => json!({
            "graph_node_id":"review",
            "option_id":"accept",
            "state":"unmet",
            "predicates":[{
                "source":{"kind":"evidence","graph_node_id":"build","item_id":"verdict"},
                "operator":"equals",
                "expected":"approved",
                "actual":"changes-requested",
                "state":"unmet"
            }]
        }),
        "ROUTE_NOT_ALLOWED" => json!({"graph_node_id":"review","option_id":"accept"}),
        "EVIDENCE_REFERENCE_UNRESOLVED" => {
            json!({"graph_node_id":"review","source_graph_node_ids":["build"]})
        }
        "EVIDENCE_REFERENCE_STALE" => {
            json!({"graph_node_id":"review","source_graph_node_id":"build","expected_source_attempt_id":UUID,"current_source_attempt_id":null})
        }
        "MANUAL_REWORK_TARGET_NOT_ALLOWED"
        | "MANUAL_REWORK_TARGET_NOT_ON_TRACE"
        | "GOAL_REVISION_TARGET_NOT_ALLOWED"
        | "GOAL_REVISION_TARGET_NOT_REVISION_SAFE" => json!({"target_graph_node_id":"build"}),
        "GOAL_TRACKING_NOT_ENABLED" | "SESSION_GOAL_MISSING" | "REACTIVATION_FLAG_REQUIRED" => {
            json!({})
        }
        "SESSION_GOAL_ALREADY_DEFINED" | "FRESH_GOAL_ASSESSMENT_MISSING" => {
            json!({"goal_revision":1})
        }
        "GOAL_REVISION_STALE" => json!({"expected_goal_revision":1,"actual_goal_revision":2}),
        "CRITERION_MODE_MIXED" => {
            json!({"criterion_id":"tests","expected_mode":"assessment","actual_status":"not_applicable"})
        }
        "CRITERION_CITATION_INVALID" => {
            json!({"criterion_id":"tests","citation":{"local_item_id":"evidence"}})
        }
        "CRITERION_RESULT_MISSING" => json!({"missing_criterion_ids":["tests"]}),
        "CRITERION_NOT_FOUND" => json!({"criterion_id":"tests"}),
        "GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED" => {
            json!({"option_id":"accept","determined_outcome":"achieved","allowed_option_ids":["reject"]})
        }
        "DIGEST_CONFIRMATION_REQUIRED" => {
            json!({"procedure_digest":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"})
        }
        "EVIDENCE_NOT_AVAILABLE" => {
            json!({"graph_node_id":"review","source_graph_node_id":"implement","item_id":"notes","reference_state":"skipped"})
        }
        "ATTEMPT_CONTENT_LIMIT_EXCEEDED" => {
            json!({"field":"items.notes.value","actual":16_777_217,"maximum":16_777_216,"unit":"scalars"})
        }
        "ITEM_CONSTRAINT_FAILED" => {
            json!({"field":"item list content","actual":300_001,"maximum":300_000,"unit":"scalars"})
        }
        "EVIDENCE_PAGE_TOKEN_EXHAUSTED" => {
            json!({"source_graph_node_id":"implement","item_id":"notes","offset":4_000,"total_size":4_000,"unit":"scalars"})
        }
        "EVIDENCE_PAGE_TOKEN_STALE" => json!({
            "graph_node_id":"review",
            "source_graph_node_id":"implement",
            "item_id":"notes",
            "expected_value_digest":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "current_value_digest":"sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        }),
        _ => panic!("unknown current runtime error fixture: {code}"),
    };
    fields.as_object().unwrap().clone()
}

fn current_runtime_error_details(code: &str) -> Value {
    let recoverable = matches!(
        code,
        "EVIDENCE_REFERENCE_STALE" | "GOAL_REVISION_STALE" | "EVIDENCE_PAGE_TOKEN_STALE"
    );
    let mut fields = current_runtime_error_fields(code);
    fields.insert(
        "schema".to_owned(),
        json!(if recoverable {
            "podway.recoverable-v2-runtime-error-details/v2"
        } else {
            "podway.v2-runtime-error-details/v2"
        }),
    );
    fields.insert("kind".to_owned(), json!(code));
    if recoverable {
        fields.insert(
            "recovery".to_owned(),
            examples()["podway.recoverable-v2-runtime-error-details/v2"]["recovery"].clone(),
        );
    }
    Value::Object(fields)
}

fn current_error_envelope(code: &str, details: Value) -> Value {
    let (_, exit, retryable) = podway_protocol::error_code_catalog_v1()
        .find(|(registered, _, _)| *registered == code)
        .expect("registered current error");
    json!({
        "schema":"podway.error/v2", "request_id":UUID, "command":"session.complete",
        "generated_at":"2026-10-06T00:00:00.000Z", "code":code,
        "message":"The command must satisfy its current contract.",
        "retryable":retryable, "exit_code":exit, "details":details
    })
}

fn current_error_validator() -> jsonschema::Validator {
    let mut schema = read_schema("schemas/error-v2.schema.json");
    schema.as_object_mut().unwrap().remove("$id");
    jsonschema::draft202012::options()
        .with_base_uri(format!("{SCHEMA_BASE}error-v2.schema.json"))
        .with_retriever(local_schemas())
        .should_validate_formats(true)
        .build(&schema)
        .unwrap()
}

fn assert_current_error_validation(
    validator: &jsonschema::Validator,
    value: &Value,
    expected: bool,
) {
    assert_eq!(validator.is_valid(value), expected, "canonical: {value}");
    assert_eq!(
        podway_protocol::validate_daemon_free_document_v1("podway.error/v2", value),
        expected,
        "selected: {value}"
    );
    assert_eq!(
        serde_json::from_value::<ErrorEnvelopeV2>(value.clone()).is_ok(),
        expected,
        "typed: {value}"
    );
}

#[test]
fn current_runtime_errors_bind_each_code_to_its_own_closed_details() {
    let validator = current_error_validator();
    for code in [
        "PROCEDURE_V2_SCHEMA_INVALID",
        "GRAPH_NODE_NOT_FOUND",
        "NODE_DEFINITION_NOT_FOUND",
        "GRAPH_NODE_TYPE_MISMATCH",
        "OPTION_NOT_ALLOWED",
        "OPTION_GUARD_UNSATISFIED",
        "ROUTE_NOT_ALLOWED",
        "DECISION_REASON_MISSING",
        "EVIDENCE_REFERENCE_UNRESOLVED",
        "EVIDENCE_REFERENCE_STALE",
        "MANUAL_REWORK_TARGET_NOT_ALLOWED",
        "MANUAL_REWORK_TARGET_NOT_ON_TRACE",
        "GOAL_TRACKING_NOT_ENABLED",
        "SESSION_GOAL_MISSING",
        "SESSION_GOAL_ALREADY_DEFINED",
        "GOAL_REVISION_STALE",
        "GOAL_REVISION_TARGET_NOT_ALLOWED",
        "GOAL_REVISION_TARGET_NOT_REVISION_SAFE",
        "REACTIVATION_FLAG_REQUIRED",
        "CRITERION_MODE_MIXED",
        "CRITERION_CITATION_INVALID",
        "CRITERION_RESULT_MISSING",
        "CRITERION_NOT_FOUND",
        "FRESH_GOAL_ASSESSMENT_MISSING",
        "GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED",
        "DIGEST_CONFIRMATION_REQUIRED",
        "EVIDENCE_NOT_AVAILABLE",
        "ATTEMPT_CONTENT_LIMIT_EXCEEDED",
        "EVIDENCE_PAGE_TOKEN_EXHAUSTED",
        "EVIDENCE_PAGE_TOKEN_STALE",
    ] {
        let valid = current_error_envelope(code, current_runtime_error_details(code));
        assert_current_error_validation(&validator, &valid, true);
        let other = match code {
            "EVIDENCE_REFERENCE_STALE" | "EVIDENCE_PAGE_TOKEN_STALE" => "GOAL_REVISION_STALE",
            "GOAL_REVISION_STALE" => "EVIDENCE_REFERENCE_STALE",
            "GOAL_TRACKING_NOT_ENABLED" => "GRAPH_NODE_NOT_FOUND",
            _ => "GOAL_TRACKING_NOT_ENABLED",
        };
        let mut mismatched = valid.clone();
        // This is a valid complete branch of the same detail family, rather than malformed data.
        mismatched["details"] = current_runtime_error_details(other);
        assert!(podway_protocol::validate_daemon_free_document_v1(
            mismatched["details"]["schema"].as_str().unwrap(),
            &mismatched["details"]
        ));
        assert_current_error_validation(&validator, &mismatched, false);
        let mut missing_kind = valid.clone();
        missing_kind["details"]
            .as_object_mut()
            .unwrap()
            .remove("kind");
        assert_current_error_validation(&validator, &missing_kind, false);
        let mut unknown = valid.clone();
        unknown["details"]["credential"] = json!("unexpected");
        assert_current_error_validation(&validator, &unknown, false);
    }
}

#[test]
fn current_item_constraint_errors_validate_structured_bounds_and_keep_generic_details() {
    let validator = current_error_validator();
    for details in [json!({}), json!({"constraint":"multiline"})] {
        assert_current_error_validation(
            &validator,
            &current_error_envelope("ITEM_CONSTRAINT_FAILED", details),
            true,
        );
    }
    let valid = current_error_envelope(
        "ITEM_CONSTRAINT_FAILED",
        current_runtime_error_details("ITEM_CONSTRAINT_FAILED"),
    );
    assert_current_error_validation(&validator, &valid, true);
    let mut wrong_kind = valid.clone();
    wrong_kind["details"] = current_runtime_error_details("ATTEMPT_CONTENT_LIMIT_EXCEEDED");
    assert!(podway_protocol::validate_daemon_free_document_v1(
        "podway.v2-runtime-error-details/v2",
        &wrong_kind["details"]
    ));
    assert_current_error_validation(&validator, &wrong_kind, false);
    for field in ["schema", "kind", "actual", "maximum", "unit"] {
        let mut missing = valid.clone();
        missing["details"].as_object_mut().unwrap().remove(field);
        assert_current_error_validation(&validator, &missing, false);
    }
    let mut unknown = valid;
    unknown["details"]["credential"] = json!("unexpected");
    assert_current_error_validation(&validator, &unknown, false);
}

#[test]
fn current_delegation_errors_bind_each_code_to_its_exact_reason_set() {
    let validator = current_error_validator();
    for (code, accepted) in [
        ("DELEGATION_NOT_APPLICABLE", &["direct_attempt"][..]),
        ("DELEGATION_RECEIPT_REQUIRED", &["receipt_missing"][..]),
        (
            "DELEGATION_RECEIPT_STALE",
            &["package_changed", "items_changed"][..],
        ),
        (
            "DELEGATION_BINDING_MISMATCH",
            &[
                "binding_mismatch",
                "role_mismatch",
                "disposition_mismatch",
                "round_limit",
            ][..],
        ),
        ("DELEGATION_ALREADY_ACCEPTED", &["duplicate_acceptance"][..]),
    ] {
        for reason in [
            "direct_attempt",
            "receipt_missing",
            "package_changed",
            "items_changed",
            "binding_mismatch",
            "role_mismatch",
            "duplicate_acceptance",
            "disposition_mismatch",
            "round_limit",
            "item_not_ready",
            "blocker_open",
        ] {
            let value = current_error_envelope(
                code,
                json!({"schema":"podway.delegation-error-details/v2", "reason":reason}),
            );
            assert_current_error_validation(&validator, &value, accepted.contains(&reason));
        }
        let mut open = current_error_envelope(
            code,
            json!({"schema":"podway.delegation-error-details/v2", "reason":accepted[0]}),
        );
        open["details"]["credential"] = json!("unexpected");
        assert_current_error_validation(&validator, &open, false);
    }
}

#[test]
fn current_authoring_registry_has_the_exact_expected_families_and_routes() {
    let actual = EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
        .flat_map(|contract| {
            contract
                .commands
                .iter()
                .filter(|command| command.starts_with("procedure."))
                .map(move |command| (contract.schema, *command))
        })
        .collect::<BTreeSet<_>>();
    let mut expected = BTreeSet::from([
        (
            "podway.procedure-validation-result/v2",
            "procedure.validate",
        ),
        ("podway.procedure-source-result/v1", "procedure.format"),
        ("podway.procedure-source-result/v1", "procedure.scaffold"),
        ("podway.procedure-graph-result/v1", "procedure.graph"),
        ("podway.procedure-graph-result/v2", "procedure.graph"),
        ("podway.procedure-preview-result/v1", "procedure.preview"),
        ("podway.procedure-preview-result/v2", "procedure.preview"),
    ]);
    for command in [
        "procedure.format",
        "procedure.validate",
        "procedure.vet",
        "procedure.lint",
        "procedure.check",
        "procedure.graph",
        "procedure.scaffold",
    ] {
        expected.insert(("podway.procedure-diagnostics-result/v1", command));
    }
    assert_eq!(actual, expected);
    for (schema, command) in expected {
        assert!(
            validate_command_result_v2(command, &examples()[schema]).is_ok(),
            "{schema}: {command}"
        );
    }
}

fn current_detail_fixtures() -> Value {
    serde_json::from_str::<Value>(include_str!(
        "../../../tests/fixtures/v2/protocol/current-contracts.json"
    ))
    .unwrap()["fixtures"]
        .clone()
}

#[test]
fn current_error_details_preserve_closed_actions_conflicts_and_identity_fences() {
    let validator = current_error_validator();
    let fixtures = current_detail_fixtures();
    for (code, schema) in [
        (
            "SESSION_START_DECISION_REQUIRED",
            "podway.session-start-decision-required-details/v2",
        ),
        (
            "SESSION_START_STATE_CONFLICT",
            "podway.session-start-state-conflict-details/v2",
        ),
        (
            "SESSION_REVISION_CONFLICT",
            "podway.revision-conflict-details/v3",
        ),
        ("ATTEMPT_NOT_CURRENT", "podway.attempt-conflict-details/v3"),
        (
            "IDEMPOTENCY_KEY_REUSED",
            "podway.idempotency-key-reused-details/v2",
        ),
        ("BLOCKER_LIMIT_REACHED", "podway.blocker-limit-details/v2"),
        (
            "WORKSPACE_UUID_MISMATCH",
            "podway.workspace-uuid-mismatch-details/v3",
        ),
        (
            "SESSION_ID_MISMATCH",
            "podway.session-id-mismatch-details/v3",
        ),
        (
            "PROCEDURE_DIGEST_MISMATCH",
            "podway.procedure-digest-mismatch-details/v3",
        ),
    ] {
        let details = fixtures[schema].clone();
        assert_current_error_validation(
            &validator,
            &current_error_envelope(code, details.clone()),
            true,
        );
        let mut open = details.clone();
        open["unexpected"] = json!(true);
        assert_current_error_validation(&validator, &current_error_envelope(code, open), false);
        for field in details.as_object().unwrap().keys() {
            let mut missing = details.clone();
            missing.as_object_mut().unwrap().remove(field);
            let valid = matches!(
                (code, field.as_str()),
                (
                    "SESSION_REVISION_CONFLICT",
                    "expected_revision" | "current_revision"
                ) | ("ATTEMPT_NOT_CURRENT", "actual_attempt_id")
            );
            assert_current_error_validation(
                &validator,
                &current_error_envelope(code, missing),
                valid,
            );
        }
    }
    let mut revision = fixtures["podway.revision-conflict-details/v3"].clone();
    revision
        .as_object_mut()
        .unwrap()
        .remove("expected_revision");
    revision.as_object_mut().unwrap().remove("current_revision");
    assert_current_error_validation(
        &validator,
        &current_error_envelope("SESSION_REVISION_CONFLICT", revision),
        false,
    );
    for (code, schema) in [
        (
            "SESSION_START_DECISION_REQUIRED",
            "podway.session-start-decision-required-details/v2",
        ),
        (
            "SESSION_START_STATE_CONFLICT",
            "podway.session-start-state-conflict-details/v2",
        ),
    ] {
        let mut duplicate = fixtures[schema].clone();
        duplicate["allowed_actions"] = json!(["continue", "continue"]);
        assert_current_error_validation(
            &validator,
            &current_error_envelope(code, duplicate),
            false,
        );
    }
    let mut workspace = fixtures["podway.workspace-uuid-mismatch-details/v3"].clone();
    workspace["actual_workspace_uuid"] = Value::Null;
    assert_current_error_validation(
        &validator,
        &current_error_envelope("WORKSPACE_UUID_MISMATCH", workspace),
        false,
    );
    for actual in [Value::Null, json!(UUID)] {
        let mut session = fixtures["podway.session-id-mismatch-details/v3"].clone();
        session["actual_session_id"] = actual;
        assert_current_error_validation(
            &validator,
            &current_error_envelope("SESSION_ID_MISMATCH", session),
            true,
        );
    }
    let mut session = fixtures["podway.session-id-mismatch-details/v3"].clone();
    session["expected_session_id"] = json!("not-a-uuid");
    assert_current_error_validation(
        &validator,
        &current_error_envelope("SESSION_ID_MISMATCH", session),
        false,
    );
}

#[test]
fn current_reset_eligibility_details_preserve_the_lifecycle_action_matrix() {
    let validator = current_error_validator();
    let base = current_detail_fixtures()["podway.session-reset-not-eligible-details/v2"].clone();
    for (lifecycle, action, valid) in [
        ("running", "force", true),
        ("completed", "record_disposition", true),
        ("cancelled", "record_disposition", true),
        ("running", "record_disposition", false),
        ("completed", "force", false),
        ("cancelled", "force", false),
        ("prepared", "force", false),
    ] {
        let mut details = base.clone();
        details["lifecycle"] = json!(lifecycle);
        details["required_action"] = json!(action);
        assert_current_error_validation(
            &validator,
            &current_error_envelope("SESSION_RESET_NOT_ELIGIBLE", details),
            valid,
        );
    }
    for field in base.as_object().unwrap().keys() {
        let mut details = base.clone();
        details.as_object_mut().unwrap().remove(field);
        assert_current_error_validation(
            &validator,
            &current_error_envelope("SESSION_RESET_NOT_ELIGIBLE", details),
            false,
        );
    }
    for (field, value) in [
        ("current_terminal_disposition", json!(true)),
        ("unexpected", json!(true)),
        (
            "schema",
            json!("podway.session-reset-not-eligible-details/v1"),
        ),
    ] {
        let mut details = base.clone();
        details[field] = value;
        assert_current_error_validation(
            &validator,
            &current_error_envelope("SESSION_RESET_NOT_ELIGIBLE", details),
            false,
        );
    }
}

#[test]
fn current_generic_error_depth_is_measured_from_the_full_envelope() {
    let mut nested = Value::Null;
    for _ in 0..podway_protocol::MAX_JSON_DEPTH_V1 - 1 {
        nested = json!({"nested": nested});
    }
    let mut error = current_error_envelope("REQUEST_INVALID", nested.clone());
    assert!(podway_protocol::validate_daemon_free_document_v1(
        "podway.error/v2",
        &error
    ));
    assert!(serde_json::from_value::<ErrorEnvelopeV2>(error.clone()).is_ok());
    error["details"] = json!({"nested": nested});
    assert!(!podway_protocol::validate_daemon_free_document_v1(
        "podway.error/v2",
        &error
    ));
    assert!(serde_json::from_value::<ErrorEnvelopeV2>(error).is_err());
}

#[test]
fn current_recovery_recipes_preserve_closed_bounds_and_explicit_authority() {
    let base = current_detail_fixtures()["podway.revision-conflict-details/v3"]["recovery"].clone();
    let valid = |value: &Value| {
        assert_valid("schemas/recovery-recipe-v2.schema.json", value);
        assert!(serde_json::from_value::<podway_protocol::RecoveryRecipeV2>(value.clone()).is_ok());
    };
    let invalid = |value: Value| {
        assert_invalid("schemas/recovery-recipe-v2.schema.json", &value);
        assert!(serde_json::from_value::<podway_protocol::RecoveryRecipeV2>(value).is_err());
    };
    valid(&base);
    let mut maximum = base.clone();
    maximum["reason"] = json!("界".repeat(256));
    valid(&maximum);
    for (field, value) in [
        ("reason", json!("界".repeat(257))),
        ("reason", json!("")),
        ("unexpected", json!(true)),
        ("requires_explicit_authorization", json!(true)),
        ("command", json!("session.reset")),
        ("argv", json!(["podway", "reset", "--yes"])),
        ("argv", json!(["podway", "--json", "observe", "extra"])),
        ("argv", json!(["x".repeat(257), "--json", "observe"])),
    ] {
        let mut candidate = base.clone();
        candidate[field] = value;
        invalid(candidate);
    }
}

#[test]
fn archive_list_and_purge_results_decode_and_bind_to_their_retained_commands() {
    let summary = json!({
        "session_id": UUID,
        "task_title": "Archived task",
        "lifecycle": "completed",
        "session_revision": 2,
        "archive_slot": 1,
        "archived_at_ms": 1,
        "archived_workspace_revision": 3,
    });
    for (command, schema_path, result) in [
        (
            "session.archive_list",
            "schemas/session-archive-list-result-v2.schema.json",
            json!({
                "schema": "podway.session-archive-list-result/v2",
                "count": 1, "truncated": false,
                "oldest": summary.clone(), "sessions": [summary.clone()],
            }),
        ),
        (
            "session.archive_purge",
            "schemas/session-archive-purge-result-v1.schema.json",
            json!({
                "schema": "podway.session-archive-purge-result/v1",
                "purged": true, "session": summary,
            }),
        ),
    ] {
        assert_valid(schema_path, &result);
        let contract = decode_result_schema_contract_v2(&result).expect("retained archive result");
        assert_eq!(contract.schema_path, schema_path);
        validate_command_result_v2(command, &result).unwrap();
        assert!(validate_command_result_v2("session.archive_show", &result).is_err());
        let mut invalid = result;
        invalid
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_owned(), json!(true));
        assert!(decode_result_schema_contract_v2(&invalid).is_none());
        assert!(validate_command_result_v2(command, &invalid).is_err());
    }
}

#[test]
fn current_results_round_trip_every_registered_direct_command_and_reject_discriminators() {
    let fixtures = examples();
    let contracts = EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
        .chain(
            DAEMON_FREE_DOCUMENT_SCHEMAS_V1
                .iter()
                .filter(|entry| !entry.commands.is_empty()),
        );
    for contract in contracts {
        let result = &fixtures[contract.schema];
        for command in contract.commands {
            let mut input = authoring_input(command, result);
            if !command.starts_with("procedure.")
                && !matches!(*command, "version" | "workspace.remove")
            {
                input.workspace = Some(
                    serde_json::from_value::<podway_protocol::WorkspaceOutputV1>(json!({
                        "uuid":UUID, "root":"/tmp/podway-example", "latest_workspace_sequence":1
                    }))
                    .unwrap(),
                );
            }
            let output = OutputEnvelopeV4::new(input).unwrap();
            let bytes = serde_json::to_vec(&output).unwrap();
            let decoded: OutputEnvelopeV4 = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                decoded.as_value(),
                output.as_value(),
                "{command}: {}",
                contract.schema
            );
        }
    }
    for result in [
        json!({}),
        json!({"schema": 1}),
        json!({"schema": "podway.unknown-result/v1"}),
        Value::Null,
        json!([]),
        json!(true),
        json!("text"),
    ] {
        assert!(decode_result_schema_contract_v2(&result).is_none());
        assert!(validate_command_result_v2("procedure.format", &result).is_err());
        assert!(validate_command_result_v2("version", &result).is_err());
    }
    let mut incomplete = fixtures["podway.next-result/v5"].clone();
    incomplete.as_object_mut().unwrap().remove("node");
    assert!(decode_result_schema_contract_v2(&incomplete).is_none());
}

#[test]
fn current_result_decoders_preserve_deep_closed_family_and_variant_failures() {
    let fixtures = examples();
    let mut invalid = Vec::new();
    for (schema, pointer) in [
        ("podway.compact-status-result/v5", "/procedure"),
        ("podway.next-result/v5", "/node"),
        ("podway.procedure-preview-result/v1", "/checks"),
        ("podway.decision-result/v2", "/record"),
        ("podway.goal-definition-result/v2", "/criteria/0"),
        ("podway.goal-revision-result/v2", "/criteria/0"),
        ("podway.criterion-assessment-result/v2", "/result"),
    ] {
        let mut value = fixtures[schema].clone();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown".to_owned(), json!(true));
        invalid.push(value);
    }
    for (schema, field, replacement) in [
        (
            "podway.procedure-validation-result/v2",
            "valid",
            json!("true"),
        ),
        (
            "podway.procedure-source-result/v1",
            "operation",
            json!("unknown"),
        ),
        (
            "podway.procedure-source-result/v1",
            "template",
            json!("minimal"),
        ),
        (
            "podway.procedure-diagnostics-result/v1",
            "diagnostics",
            json!([{"unknown":true}]),
        ),
        (
            "podway.procedure-graph-result/v1",
            "format",
            json!("unknown"),
        ),
        ("podway.rework-result/v2", "reason", json!("x".repeat(2001))),
        ("podway.next-result/v5", "title", json!("x".repeat(121))),
        ("podway.next-result/v5", "intent", json!("")),
        (
            "podway.next-result/v5",
            "allowed_actions",
            json!(["session.explode"]),
        ),
        (
            "podway.next-result/v5",
            "suggestions",
            json!([{"command":"session.explode","argv":["explode"]}]),
        ),
        (
            "podway.next-result/v5",
            "next_graph_node_id",
            json!("finish"),
        ),
    ] {
        let mut value = fixtures[schema].clone();
        value[field] = replacement;
        invalid.push(value);
    }
    for pointer in ["/record/procedure_snapshot_id", "/target_attempt_id"] {
        let mut value = fixtures["podway.decision-result/v2"].clone();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        if parent.is_empty() {
            value.as_object_mut().unwrap().remove(key);
        } else {
            value
                .pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
        }
        invalid.push(value);
    }
    for value in invalid {
        assert!(
            decode_result_schema_contract_v2(&value).is_none(),
            "accepted malformed {}: {value}",
            value["schema"]
        );
    }
}

#[test]
fn current_warnings_and_workspace_removal_preserve_closed_scalar_boundaries() {
    let result = examples()["podway.procedure-validation-result/v2"].clone();
    let mut maximum = warning(&"A".repeat(64));
    maximum.insert("path".to_owned(), json!("界".repeat(256)));
    maximum.insert("message".to_owned(), json!("界".repeat(512)));
    let mut input = authoring_input("procedure.validate", &result);
    input.warnings = vec![maximum.clone(); 4];
    assert!(OutputEnvelopeV4::new(input).is_ok());
    for (field, value) in [
        ("code", json!("A".repeat(65))),
        ("code", json!("lowercase")),
        ("path", json!("界".repeat(257))),
        ("message", json!("界".repeat(513))),
        ("message", json!("")),
        ("unknown", json!(true)),
    ] {
        let mut warning = maximum.clone();
        warning.insert(field.to_owned(), value);
        let mut input = authoring_input("procedure.validate", &result);
        input.warnings = vec![warning];
        assert!(OutputEnvelopeV4::new(input).is_err());
    }
    let mut input = authoring_input("procedure.validate", &result);
    input.warnings = vec![maximum; 5];
    assert!(OutputEnvelopeV4::new(input).is_err());
    let present = current_detail_fixtures()["podway.workspace-removal-result/v2"].clone();
    let absent = json!({"schema":"podway.workspace-removal-result/v2","worktree_root":"/tmp/podway-example","workspace_uuid":null,"podway_directory_removed":false,"already_absent":true});
    let mut invalid_present = present.clone();
    invalid_present["workspace_uuid"] = Value::Null;
    for result in [present, invalid_present.clone(), absent] {
        assert!(OutputEnvelopeV4::new(authoring_input("workspace.remove", &result)).is_ok());
        for field in result.as_object().unwrap().keys() {
            let mut missing = result.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(OutputEnvelopeV4::new(authoring_input("workspace.remove", &missing)).is_err());
        }
        let mut open = result;
        open["registry_entry_removed"] = json!(true);
        assert!(OutputEnvelopeV4::new(authoring_input("workspace.remove", &open)).is_err());
    }
    invalid_present["podway_directory_removed"] = json!(false);
    assert!(OutputEnvelopeV4::new(authoring_input("workspace.remove", &invalid_present)).is_err());
}

#[test]
fn version_output_v4_preserves_its_exact_closed_identity_boundary() {
    let output = json!({
        "schema":"podway.output/v4", "request_id":UUID, "command":"version",
        "generated_at":"2026-08-04T00:00:00.000Z", "warnings":[],
        "result":{"schema":"podway.version-result/v2","product":"podway","version":"v0.2.12",
            "target":"aarch64-apple-darwin","build_identity":DIGEST,"source_commit":null,
            "contract_manifest_schema":"podway.contract-manifest/v2","contract_manifest_digest":DIGEST}
    });
    assert_valid("schemas/output-v4.schema.json", &output);
    let typed: podway_protocol::OutputEnvelopeV4 = serde_json::from_value(output.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), output);
    let mut invalid = Vec::new();
    for field in output.as_object().unwrap().keys() {
        let mut value = output.clone();
        value.as_object_mut().unwrap().remove(field);
        invalid.push(value);
    }
    for (field, value) in [
        ("result", json!({})),
        ("command", json!("daemon.status")),
        ("workspace", json!({"uuid":UUID})),
        ("job", json!({})),
        ("schema", json!("podway.output/v1")),
    ] {
        let mut candidate = output.clone();
        candidate[field] = value;
        invalid.push(candidate);
    }
    for field in output["result"].as_object().unwrap().keys() {
        let mut value = output.clone();
        value["result"].as_object_mut().unwrap().remove(field);
        invalid.push(value);
    }
    for (field, value) in [
        ("schema", json!("podway.version-result/v1")),
        (
            "contract_manifest_schema",
            json!("podway.contract-manifest/v1"),
        ),
        ("supported_ipc_ids", json!(["podway.ipc/v1"])),
        ("build_identity", json!("sha256:invalid")),
        ("unknown", json!(true)),
    ] {
        let mut candidate = output.clone();
        candidate["result"][field] = value;
        invalid.push(candidate);
    }
    for value in invalid {
        assert_invalid("schemas/output-v4.schema.json", &value);
        assert!(serde_json::from_value::<podway_protocol::OutputEnvelopeV4>(value).is_err());
    }
}

#[test]
fn current_direct_registry_preserves_the_accepted_known_answer_family_inventory() {
    // This explicit inventory comes from the adopted daemon-free known answers,
    // independently of the production registry and the current merged fixture.
    let expected = BTreeSet::from([
        "podway.attempt-conflict-details/v3",
        "podway.blocker-limit-details/v2",
        "podway.command-retired-details/v1",
        "podway.compact-status-result/v5",
        "podway.contract-manifest/v2",
        "podway.criterion-assessment-result/v2",
        "podway.criterion-citation-invalid-details/v1",
        "podway.decision-result/v2",
        "podway.delegation-accept-result/v2",
        "podway.delegation-error-details/v2",
        "podway.error/v2",
        "podway.goal-definition-result/v2",
        "podway.goal-revision-result/v2",
        "podway.idempotency-key-reused-details/v2",
        "podway.internal-error-details/v2",
        "podway.item-mutation-result/v3",
        "podway.item-record-many-result/v2",
        "podway.legacy-runtime-state-unsupported-details/v1",
        "podway.mutation-outcome-unknown-details/v3",
        "podway.next-result/v5",
        "podway.observation-result/v5",
        "podway.observation-result/v6",
        "podway.outcome-lookup-result/v1",
        "podway.output/v4",
        "podway.prepared-next-result/v3",
        "podway.procedure-digest-mismatch-details/v3",
        "podway.recoverable-v2-runtime-error-details/v2",
        "podway.recovery-recipe/v2",
        "podway.revision-conflict-details/v3",
        "podway.rework-result/v2",
        "podway.session-archive-limit-details/v2",
        "podway.session-archive-list-result/v2",
        "podway.session-archive-not-eligible-details/v2",
        "podway.session-archive-not-found-details/v2",
        "podway.session-archive-purge-result/v1",
        "podway.session-archive-result/v2",
        "podway.session-archive-show-result/v3",
        "podway.session-begin-result/v2",
        "podway.session-id-mismatch-details/v3",
        "podway.session-reset-not-eligible-details/v2",
        "podway.session-reset-result/v2",
        "podway.session-start-decision-required-details/v2",
        "podway.session-start-result/v4",
        "podway.session-start-state-conflict-details/v2",
        "podway.stage-transition-result/v3",
        "podway.status-result/v5",
        "podway.terminal-disposition-result/v2",
        "podway.v2-runtime-error-details/v2",
        "podway.version-result/v2",
        "podway.workspace-busy-details/v1",
        "podway.workspace-contract-mismatch-details/v1",
        "podway.workspace-contract-mismatch-details/v2",
        "podway.workspace-init-result/v2",
        "podway.workspace-maintenance-details/v1",
        "podway.workspace-maintenance-intent-marker/v1",
        "podway.workspace-recovery-details/v2",
        "podway.workspace-removal-marker/v2",
        "podway.workspace-removal-result/v2",
        "podway.workspace-reset-marker/v1",
        "podway.workspace-schema-unsupported-details/v1",
        "podway.workspace-uuid-mismatch-details/v3",
        "podway.writer-predecessor-declaration/v1",
    ]);
    let actual = DAEMON_FREE_DOCUMENT_SCHEMAS_V1
        .iter()
        .map(|entry| entry.schema)
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
}
