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
    CommandNameV1, EXISTING_ROUTE_RESULT_SCHEMAS_V2, ErrorEnvelopeV1, MAX_V2_OUTPUT_WARNINGS,
    MAX_V2_RUNTIME_ERROR_MESSAGE_CHARS_V1, MAX_V2_TERMINAL_ERROR_BYTES,
    NEW_ROUTE_RESULT_SCHEMAS_V1, OUTPUT_SCHEMA_V2, OutputEnvelopeInputV2, OutputEnvelopeV2,
    PROCEDURE_DIAGNOSTICS_RESULT_SCHEMA_V1, ProtocolError, RequestIdV1, ResponseEnvelopeV2,
    Rfc3339MillisV1, V2_RUNTIME_ERROR_CODES_V1, decode_response_payload_v2,
    decode_result_schema_contract_v2, decode_single_frame_v1, encode_frame_v1,
    encode_response_payload_v2, ensure_command_result_schema_v1, result_schema_top_level_fields_v2,
    validate_command_result_v1, validate_command_result_v2, validate_frame_payload_length,
    validate_v2_output_warnings,
};
use serde_json::{Map, Value, json};

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

fn admission() -> Value {
    json!({"admitted":true,"job_id":UUID,"workspace_sequence":1})
}

fn add_admitted_envelope_metadata(output: &mut Value) {
    output["workspace"] = json!({
        "uuid":UUID,"root":"/tmp/podway-v2ctr","latest_workspace_sequence":1
    });
    output["job"] = json!({
        "id":UUID,"sequence":1,"state":"succeeded",
        "submitted_at":"2026-08-04T00:00:00.000Z",
        "claimed_at":"2026-08-04T00:00:00.001Z",
        "finished_at":"2026-08-04T00:00:00.002Z"
    });
}

fn add_queued_job_envelope_metadata(output: &mut Value) {
    output["workspace"] = json!({
        "uuid":UUID,"root":"/tmp/podway-v2ctr","latest_workspace_sequence":1
    });
    output["job"] = json!({
        "id":UUID,"sequence":1,"state":"queued",
        "submitted_at":"2026-08-04T00:00:00.000Z",
        "finished_at":null
    });
}

fn admission_matches_job(output: &Value) -> bool {
    output["result"]["admission"]["admitted"] == json!(true)
        && output["result"]["admission"]["job_id"] == output["job"]["id"]
        && output["result"]["admission"]["workspace_sequence"] == output["job"]["sequence"]
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

fn examples() -> BTreeMap<&'static str, Value> {
    let node = json!({"node_definition_id":"work","graph_node_id":"work","node_type":"action"});
    let attempt = json!({"attempt_id":UUID,"attempt_number":1});
    let queue = json!({"pending_mutations":false,"queued_count":0,"running_job_id":null,"latest_workspace_sequence":1});
    let readiness =
        json!({"items_satisfied":true,"unblocked":true,"goal_ready":true,"can_advance":true});
    BTreeMap::from([
        (
            "podway.procedure-validation-result/v2",
            json!({"schema":"podway.procedure-validation-result/v2","file":"workflow.yaml","procedure_schema":"podway.procedure/v2","digest":DIGEST,"valid":true}),
        ),
        (
            "podway.detached-admission-result/v2",
            json!({"schema":"podway.detached-admission-result/v2","detached":true,"admission":admission()}),
        ),
        (
            "podway.session-start-result/v2",
            json!({"schema":"podway.session-start-result/v2","procedure_schema":"podway.procedure/v2","procedure_digest":DIGEST,"dry_run":false,"goal_tracking":false,"goal_defined":false,"admission":admission(),"session_id":UUID,"revision":1,"entry_graph_node_id":"work"}),
        ),
        (
            "podway.compact-status-result/v2",
            json!({"schema":"podway.compact-status-result/v2","procedure":{"schema":"podway.procedure/v2","id":"workflow","version":"1","digest":DIGEST},"session":{"id":UUID,"lifecycle":"running","revision":1},"current":{"node":node,"attempt":attempt,"readiness":readiness,"missing_required_item_count":0,"blockers_total":0},"goal_tracking":false,"goal_defined":false,"trace_length":1,"counters":[{"graph_node_id":"work","attempt_count":1,"rework_traversal_count":0}],"items":[],"queue":queue}),
        ),
        (
            "podway.status-result/v2",
            json!({"schema":"podway.status-result/v2","tier":"standard","procedure":{"schema":"podway.procedure/v2","id":"workflow","version":"1","digest":DIGEST},"session":{"id":UUID,"lifecycle":"running","revision":1},"current":{"node":node,"attempt":attempt,"readiness":readiness,"missing_required_item_count":0,"blockers_total":0},"purpose":"test","goal_tracking":false,"goal_defined":false,"trace_length":1,"counters":[{"graph_node_id":"work","attempt_count":1,"rework_traversal_count":0}],"items":[],"queue":queue,"missing_required_item_ids":[],"blocker_window":[],"blockers_truncated":false,"item_values":[],"items_total":0,"items_truncated":false,"references":[],"allowed_option_ids":[],"terminal":true,"allowed_manual_rework_targets":[]}),
        ),
        (
            "podway.next-result/v2",
            json!({"schema":"podway.next-result/v2","procedure_schema":"podway.procedure/v2","procedure_digest":DIGEST,"goal_tracking":false,"goal_defined":false,"node":node,"attempt":attempt,"trace_length":1,"counters":[{"graph_node_id":"work","attempt_count":1,"rework_traversal_count":0}],"queue":queue,"revision":1,"readiness":readiness,"title":"Work","intent":"Do work","instructions":[],"missing_required_item_count":0,"missing_required_items":[],"blockers_total":0,"blockers":[],"blockers_truncated":false,"terminal":true,"allowed_actions":["session.complete"],"suggestions":[{"command":"session.complete","argv":["podway","complete"]}],"references":[],"readback":[],"allowed_manual_rework_targets":[]}),
        ),
        (
            "podway.stage-transition-result/v2",
            json!({"schema":"podway.stage-transition-result/v2","admission":admission(),"transition":"complete","from_graph_node_id":"work","from_attempt_id":UUID,"revision":2,"session_state":"completed"}),
        ),
        (
            "podway.item-mutation-result/v2",
            json!({"schema":"podway.item-mutation-result/v2","admission":admission(),"changed":true,"graph_node_id":"work","attempt_id":UUID,"attempt_number":1,"item_id":"done","revision":2}),
        ),
        (
            "podway.job-lookup-result/v2",
            json!({"schema":"podway.job-lookup-result/v2","found":false}),
        ),
        (
            "podway.job-result/v2",
            json!({"schema":"podway.job-result/v2","job":null}),
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
        (
            "podway.decision-result/v1",
            json!({"schema":"podway.decision-result/v1","admission":admission(),"graph_node_id":"review","attempt_id":UUID,"attempt_number":1,"option_id":"accept","effect":"advance","target_graph_node_id":"finish","target_attempt_id":UUID,"revision":2,"session_state":"running","record":{"trace_sequence":1,"session_id":UUID,"session_revision":1,"procedure_schema":"podway.procedure/v2","procedure_snapshot_id":UUID,"procedure_digest":DIGEST,"graph_node_id":"review","node_definition_id":"review","attempt_id":UUID,"attempt_number":1,"goal_revision":null,"option_id":"accept","effect":"advance","target_graph_node_id":"finish","reason":"accepted","recorded_at":"2026-08-04T00:00:00.000Z","references":[]}}),
        ),
        (
            "podway.rework-result/v1",
            json!({"schema":"podway.rework-result/v1","admission":admission(),"from_graph_node_id":"finish","to_graph_node_id":"work","target_attempt_id":UUID,"reason":"retry","reactivated":true,"revision":2}),
        ),
        (
            "podway.goal-definition-result/v1",
            json!({"schema":"podway.goal-definition-result/v1","admission":admission(),"goal_revision":1,"statement":"Ship safely","criteria":[{"criterion_id":"tests","statement":"Tests pass"}],"actor":"master","recorded_at":"2026-08-04T00:00:00.000Z","revision":2}),
        ),
        (
            "podway.goal-revision-result/v1",
            json!({"schema":"podway.goal-revision-result/v1","admission":admission(),"goal_revision":2,"statement":"Ship safely","criteria":[{"criterion_id":"tests","statement":"Tests pass"}],"reason":"clarify","actor":"master","recorded_at":"2026-08-04T00:00:00.000Z","rework_to":"work","reactivated":false,"revision":3}),
        ),
        (
            "podway.criterion-assessment-result/v1",
            json!({"schema":"podway.criterion-assessment-result/v1","admission":admission(),"graph_node_id":"assess","attempt_id":UUID,"goal_revision":1,"mode":"assessment","result":{"criterion_id":"tests","status":"satisfied","reason":"verified","citations":[]},"complete":true,"determined_outcome":"achieved","revision":3}),
        ),
    ])
}

#[test]
fn v2run003_readback_schema_accepts_the_declared_list_maximum() {
    let values = (0..200)
        .map(|index| format!("value-{index}"))
        .collect::<Vec<_>>();
    let mut next = examples()["podway.next-result/v2"].clone();
    next["references"] = json!([{
        "source_graph_node_id": "source",
        "source_title": "Source",
        "source_attempt_id": UUID,
        "source_attempt_number": 1,
        "items_digest": DIGEST,
        "state": "resolved"
    }]);
    next["readback"] = json!([{
        "source_graph_node_id": "source",
        "source_title": "Source",
        "source_attempt_id": UUID,
        "source_attempt_number": 1,
        "items_digest": DIGEST,
        "state": "resolved",
        "items": [{"item_id": "values", "type": "list", "value": values}]
    }]);
    assert_valid("schemas/next-result-v2.schema.json", &next);
    assert!(decode_result_schema_contract_v2(next.as_object().unwrap()).is_some());

    next["readback"][0]["items"][0]["value"]
        .as_array_mut()
        .unwrap()
        .push(json!("overflow"));
    assert_invalid("schemas/next-result-v2.schema.json", &next);
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
        "schema":OUTPUT_SCHEMA_V2,"request_id":UUID,"command":"procedure.preview",
        "generated_at":"2026-08-04T00:00:00.000Z","result":diagnostics,"warnings":[]
    });
    assert_invalid("schemas/output-v2.schema.json", &output);
}

#[test]
fn v2ctr003_registry_is_versioned_and_covers_exactly_the_v2_authoring_routes() {
    assert_eq!(EXISTING_ROUTE_RESULT_SCHEMAS_V2.len(), 10);
    assert_eq!(NEW_ROUTE_RESULT_SCHEMAS_V1.len(), 9);
    assert!(
        EXISTING_ROUTE_RESULT_SCHEMAS_V2
            .iter()
            .all(|entry| entry.schema.ends_with("/v2"))
    );
    assert!(
        NEW_ROUTE_RESULT_SCHEMAS_V1
            .iter()
            .all(|entry| entry.schema.ends_with("/v1"))
    );

    let routes: BTreeSet<_> = NEW_ROUTE_RESULT_SCHEMAS_V1
        .iter()
        .flat_map(|entry| entry.commands.iter().copied())
        .collect();
    assert_eq!(
        routes,
        BTreeSet::from([
            "procedure.format",
            "procedure.validate",
            "procedure.vet",
            "procedure.lint",
            "procedure.check",
            "procedure.graph",
            "procedure.preview",
            "procedure.scaffold",
            "procedure.convert",
            "session.decide",
            "session.rework",
            "goal.define",
            "goal.revise",
            "goal.assess_criterion",
        ])
    );
    assert_eq!(routes.len(), 14);
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
fn v2ctr003_every_registered_result_has_a_closed_schema_and_known_answer() {
    let examples = examples();
    let registry: Vec<_> = EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
        .collect();
    assert_eq!(examples.len(), registry.len());
    for contract in registry {
        let example = examples
            .get(contract.schema)
            .unwrap_or_else(|| panic!("missing fixture for {}", contract.schema));
        assert_valid(contract.schema_path, example);
        let result: Map<String, Value> = example.as_object().unwrap().clone();
        assert_eq!(decode_result_schema_contract_v2(&result), Some(contract));

        let mut unknown = example.clone();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("unknown".to_owned(), json!(true));
        assert_eq!(
            decode_result_schema_contract_v2(unknown.as_object().unwrap()),
            None
        );
        let schema = read_schema(contract.schema_path);
        assert_eq!(schema["additionalProperties"], json!(false));
        let mut schema_without_id = schema.clone();
        schema_without_id.as_object_mut().unwrap().remove("$id");
        let filename = Path::new(contract.schema_path)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let validator = jsonschema::draft202012::options()
            .with_base_uri(format!("{SCHEMA_BASE}{filename}"))
            .with_retriever(local_schemas())
            .should_validate_formats(true)
            .build(&schema_without_id)
            .unwrap();
        assert!(
            !validator.is_valid(&unknown),
            "{} accepted an unknown field",
            contract.schema
        );
    }
    let expected_counters = json!([{
        "graph_node_id":"work", "attempt_count":1, "rework_traversal_count":0
    }]);
    for schema in [
        "podway.compact-status-result/v2",
        "podway.status-result/v2",
        "podway.next-result/v2",
    ] {
        assert_eq!(examples[schema]["counters"], expected_counters);
    }
}

#[test]
fn v2ctr003_output_v2_validates_every_registered_command_result_pair() {
    let examples = examples();
    for contract in EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
    {
        for command in contract.commands {
            let mut result = examples[contract.schema].clone();
            if contract.schema == "podway.detached-admission-result/v2"
                && matches!(*command, "session.start" | "session.start_replace")
            {
                result["procedure_digest"] = json!(DIGEST);
            }
            let mut output = json!({
                "schema": OUTPUT_SCHEMA_V2,
                "request_id": UUID,
                "command": command,
                "generated_at": "2026-08-04T00:00:00.000Z",
                "result": result,
                "warnings": []
            });
            if output["result"].get("admission").is_some() {
                add_admitted_envelope_metadata(&mut output);
            } else if contract.schema == "podway.job-result/v2" {
                add_queued_job_envelope_metadata(&mut output);
            }
            assert_valid("schemas/output-v2.schema.json", &output);
        }
    }
}

#[test]
fn v2plt006_production_codec_round_trips_every_registered_command_result_pair() {
    let examples = examples();
    for contract in EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
    {
        for command in contract.commands {
            let mut result = examples[contract.schema].clone();
            if contract.schema == "podway.detached-admission-result/v2"
                && matches!(*command, "session.start" | "session.start_replace")
            {
                result["procedure_digest"] = json!(DIGEST);
            }
            let mut expected = json!({
                "schema": OUTPUT_SCHEMA_V2,
                "request_id": UUID,
                "command": command,
                "generated_at": "2026-08-04T00:00:00.000Z",
                "result": result,
                "warnings": []
            });
            if expected["result"].get("admission").is_some() {
                add_admitted_envelope_metadata(&mut expected);
            } else if contract.schema == "podway.job-result/v2" {
                add_queued_job_envelope_metadata(&mut expected);
            }

            let decoded = decode_response_payload_v2(&serde_json::to_vec(&expected).unwrap())
                .unwrap_or_else(|error| {
                    panic!("{} for {command} did not decode: {error}", contract.schema)
                });
            assert!(matches!(decoded, ResponseEnvelopeV2::OutputV2(_)));
            let encoded = encode_response_payload_v2(&decoded).unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&encoded).unwrap(), expected);
        }
    }
}

#[test]
fn v2ctr003_v2_runtime_error_details_are_code_bound_and_closed() {
    let valid_details = json!({
        "schema": "podway.v2-runtime-error-details/v1",
        "kind": "GRAPH_NODE_NOT_FOUND",
        "graph_node_id": "review"
    });
    assert_valid(
        "schemas/v2-runtime-error-details-v1.schema.json",
        &valid_details,
    );
    let envelope = json!({
        "schema": "podway.error/v1",
        "request_id": UUID,
        "command": "session.status",
        "generated_at": "2026-08-04T00:00:00.000Z",
        "code": "GRAPH_NODE_NOT_FOUND",
        "message": "Graph node not found.",
        "retryable": false,
        "exit_code": 1,
        "details": valid_details
    });
    assert_valid("schemas/error-v1.schema.json", &envelope);
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(envelope.clone()).is_ok());

    let mut mismatched = envelope.clone();
    mismatched["details"]["kind"] = json!("NODE_DEFINITION_NOT_FOUND");
    assert_valid("schemas/error-v1.schema.json", &mismatched);
    assert_invalid(
        "schemas/v2-runtime-error-details-v1.schema.json",
        &mismatched["details"],
    );
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(mismatched).is_err());

    let mut open = envelope.clone();
    open["details"]["unknown"] = json!(true);
    assert_valid("schemas/error-v1.schema.json", &open);
    assert_invalid(
        "schemas/v2-runtime-error-details-v1.schema.json",
        &open["details"],
    );
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(open).is_err());

    let mut missing = envelope.clone();
    missing["details"]
        .as_object_mut()
        .unwrap()
        .remove("graph_node_id");
    assert_valid("schemas/error-v1.schema.json", &missing);
    assert_invalid(
        "schemas/v2-runtime-error-details-v1.schema.json",
        &missing["details"],
    );
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(missing).is_err());

    let mut maximum_message = envelope.clone();
    maximum_message["message"] = json!("x".repeat(MAX_V2_RUNTIME_ERROR_MESSAGE_CHARS_V1));
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(maximum_message.clone()).is_ok());
    maximum_message["message"] = json!("x".repeat(MAX_V2_RUNTIME_ERROR_MESSAGE_CHARS_V1 + 1));
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(maximum_message).is_err());

    let mut mutation = envelope;
    mutation["command"] = json!("session.complete");
    assert_valid("schemas/error-v1.schema.json", &mutation);
    assert_valid(
        "schemas/v2-runtime-error-details-v1.schema.json",
        &mutation["details"],
    );
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(mutation.clone()).is_err());
    mutation["details"]["admission"] = json!({"admitted":false});
    assert_valid("schemas/error-v1.schema.json", &mutation);
    assert_valid(
        "schemas/v2-runtime-error-details-v1.schema.json",
        &mutation["details"],
    );
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(mutation).is_ok());
}

#[test]
fn v2ctr004_v2_runtime_error_catalog_is_schema_and_decoder_bound() {
    let catalog: Value = serde_json::from_slice(
        &fs::read(root().join("assets/specifications/error-codes.json")).unwrap(),
    )
    .unwrap();
    let catalog_codes = catalog["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["details_schema"] == json!("podway.v2-runtime-error-details/v1"))
        .map(|entry| entry["code"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let decoder_codes = V2_RUNTIME_ERROR_CODES_V1
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let details_schema = read_schema("schemas/v2-runtime-error-details-v1.schema.json");
    let schema_codes = details_schema["$defs"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(|definition| definition["allOf"].as_array())
        .filter_map(|all_of| all_of.get(1))
        .filter_map(|variant| variant["properties"]["kind"]["const"].as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(catalog_codes, decoder_codes);
    assert_eq!(schema_codes, decoder_codes);
    assert_eq!(decoder_codes.len(), 26);
}

#[test]
fn v2ctr003_job_reconciliation_is_non_recursive_and_state_consistent() {
    let mut terminal_success = json!({
        "schema": OUTPUT_SCHEMA_V2,
        "request_id": UUID,
        "command": "session.complete",
        "generated_at": "2026-08-04T00:00:00.000Z",
        "result": examples()["podway.stage-transition-result/v2"].clone(),
        "warnings": []
    });
    add_admitted_envelope_metadata(&mut terminal_success);
    let succeeded = json!({
        "schema":"podway.job-lookup-result/v2",
        "found":true,
        "job":{
            "id":UUID,"sequence":1,"state":"succeeded",
            "submitted_at":"2026-08-04T00:00:00.000Z",
            "claimed_at":"2026-08-04T00:00:00.001Z",
            "finished_at":"2026-08-04T00:00:00.002Z",
            "command":"session.complete","request_digest":DIGEST,
            "terminal_response":terminal_success
        }
    });
    assert_valid("schemas/job-lookup-result-v2.schema.json", &succeeded);

    let terminal_error = json!({
        "schema":"podway.error/v1","request_id":UUID,"command":"session.complete",
        "generated_at":"2026-08-04T00:00:00.000Z","code":"GRAPH_NODE_NOT_FOUND",
        "message":"Graph node not found.","retryable":false,"exit_code":1,
        "workspace":{
            "uuid":UUID,"root":"/tmp/podway-v2ctr","latest_workspace_sequence":1
        },
        "details":{
            "schema":"podway.v2-runtime-error-details/v1","kind":"GRAPH_NODE_NOT_FOUND",
            "graph_node_id":"work","admission":{"admitted":true,"job_id":UUID,"workspace_sequence":1}
        }
    });
    let mut failed = succeeded.clone();
    failed["job"]["state"] = json!("failed");
    assert_invalid("schemas/job-lookup-result-v2.schema.json", &failed);
    failed["job"]["terminal_response"] = terminal_error.clone();
    assert_valid("schemas/job-lookup-result-v2.schema.json", &failed);
    assert!(decode_result_schema_contract_v2(failed.as_object().unwrap()).is_some());

    let mut query_error = failed.clone();
    query_error["job"]["terminal_response"]["command"] = json!("job.status");
    assert_invalid("schemas/job-lookup-result-v2.schema.json", &query_error);
    assert!(decode_result_schema_contract_v2(query_error.as_object().unwrap()).is_none());

    let mut mismatched_mutation_error = failed.clone();
    mismatched_mutation_error["job"]["terminal_response"]["command"] = json!("item.set");
    assert_valid(
        "schemas/job-lookup-result-v2.schema.json",
        &mismatched_mutation_error,
    );
    assert!(
        decode_result_schema_contract_v2(mismatched_mutation_error.as_object().unwrap()).is_none()
    );

    let mut succeeded_with_error = succeeded.clone();
    succeeded_with_error["job"]["terminal_response"] = terminal_error;
    assert_invalid(
        "schemas/job-lookup-result-v2.schema.json",
        &succeeded_with_error,
    );

    let mut running_with_response = succeeded.clone();
    running_with_response["job"]["state"] = json!("running");
    running_with_response["job"]["finished_at"] = Value::Null;
    assert_invalid(
        "schemas/job-lookup-result-v2.schema.json",
        &running_with_response,
    );

    let mut cancelled_with_success = succeeded.clone();
    cancelled_with_success["job"]["state"] = json!("cancelled");
    assert_invalid(
        "schemas/job-lookup-result-v2.schema.json",
        &cancelled_with_success,
    );

    let mut unsupported_command = succeeded.clone();
    unsupported_command["job"]["command"] = json!("session.status");
    assert_invalid(
        "schemas/job-lookup-result-v2.schema.json",
        &unsupported_command,
    );

    let nested_query = json!({
        "schema":OUTPUT_SCHEMA_V2,"request_id":UUID,"command":"job.status",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "result":{"schema":"podway.job-result/v2","job":null},"warnings":[]
    });
    assert_invalid(
        "schemas/job-result-v2.schema.json",
        &json!({
            "schema":"podway.job-result/v2","job":nested_query
        }),
    );

    let nested_authoring = json!({
        "schema":OUTPUT_SCHEMA_V2,"request_id":UUID,"command":"procedure.format",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "result":examples()["podway.procedure-source-result/v1"].clone(),"warnings":[]
    });
    assert_invalid(
        "schemas/job-result-v2.schema.json",
        &json!({
            "schema":"podway.job-result/v2","job":nested_authoring
        }),
    );

    let detached = json!({
        "schema":OUTPUT_SCHEMA_V2,"request_id":UUID,"command":"session.complete",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "result":examples()["podway.detached-admission-result/v2"].clone(),"warnings":[]
    });
    assert_invalid(
        "schemas/job-result-v2.schema.json",
        &json!({
            "schema":"podway.job-result/v2","job":detached
        }),
    );
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
    assert_eq!(schema_pairs.len(), 53);

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
fn v2ctr003_decoder_rejects_missing_non_string_and_unregistered_discriminators() {
    assert_eq!(decode_result_schema_contract_v2(&Map::new()), None);
    assert_eq!(
        decode_result_schema_contract_v2(&Map::from_iter([("schema".to_owned(), json!(1))])),
        None
    );
    assert_eq!(
        decode_result_schema_contract_v2(&Map::from_iter([(
            "schema".to_owned(),
            json!("podway.status-result/v3")
        )])),
        None
    );
    assert_eq!(
        EXISTING_ROUTE_RESULT_SCHEMAS_V2
            .iter()
            .filter(|entry| entry.schema.starts_with("podway.job-"))
            .count(),
        2
    );
    let mut incomplete = examples()["podway.next-result/v2"].clone();
    incomplete.as_object_mut().unwrap().remove("node");
    assert_eq!(
        decode_result_schema_contract_v2(incomplete.as_object().unwrap()),
        None
    );
}

#[test]
fn v2plt006_production_decoder_rejects_deep_schema_violations_in_every_family() {
    let fixtures = examples();
    let mut invalid = Vec::new();

    let mut value = fixtures["podway.procedure-validation-result/v2"].clone();
    value["valid"] = json!("true");
    invalid.push(value);
    for (schema, pointer) in [
        ("podway.detached-admission-result/v2", "/admission"),
        ("podway.session-start-result/v2", "/admission"),
        ("podway.compact-status-result/v2", "/procedure"),
        ("podway.status-result/v2", "/current"),
        ("podway.next-result/v2", "/node"),
        ("podway.stage-transition-result/v2", "/admission"),
        ("podway.item-mutation-result/v2", "/admission"),
        ("podway.procedure-preview-result/v1", "/checks"),
        ("podway.decision-result/v1", "/record"),
        ("podway.goal-definition-result/v1", "/criteria/0"),
        ("podway.goal-revision-result/v1", "/criteria/0"),
        ("podway.criterion-assessment-result/v1", "/result"),
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
    let mut value = fixtures["podway.job-lookup-result/v2"].clone();
    value["found"] = json!(true);
    invalid.push(value);
    let mut value = fixtures["podway.job-result/v2"].clone();
    value["job"] = json!({"unknown":true});
    invalid.push(value);
    let mut value = fixtures["podway.procedure-source-result/v1"].clone();
    value["operation"] = json!("unknown");
    invalid.push(value);
    let mut value = fixtures["podway.procedure-diagnostics-result/v1"].clone();
    value["diagnostics"] = json!([{"unknown":true}]);
    value["diagnostics_total"] = json!(1);
    invalid.push(value);
    let mut value = fixtures["podway.procedure-graph-result/v1"].clone();
    value["format"] = json!("unknown");
    invalid.push(value);
    let mut value = fixtures["podway.rework-result/v1"].clone();
    value["reason"] = json!("x".repeat(2001));
    invalid.push(value);

    assert_eq!(invalid.len(), 19);
    for value in invalid {
        assert_eq!(
            decode_result_schema_contract_v2(value.as_object().unwrap()),
            None,
            "production decoder accepted a deep schema violation in {}",
            value["schema"]
        );
    }
}

#[test]
fn v2plt006_production_output_decoder_validates_nested_terminal_receipts() {
    let mut terminal = json!({
        "schema":OUTPUT_SCHEMA_V2,
        "request_id":UUID,
        "command":"session.complete",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "result":examples()["podway.stage-transition-result/v2"].clone(),
        "warnings":[]
    });
    add_admitted_envelope_metadata(&mut terminal);
    let lookup = json!({
        "schema":OUTPUT_SCHEMA_V2,
        "request_id":UUID,
        "command":"job.lookup",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "result":{
            "schema":"podway.job-lookup-result/v2",
            "found":true,
            "job":{
                "id":UUID,
                "sequence":1,
                "state":"succeeded",
                "submitted_at":"2026-08-04T00:00:00.000Z",
                "claimed_at":"2026-08-04T00:00:00.001Z",
                "finished_at":"2026-08-04T00:00:00.002Z",
                "command":"session.complete",
                "request_digest":DIGEST,
                "terminal_response":terminal
            }
        },
        "warnings":[]
    });
    assert!(serde_json::from_value::<OutputEnvelopeV2>(lookup.clone()).is_ok());

    let mut deep_open = lookup.clone();
    deep_open["result"]["job"]["terminal_response"]["result"]["admission"]["unknown"] = json!(true);
    assert!(serde_json::from_value::<OutputEnvelopeV2>(deep_open).is_err());

    let mut mismatched_command = lookup.clone();
    mismatched_command["result"]["job"]["terminal_response"]["command"] = json!("item.set");
    assert!(serde_json::from_value::<OutputEnvelopeV2>(mismatched_command).is_err());

    let mut mismatched_identity = lookup.clone();
    mismatched_identity["result"]["job"]["id"] = json!("00000000-0000-4000-8000-000000000002");
    assert!(serde_json::from_value::<OutputEnvelopeV2>(mismatched_identity).is_err());

    let mut mismatched_job_projection = lookup.clone();
    mismatched_job_projection["result"]["job"]["terminal_response"]["job"]["state"] =
        json!("failed");
    assert!(serde_json::from_value::<OutputEnvelopeV2>(mismatched_job_projection).is_err());

    let mut mismatched_timestamp = lookup.clone();
    mismatched_timestamp["result"]["job"]["terminal_response"]["job"]["submitted_at"] =
        json!("2026-08-03T00:00:00.000Z");
    assert!(serde_json::from_value::<OutputEnvelopeV2>(mismatched_timestamp).is_err());

    let mut decision_terminal = json!({
        "schema":OUTPUT_SCHEMA_V2,
        "request_id":UUID,
        "command":"session.decide",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "result":examples()["podway.decision-result/v1"].clone(),
        "warnings":[]
    });
    add_admitted_envelope_metadata(&mut decision_terminal);
    decision_terminal["result"]["option_id"] = json!("other");
    let mut mismatched_decision = lookup.clone();
    mismatched_decision["result"]["job"]["command"] = json!("session.decide");
    mismatched_decision["result"]["job"]["terminal_response"] = decision_terminal;
    assert!(serde_json::from_value::<OutputEnvelopeV2>(mismatched_decision).is_err());

    let mismatched_error = json!({
        "schema":"podway.error/v1",
        "request_id":UUID,
        "command":"session.complete",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "code":"GRAPH_NODE_NOT_FOUND",
        "message":"Graph node not found.",
        "retryable":false,
        "exit_code":1,
        "workspace":{
            "uuid":UUID,
            "root":"/tmp/podway-v2ctr",
            "latest_workspace_sequence":1
        },
        "details":{
            "schema":"podway.v2-runtime-error-details/v1",
            "kind":"OPTION_NOT_FOUND",
            "graph_node_id":"work",
            "option_id":"accept",
            "admission":admission()
        }
    });
    let mut mismatched_error_lookup = lookup.clone();
    mismatched_error_lookup["result"]["job"]["state"] = json!("failed");
    mismatched_error_lookup["result"]["job"]["terminal_response"] = mismatched_error;
    assert!(serde_json::from_value::<OutputEnvelopeV2>(mismatched_error_lookup).is_err());

    let mut job_readback = json!({
        "schema":OUTPUT_SCHEMA_V2,
        "request_id":UUID,
        "command":"job.status",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "workspace":terminal["workspace"].clone(),
        "job":terminal["job"].clone(),
        "result":{"schema":"podway.job-result/v2","job":terminal.clone()},
        "warnings":[]
    });
    assert!(serde_json::from_value::<OutputEnvelopeV2>(job_readback.clone()).is_ok());
    job_readback.as_object_mut().unwrap().remove("job");
    assert!(serde_json::from_value::<OutputEnvelopeV2>(job_readback).is_err());

    let terminal_error_without_admission = json!({
        "schema":"podway.error/v1",
        "request_id":UUID,
        "command":"session.complete",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "code":"REQUEST_INVALID",
        "message":"Request is invalid.",
        "retryable":false,
        "exit_code":2,
        "details":{}
    });
    let mut failed_job = terminal["job"].clone();
    failed_job["state"] = json!("failed");
    let missing_admission = json!({
        "schema":OUTPUT_SCHEMA_V2,
        "request_id":UUID,
        "command":"job.wait",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "workspace":terminal["workspace"].clone(),
        "job":failed_job,
        "result":{"schema":"podway.job-result/v2","job":terminal_error_without_admission},
        "warnings":[]
    });
    assert!(serde_json::from_value::<OutputEnvelopeV2>(missing_admission).is_err());

    let mut recursive_lookup = lookup;
    let nested = recursive_lookup.clone();
    recursive_lookup["result"]["job"]["terminal_response"] = nested;
    assert!(serde_json::from_value::<OutputEnvelopeV2>(recursive_lookup).is_err());
}

fn maximum_warnings() -> Vec<Value> {
    (0..MAX_V2_OUTPUT_WARNINGS)
        .map(|_| {
            json!({
                "code": "C".repeat(64),
                "path": "\0".repeat(256),
                "message": "\0".repeat(512)
            })
        })
        .collect()
}

fn maximum_identifier(index: usize) -> String {
    let suffix = format!("-{index}");
    format!("a{}{}", "a".repeat(63 - suffix.len()), suffix)
}

fn escape_heavy(characters: usize) -> String {
    "\0".repeat(characters)
}

fn maximum_admission() -> Value {
    json!({"admitted":true,"job_id":UUID,"workspace_sequence":u64::MAX})
}

fn maximum_criteria() -> Value {
    Value::Array(
        (0..16)
            .map(|index| {
                json!({
                    "criterion_id":maximum_identifier(index),
                    "statement":escape_heavy(300)
                })
            })
            .collect(),
    )
}

fn maximum_citations() -> Value {
    Value::Array(
        (0..4)
            .map(|index| json!({"reference_graph_node_id":maximum_identifier(index)}))
            .collect(),
    )
}

fn maximum_criterion_results() -> Value {
    Value::Array(
        (0..16)
            .map(|index| {
                json!({
                    "criterion_id":maximum_identifier(index),
                    "status":"unsatisfied",
                    "reason":escape_heavy(2_000),
                    "citations":maximum_citations()
                })
            })
            .collect(),
    )
}

fn maximum_reference_snapshots() -> Value {
    Value::Array(
        (0..8)
            .map(|index| {
                json!({
                    "source_graph_node_id":maximum_identifier(index),
                    "source_attempt_id":"ffffffff-ffff-4fff-8fff-ffffffffffff",
                    "source_attempt_number":u64::MAX,
                    "items_digest":DIGEST,
                    "state":"resolved"
                })
            })
            .collect(),
    )
}

fn maximum_terminal_success_candidate(schema: &str) -> (&'static str, Value) {
    let identifier = maximum_identifier(63);
    let uuid = "ffffffff-ffff-4fff-8fff-ffffffffffff";
    let timestamp = "9999-12-31T23:59:59.999Z";
    let result = match schema {
        "podway.session-start-result/v2" => json!({
            "schema":schema,
            "procedure_schema":"podway.procedure/v2",
            "procedure_digest":DIGEST,
            "dry_run":false,
            "goal_tracking":false,
            "goal_defined":false,
            "admission":maximum_admission(),
            "session_id":uuid,
            "revision":u64::MAX,
            "entry_graph_node_id":identifier
        }),
        "podway.stage-transition-result/v2" => json!({
            "schema":schema,
            "admission":maximum_admission(),
            "transition":"retry",
            "from_graph_node_id":identifier,
            "from_attempt_id":uuid,
            "to_graph_node_id":maximum_identifier(62),
            "to_attempt_id":UUID,
            "reason":escape_heavy(2_000),
            "revision":u64::MAX,
            "session_state":"running"
        }),
        "podway.item-mutation-result/v2" => json!({
            "schema":schema,
            "admission":maximum_admission(),
            "changed":false,
            "graph_node_id":identifier,
            "attempt_id":uuid,
            "attempt_number":u64::MAX,
            "item_id":maximum_identifier(62),
            "revision":u64::MAX,
            "value_digest":DIGEST
        }),
        "podway.decision-result/v1" => {
            let record = json!({
                "trace_sequence":u64::MAX,
                "session_id":uuid,
                "session_revision":u64::MAX,
                "procedure_schema":"podway.procedure/v2",
                "procedure_snapshot_id":UUID,
                "procedure_digest":DIGEST,
                "graph_node_id":identifier,
                "node_definition_id":maximum_identifier(61),
                "attempt_id":uuid,
                "attempt_number":u64::MAX,
                "goal_revision":u64::MAX,
                "option_id":maximum_identifier(60),
                "effect":"advance",
                "target_graph_node_id":maximum_identifier(59),
                "reason":escape_heavy(2_000),
                "actor":escape_heavy(256),
                "recorded_at":timestamp,
                "references":maximum_reference_snapshots(),
                "assessment":"session_goal",
                "assessment_mode":"assessment",
                "goal_outcome":"not_achieved",
                "criterion_results":maximum_criterion_results()
            });
            json!({
                "schema":schema,
                "admission":maximum_admission(),
                "graph_node_id":record["graph_node_id"].clone(),
                "attempt_id":record["attempt_id"].clone(),
                "attempt_number":record["attempt_number"].clone(),
                "option_id":record["option_id"].clone(),
                "effect":record["effect"].clone(),
                "target_graph_node_id":record["target_graph_node_id"].clone(),
                "target_attempt_id":UUID,
                "revision":u64::MAX,
                "session_state":"running",
                "record":record
            })
        }
        "podway.rework-result/v1" => json!({
            "schema":schema,
            "admission":maximum_admission(),
            "from_graph_node_id":identifier,
            "to_graph_node_id":maximum_identifier(62),
            "target_attempt_id":uuid,
            "reason":escape_heavy(2_000),
            "reactivated":false,
            "revision":u64::MAX
        }),
        "podway.goal-definition-result/v1" => json!({
            "schema":schema,
            "admission":maximum_admission(),
            "goal_revision":1,
            "statement":escape_heavy(1_000),
            "criteria":maximum_criteria(),
            "actor":escape_heavy(256),
            "recorded_at":timestamp,
            "revision":u64::MAX
        }),
        "podway.goal-revision-result/v1" => json!({
            "schema":schema,
            "admission":maximum_admission(),
            "goal_revision":u64::MAX,
            "statement":escape_heavy(1_000),
            "criteria":maximum_criteria(),
            "reason":escape_heavy(1_000),
            "actor":escape_heavy(256),
            "recorded_at":timestamp,
            "rework_to":identifier,
            "reactivated":false,
            "revision":u64::MAX
        }),
        "podway.criterion-assessment-result/v1" => json!({
            "schema":schema,
            "admission":maximum_admission(),
            "graph_node_id":identifier,
            "attempt_id":uuid,
            "goal_revision":u64::MAX,
            "mode":"assessment",
            "result":{
                "criterion_id":maximum_identifier(62),
                "status":"unsatisfied",
                "reason":escape_heavy(2_000),
                "citations":maximum_citations()
            },
            "complete":true,
            "determined_outcome":"not_achieved",
            "revision":u64::MAX
        }),
        unexpected => panic!("unhandled terminal success schema {unexpected}"),
    };
    let command = match schema {
        "podway.session-start-result/v2" => "session.start",
        "podway.stage-transition-result/v2" => "session.retry",
        "podway.item-mutation-result/v2" => "item.attach",
        "podway.decision-result/v1" => "session.decide",
        "podway.rework-result/v1" => "session.rework",
        "podway.goal-definition-result/v1" => "goal.define",
        "podway.goal-revision-result/v1" => "goal.revise",
        "podway.criterion-assessment-result/v1" => "goal.assess_criterion",
        _ => unreachable!(),
    };
    (command, result)
}

fn maximum_runtime_error_details(code: &str) -> Value {
    let identifier = maximum_identifier(0);
    let identifiers_8 = (0..8).map(maximum_identifier).collect::<Vec<_>>();
    let identifiers_16 = (0..16).map(maximum_identifier).collect::<Vec<_>>();
    let attempt = "ffffffff-ffff-4fff-8fff-ffffffffffff";
    let mut details = json!({
        "schema":"podway.v2-runtime-error-details/v1",
        "kind":code,
        "admission":{"admitted":true,"job_id":UUID,"workspace_sequence":u64::MAX}
    });
    match code {
        "PROCEDURE_V2_SCHEMA_INVALID" => {
            details["diagnostic_codes"] = Value::Array(
                (0..256)
                    .map(|index| json!(format!("E{index:03}_{}", "A".repeat(59))))
                    .collect(),
            );
        }
        "GRAPH_NODE_NOT_FOUND" | "DECISION_REASON_MISSING" => {
            details["graph_node_id"] = json!(identifier);
        }
        "NODE_DEFINITION_NOT_FOUND" => details["node_definition_id"] = json!(identifier),
        "GRAPH_NODE_TYPE_MISMATCH" => {
            details["graph_node_id"] = json!(identifier);
            details["expected_node_type"] = json!("decision");
            details["actual_node_type"] = json!("action");
        }
        "OPTION_NOT_ALLOWED" => {
            details["graph_node_id"] = json!(identifier);
            details["option_id"] = json!(maximum_identifier(9));
            details["allowed_option_ids"] = json!(identifiers_8);
        }
        "ROUTE_NOT_ALLOWED" => {
            details["graph_node_id"] = json!(identifier);
            details["option_id"] = json!(maximum_identifier(9));
        }
        "EVIDENCE_REFERENCE_UNRESOLVED" => {
            details["graph_node_id"] = json!(identifier);
            details["source_graph_node_ids"] = json!(identifiers_8);
        }
        "EVIDENCE_REFERENCE_STALE" => {
            details["graph_node_id"] = json!(identifier);
            details["source_graph_node_id"] = json!(maximum_identifier(9));
            details["expected_source_attempt_id"] = json!(attempt);
            details["current_source_attempt_id"] = json!(UUID);
        }
        "MANUAL_REWORK_TARGET_NOT_ALLOWED"
        | "MANUAL_REWORK_TARGET_NOT_ON_TRACE"
        | "GOAL_REVISION_TARGET_NOT_ALLOWED"
        | "GOAL_REVISION_TARGET_NOT_REVISION_SAFE" => {
            details["target_graph_node_id"] = json!(identifier);
        }
        "SESSION_GOAL_ALREADY_DEFINED" | "FRESH_GOAL_ASSESSMENT_MISSING" => {
            details["goal_revision"] = json!(u64::MAX);
        }
        "GOAL_REVISION_STALE" => {
            details["expected_goal_revision"] = json!(u64::MAX);
            details["actual_goal_revision"] = json!(u64::MAX - 1);
        }
        "CRITERION_MODE_MIXED" => {
            details["criterion_id"] = json!(identifier);
            details["expected_mode"] = json!("applicability");
            details["actual_status"] = json!("not_applicable");
        }
        "CRITERION_CITATION_INVALID" => {
            details["criterion_id"] = json!(identifier);
            details["citation"] = json!({"reference_graph_node_id":maximum_identifier(9)});
        }
        "CRITERION_RESULT_MISSING" => {
            details["missing_criterion_ids"] = json!(identifiers_16);
        }
        "CRITERION_NOT_FOUND" => details["criterion_id"] = json!(identifier),
        "GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED" => {
            details["option_id"] = json!(identifier);
            details["determined_outcome"] = json!("not_achieved");
            details["allowed_option_ids"] = json!(identifiers_8);
        }
        "DIGEST_CONFIRMATION_REQUIRED" => details["procedure_digest"] = json!(DIGEST),
        "UNSUPPORTED_V2_CAPABILITY" => {
            details["capability"] = json!(escape_heavy(128));
            details["required_result_schema"] = json!(escape_heavy(128));
            details["contract_manifest_digest"] = json!(DIGEST);
        }
        "GOAL_TRACKING_NOT_ENABLED" | "SESSION_GOAL_MISSING" | "REACTIVATION_FLAG_REQUIRED" => {}
        unexpected => panic!("unhandled v2 runtime error code {unexpected}"),
    }
    details
}

fn shared_terminal_error_details(code: &str) -> Value {
    match code {
        "SESSION_REVISION_CONFLICT" | "ITEM_REVISION_CONFLICT" => json!({
            "schema":"podway.revision-conflict-details/v1",
            "expected_revision":u64::MAX,
            "current_revision":u64::MAX - 1
        }),
        "SESSION_ID_MISMATCH" => json!({
            "schema":"podway.session-id-mismatch-details/v1",
            "expected_session_id":UUID,
            "actual_session_id":null,
            "admission":{"admitted":false}
        }),
        "ATTEMPT_NOT_CURRENT" => json!({
            "schema":"podway.attempt-conflict-details/v1",
            "expected_attempt_id":UUID
        }),
        "BLOCKER_LIMIT_REACHED" => json!({
            "schema":"podway.blocker-limit-details/v1",
            "maximum_open_blockers":1_024
        }),
        _ => json!({}),
    }
}

#[test]
fn v2rel002_largest_terminal_success_receipt_round_trips_once_in_job_reads() {
    let candidates = [
        "podway.session-start-result/v2",
        "podway.stage-transition-result/v2",
        "podway.item-mutation-result/v2",
        "podway.decision-result/v1",
        "podway.rework-result/v1",
        "podway.goal-definition-result/v1",
        "podway.goal-revision-result/v1",
        "podway.criterion-assessment-result/v1",
    ];
    let mut direct = Vec::new();
    for schema in candidates {
        let (command, result) = maximum_terminal_success_candidate(schema);
        let terminal = json!({
            "schema": OUTPUT_SCHEMA_V2,
            "request_id": "ffffffff-ffff-4fff-8fff-ffffffffffff",
            "command": command,
            "generated_at": "9999-12-31T23:59:59.999Z",
            "workspace": {
                "uuid":"ffffffff-ffff-4fff-8fff-ffffffffffff",
                "root":escape_heavy(4_096),
                "latest_workspace_sequence":u64::MAX
            },
            "job": {
                "id":UUID,
                "sequence":u64::MAX,
                "state":"succeeded",
                "submitted_at":"9999-12-31T23:59:59.997Z",
                "claimed_at":"9999-12-31T23:59:59.998Z",
                "finished_at":"9999-12-31T23:59:59.999Z"
            },
            "session": {
                "id":"ffffffff-ffff-4fff-8fff-ffffffffffff",
                "title":escape_heavy(500),
                "lifecycle":"completed",
                "revision_before":u64::MAX,
                "revision_after":u64::MAX
            },
            "result": result,
            "warnings": maximum_warnings()
        });
        assert_valid("schemas/output-v2.schema.json", &terminal);
        let decoded = decode_response_payload_v2(&serde_json::to_vec(&terminal).unwrap())
            .unwrap_or_else(|error| panic!("maximum {schema} terminal did not decode: {error}"));
        let encoded = encode_response_payload_v2(&decoded).unwrap();
        let frame = encode_frame_v1(&encoded).unwrap();
        assert_eq!(decode_single_frame_v1(&frame).unwrap(), encoded);
        assert_eq!(frame.len(), encoded.len() + 4);
        direct.push((encoded.len(), terminal));
    }

    let largest_direct_length = direct.iter().map(|(length, _)| *length).max().unwrap();
    let (_, largest) = direct
        .into_iter()
        .max_by_key(|(length, _)| *length)
        .unwrap();
    assert_eq!(largest["result"]["schema"], "podway.decision-result/v1");
    assert_eq!(
        serde_json::to_vec(&largest).unwrap().len(),
        largest_direct_length
    );

    for command in ["job.status", "job.wait"] {
        let receipt = json!({
            "schema": OUTPUT_SCHEMA_V2,
            "request_id": "ffffffff-ffff-4fff-8fff-ffffffffffff",
            "command": command,
            "generated_at": "9999-12-31T23:59:59.999Z",
            "workspace": largest["workspace"].clone(),
            "job": largest["job"].clone(),
            "session":largest["session"].clone(),
            "result": {"schema":"podway.job-result/v2", "job":largest.clone()},
            "warnings": maximum_warnings()
        });
        assert_valid("schemas/output-v2.schema.json", &receipt);
        let decoded = decode_response_payload_v2(&serde_json::to_vec(&receipt).unwrap()).unwrap();
        let payload = encode_response_payload_v2(&decoded).unwrap();
        assert!(payload.len() <= 1_048_576);
        let frame = encode_frame_v1(&payload).unwrap();
        assert_eq!(frame.len(), payload.len() + 4);
        assert!(frame.len() <= 1_048_580);
        assert_eq!(decode_single_frame_v1(&frame).unwrap(), payload);
        assert_eq!(decode_response_payload_v2(&payload).unwrap(), decoded);
    }
}

#[test]
fn v2rel002_largest_terminal_error_receipt_round_trips_once_in_job_reads() {
    let mut direct = Vec::new();
    for code in V2_RUNTIME_ERROR_CODES_V1 {
        let (exit_code, retryable) = match *code {
            "EVIDENCE_REFERENCE_STALE" | "GOAL_REVISION_STALE" => (4, true),
            "DIGEST_CONFIRMATION_REQUIRED" => (2, false),
            "UNSUPPORTED_V2_CAPABILITY" => (3, false),
            _ => (1, false),
        };
        let details = maximum_runtime_error_details(code);
        assert_valid("schemas/v2-runtime-error-details-v1.schema.json", &details);
        let error = json!({
            "schema":"podway.error/v1",
            "request_id":"ffffffff-ffff-4fff-8fff-ffffffffffff",
            "command":"goal.assess_criterion",
            "generated_at":"9999-12-31T23:59:59.999Z",
            "code":code,
            "message":escape_heavy(MAX_V2_RUNTIME_ERROR_MESSAGE_CHARS_V1),
            "retryable":retryable,
            "exit_code":exit_code,
            "workspace":{
                "uuid":"ffffffff-ffff-4fff-8fff-ffffffffffff",
                "root":escape_heavy(4_096),
                "latest_workspace_sequence":u64::MAX
            },
            "details":details
        });
        assert_valid("schemas/error-v1.schema.json", &error);
        let nested = json!({"schema":"podway.job-result/v2","job":error.clone()});
        assert_valid("schemas/job-result-v2.schema.json", &nested);
        assert!(decode_result_schema_contract_v2(nested.as_object().unwrap()).is_some());
        let decoded = decode_response_payload_v2(&serde_json::to_vec(&error).unwrap())
            .unwrap_or_else(|failure| panic!("maximum {code} error did not decode: {failure}"));
        assert!(matches!(decoded, ResponseEnvelopeV2::Error(_)));
        let payload = encode_response_payload_v2(&decoded).unwrap();
        assert_eq!(payload.len(), serde_json::to_vec(&error).unwrap().len());
        let frame = encode_frame_v1(&payload).unwrap();
        assert_eq!(decode_single_frame_v1(&frame).unwrap(), payload);
        direct.push((payload.len(), error));
    }
    assert_eq!(direct.len(), 26);
    let largest_direct_length = direct.iter().map(|(length, _)| *length).max().unwrap();
    let (_, largest) = direct
        .into_iter()
        .max_by_key(|(length, _)| *length)
        .unwrap();
    assert_eq!(largest["code"], "PROCEDURE_V2_SCHEMA_INVALID");
    assert_eq!(
        serde_json::to_vec(&largest).unwrap().len(),
        largest_direct_length
    );

    for command in ["job.status", "job.wait"] {
        let receipt = json!({
            "schema":OUTPUT_SCHEMA_V2,
            "request_id":"ffffffff-ffff-4fff-8fff-ffffffffffff",
            "command":command,
            "generated_at":"9999-12-31T23:59:59.999Z",
            "workspace":largest["workspace"].clone(),
            "job":{
                "id":UUID,"sequence":u64::MAX,"state":"failed",
                "submitted_at":"9999-12-31T23:59:59.997Z",
                "claimed_at":"9999-12-31T23:59:59.998Z",
                "finished_at":"9999-12-31T23:59:59.999Z"
            },
            "result":{"schema":"podway.job-result/v2","job":largest.clone()},
            "warnings":maximum_warnings()
        });
        assert_valid("schemas/output-v2.schema.json", &receipt);
        let decoded = decode_response_payload_v2(&serde_json::to_vec(&receipt).unwrap()).unwrap();
        let payload = encode_response_payload_v2(&decoded).unwrap();
        assert!(payload.len() <= 1_048_576);
        let frame = encode_frame_v1(&payload).unwrap();
        assert_eq!(frame.len(), payload.len() + 4);
        assert!(frame.len() <= 1_048_580);
        assert_eq!(decode_single_frame_v1(&frame).unwrap(), payload);
        assert_eq!(decode_response_payload_v2(&payload).unwrap(), decoded);

        let mut recursive = receipt;
        recursive["result"]["job"] = recursive.clone();
        assert_invalid("schemas/output-v2.schema.json", &recursive);
        assert!(decode_response_payload_v2(&serde_json::to_vec(&recursive).unwrap()).is_err());
    }
}

#[test]
fn v2rel002_terminal_error_is_contextually_closed_and_bounded() {
    const SHARED_PERSISTED_TERMINAL_CODES: &[&str] = &[
        "REQUEST_INVALID",
        "INTERNAL_ERROR",
        "SESSION_NOT_RUNNING",
        "SESSION_CANCELLED",
        "SESSION_REVISION_CONFLICT",
        "SESSION_ID_MISMATCH",
        "ATTEMPT_NOT_CURRENT",
        "STAGE_NOT_SKIPPABLE",
        "RETURN_NOT_ALLOWED",
        "REOPEN_NOT_ALLOWED",
        "REQUIRED_ITEMS_MISSING",
        "BLOCKERS_PRESENT",
        "BLOCKER_LIMIT_REACHED",
        "ITEM_NOT_FOUND",
        "ITEM_REVISION_CONFLICT",
        "ITEM_TYPE_MISMATCH",
        "ITEM_CONSTRAINT_FAILED",
        "LIST_VALUE_NOT_FOUND",
        "LIST_VALUE_DUPLICATE",
        "ARTIFACT_CHANGED",
        "BLOCKER_NOT_FOUND",
        "BLOCKER_NOT_CURRENT",
    ];
    let catalog = podway_protocol::error_code_catalog_v1()
        .map(|(code, exit_code, retryable)| (code, (exit_code, retryable)))
        .collect::<HashMap<_, _>>();
    for code in SHARED_PERSISTED_TERMINAL_CODES {
        let (exit_code, retryable) = catalog[code];
        let error = json!({
            "schema":"podway.error/v1",
            "request_id":UUID,
            "command":"session.complete",
            "generated_at":"2026-08-04T00:00:00.000Z",
            "code":code,
            "message":"Bound persisted terminal failure.",
            "retryable":retryable,
            "exit_code":exit_code,
            "workspace":{
                "uuid":UUID,
                "root":"/tmp/podway-v2rel002",
                "latest_workspace_sequence":1
            },
            "details":shared_terminal_error_details(code)
        });
        assert!(
            serde_json::from_value::<ErrorEnvelopeV1>(error.clone()).is_ok(),
            "shared persisted terminal code {code} must remain a valid direct error"
        );
        let nested = json!({"schema":"podway.job-result/v2","job":error});
        assert_valid("schemas/job-result-v2.schema.json", &nested);
        assert!(
            decode_result_schema_contract_v2(nested.as_object().unwrap()).is_some(),
            "shared persisted terminal code {code} must remain valid when bounded"
        );
    }

    let terminal = json!({
        "schema":"podway.error/v1",
        "request_id":UUID,
        "command":"session.complete",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "code":"GRAPH_NODE_NOT_FOUND",
        "message":"Graph node not found.",
        "retryable":false,
        "exit_code":1,
        "workspace":{
            "uuid":UUID,
            "root":"/tmp/podway-v2rel002",
            "latest_workspace_sequence":1
        },
        "details":{
            "schema":"podway.v2-runtime-error-details/v1",
            "kind":"GRAPH_NODE_NOT_FOUND",
            "graph_node_id":"work",
            "admission":admission()
        }
    });
    let normal_result = json!({"schema":"podway.job-result/v2","job":terminal.clone()});
    assert_valid("schemas/job-result-v2.schema.json", &normal_result);
    assert!(decode_result_schema_contract_v2(normal_result.as_object().unwrap()).is_some());

    let normal_read = json!({
        "schema":OUTPUT_SCHEMA_V2,
        "request_id":UUID,
        "command":"job.status",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "workspace":terminal["workspace"].clone(),
        "job":{
            "id":UUID,"sequence":1,"state":"failed",
            "submitted_at":"2026-08-04T00:00:00.000Z",
            "claimed_at":"2026-08-04T00:00:00.001Z",
            "finished_at":"2026-08-04T00:00:00.002Z"
        },
        "result":normal_result.clone(),
        "warnings":[]
    });
    let normal = serde_json::from_value::<OutputEnvelopeV2>(normal_read).unwrap();
    let normal = ResponseEnvelopeV2::OutputV2(normal);
    let encoded = encode_response_payload_v2(&normal).unwrap();
    assert_eq!(decode_response_payload_v2(&encoded).unwrap(), normal);

    let mut oversized_root = terminal.clone();
    oversized_root["workspace"]["root"] = json!("r".repeat(4097));
    let oversized_root = json!({"schema":"podway.job-result/v2","job":oversized_root});
    assert_invalid("schemas/job-result-v2.schema.json", &oversized_root);
    assert!(decode_result_schema_contract_v2(oversized_root.as_object().unwrap()).is_none());

    let mut generic = json!({
        "schema":"podway.error/v1",
        "request_id":UUID,
        "command":"session.complete",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "code":"INTERNAL_ERROR",
        "message":"Internal error.",
        "retryable":false,
        "exit_code":6,
        "workspace":{
            "uuid":UUID,
            "root":escape_heavy(4_096),
            "latest_workspace_sequence":u64::MAX
        },
        "details":{
            "context":"",
            "admission":{"admitted":true,"job_id":UUID,"workspace_sequence":u64::MAX}
        }
    });
    let baseline = serde_json::to_vec(&generic).unwrap().len();
    generic["details"]["context"] = json!("x".repeat(MAX_V2_TERMINAL_ERROR_BYTES - baseline));
    assert_eq!(
        serde_json::to_vec(&generic).unwrap().len(),
        MAX_V2_TERMINAL_ERROR_BYTES
    );
    let maximum_result = json!({"schema":"podway.job-result/v2","job":generic.clone()});
    assert_valid("schemas/job-result-v2.schema.json", &maximum_result);
    assert!(decode_result_schema_contract_v2(maximum_result.as_object().unwrap()).is_some());
    assert_eq!(
        generic["workspace"]["root"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        4_096
    );
    for command in ["job.status", "job.wait"] {
        let maximum_read = json!({
            "schema":OUTPUT_SCHEMA_V2,
            "request_id":UUID,
            "command":command,
            "generated_at":"9999-12-31T23:59:59.999Z",
            "workspace":generic["workspace"].clone(),
            "job":{
                "id":UUID,"sequence":u64::MAX,"state":"failed",
                "submitted_at":"9999-12-31T23:59:59.997Z",
                "claimed_at":"9999-12-31T23:59:59.998Z",
                "finished_at":"9999-12-31T23:59:59.999Z"
            },
            "result":maximum_result.clone(),
            "warnings":maximum_warnings()
        });
        let decoded =
            decode_response_payload_v2(&serde_json::to_vec(&maximum_read).unwrap()).unwrap();
        assert!(matches!(
            &decoded,
            ResponseEnvelopeV2::OutputV2(output) if output.session().is_none()
        ));
        let encoded = encode_response_payload_v2(&decoded).unwrap();
        assert!(encoded.len() <= 1_048_576);
        assert_eq!(
            decode_single_frame_v1(&encode_frame_v1(&encoded).unwrap()).unwrap(),
            encoded
        );
    }

    generic["details"]["context"] = json!("x".repeat(MAX_V2_TERMINAL_ERROR_BYTES + 1 - baseline));
    let direct = serde_json::to_vec(&generic).unwrap();
    assert_eq!(direct.len(), MAX_V2_TERMINAL_ERROR_BYTES + 1);
    assert_valid("schemas/error-v1.schema.json", &generic);
    assert!(matches!(
        decode_response_payload_v2(&direct).unwrap(),
        ResponseEnvelopeV2::Error(_)
    ));

    let oversized_result = json!({"schema":"podway.job-result/v2","job":generic});
    assert_valid("schemas/job-result-v2.schema.json", &oversized_result);
    assert!(decode_result_schema_contract_v2(oversized_result.as_object().unwrap()).is_none());
    assert!(serde_json::to_vec(&oversized_result).unwrap().len() > MAX_V2_TERMINAL_ERROR_BYTES);

    let mut oversized_message = normal_result;
    oversized_message["job"]["code"] = json!("INTERNAL_ERROR");
    oversized_message["job"]["retryable"] = json!(false);
    oversized_message["job"]["exit_code"] = json!(6);
    oversized_message["job"]["message"] = json!("m".repeat(513));
    oversized_message["job"]["details"] = json!({});
    assert!(serde_json::from_value::<ErrorEnvelopeV1>(oversized_message["job"].clone()).is_ok());
    assert_invalid("schemas/job-result-v2.schema.json", &oversized_message);
    assert!(decode_result_schema_contract_v2(oversized_message.as_object().unwrap()).is_none());
}

#[test]
fn v2ctr003_decision_result_projections_match_the_immutable_record() {
    let decision = examples()["podway.decision-result/v1"].clone();
    assert_valid("schemas/decision-result-v1.schema.json", &decision);
    assert!(decode_result_schema_contract_v2(decision.as_object().unwrap()).is_some());

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
        assert_valid("schemas/decision-result-v1.schema.json", &result);
        assert_eq!(
            decode_result_schema_contract_v2(result.as_object().unwrap()),
            None,
            "decoder accepted mismatched decision projection {field}"
        );
    }
}

#[test]
fn v2ctr003_critical_bounds_and_variant_rules_fail_closed() {
    let fixtures = examples();

    let mut decision_next = fixtures["podway.next-result/v2"].clone();
    decision_next["node"]["node_type"] = json!("decision");
    decision_next.as_object_mut().unwrap().remove("intent");
    decision_next
        .as_object_mut()
        .unwrap()
        .remove("instructions");
    decision_next.as_object_mut().unwrap().remove("terminal");
    decision_next["objective"] = json!("Choose");
    decision_next["prompt"] = json!("Continue?");
    decision_next["options"] = json!([{"option_id":"accept","label":"Accept"}]);
    decision_next["reason_policy"] = json!({"required":true});
    assert_valid("schemas/next-result-v2.schema.json", &decision_next);

    let mut decision_status = fixtures["podway.status-result/v2"].clone();
    decision_status["current"]["node"]["node_type"] = json!("decision");
    decision_status.as_object_mut().unwrap().remove("terminal");
    decision_status["allowed_option_ids"] = json!(["accept"]);
    assert_valid("schemas/status-result-v2.schema.json", &decision_status);

    let mut completed_status = fixtures["podway.status-result/v2"].clone();
    completed_status["session"]["lifecycle"] = json!("completed");
    completed_status["current"] = Value::Null;
    completed_status.as_object_mut().unwrap().remove("terminal");
    assert_valid("schemas/status-result-v2.schema.json", &completed_status);

    let mut running_without_current = completed_status;
    running_without_current["session"]["lifecycle"] = json!("running");
    assert_invalid(
        "schemas/status-result-v2.schema.json",
        &running_without_current,
    );

    let mut dry_run = fixtures["podway.session-start-result/v2"].clone();
    dry_run["dry_run"] = json!(true);
    dry_run.as_object_mut().unwrap().remove("session_id");
    dry_run.as_object_mut().unwrap().remove("revision");
    dry_run.as_object_mut().unwrap().remove("admission");
    dry_run
        .as_object_mut()
        .unwrap()
        .remove("entry_graph_node_id");
    assert_valid("schemas/session-start-result-v2.schema.json", &dry_run);

    let mut dry_run_with_admission = dry_run.clone();
    dry_run_with_admission["admission"] = admission();
    assert_invalid(
        "schemas/session-start-result-v2.schema.json",
        &dry_run_with_admission,
    );

    let mut tracked_without_goal = fixtures["podway.session-start-result/v2"].clone();
    tracked_without_goal["goal_tracking"] = json!(true);
    assert_valid(
        "schemas/session-start-result-v2.schema.json",
        &tracked_without_goal,
    );

    let mut tracked_with_goal = tracked_without_goal.clone();
    tracked_with_goal["goal_defined"] = json!(true);
    assert_valid(
        "schemas/session-start-result-v2.schema.json",
        &tracked_with_goal,
    );

    let mut untracked_with_goal = fixtures["podway.session-start-result/v2"].clone();
    untracked_with_goal["goal_defined"] = json!(true);
    assert_invalid(
        "schemas/session-start-result-v2.schema.json",
        &untracked_with_goal,
    );

    let mut legacy_goal_required = fixtures["podway.session-start-result/v2"].clone();
    legacy_goal_required["goal_required"] = json!(false);
    assert_invalid(
        "schemas/session-start-result-v2.schema.json",
        &legacy_goal_required,
    );

    let mut terminal_compact = fixtures["podway.compact-status-result/v2"].clone();
    terminal_compact["session"]["lifecycle"] = json!("completed");
    terminal_compact["current"] = Value::Null;
    assert_valid(
        "schemas/compact-status-result-v2.schema.json",
        &terminal_compact,
    );

    let mut compact_with_root_blockers = fixtures["podway.compact-status-result/v2"].clone();
    compact_with_root_blockers["blockers"] = json!([]);
    assert_invalid(
        "schemas/compact-status-result-v2.schema.json",
        &compact_with_root_blockers,
    );

    let mut status_with_root_blockers = fixtures["podway.status-result/v2"].clone();
    status_with_root_blockers["blockers"] = json!([]);
    assert_invalid(
        "schemas/status-result-v2.schema.json",
        &status_with_root_blockers,
    );

    let mut next = fixtures["podway.next-result/v2"].clone();
    next["title"] = json!("x".repeat(121));
    assert_invalid("schemas/next-result-v2.schema.json", &next);

    let mut empty_intent = fixtures["podway.next-result/v2"].clone();
    empty_intent["intent"] = json!("");
    assert_invalid("schemas/next-result-v2.schema.json", &empty_intent);

    let mut unknown_action = fixtures["podway.next-result/v2"].clone();
    unknown_action["allowed_actions"] = json!(["session.explode"]);
    assert_invalid("schemas/next-result-v2.schema.json", &unknown_action);

    let mut unknown_suggestion = fixtures["podway.next-result/v2"].clone();
    unknown_suggestion["suggestions"] = json!([{"command":"session.explode","argv":["explode"]}]);
    assert_invalid("schemas/next-result-v2.schema.json", &unknown_suggestion);

    let mut ambiguous_action = fixtures["podway.next-result/v2"].clone();
    ambiguous_action["next_graph_node_id"] = json!("finish");
    assert_invalid("schemas/next-result-v2.schema.json", &ambiguous_action);

    let mut stale_readback = fixtures["podway.next-result/v2"].clone();
    stale_readback["references"] = json!([{
        "source_graph_node_id":"build", "source_title":"Build", "source_attempt_id":UUID,
        "source_attempt_number":1, "items_digest":DIGEST, "state":"stale"
    }]);
    assert_invalid("schemas/next-result-v2.schema.json", &stale_readback);

    let mut stale_readback_values = fixtures["podway.next-result/v2"].clone();
    stale_readback_values["readback"] = json!([{
        "source_graph_node_id":"build", "source_title":"Build", "source_attempt_id":UUID,
        "source_attempt_number":1, "items_digest":DIGEST, "state":"stale", "items":[]
    }]);
    assert_invalid("schemas/next-result-v2.schema.json", &stale_readback_values);

    let mut mixed_source = fixtures["podway.procedure-source-result/v1"].clone();
    mixed_source["template"] = json!("minimal");
    assert_invalid(
        "schemas/procedure-source-result-v1.schema.json",
        &mixed_source,
    );

    let mut incomplete_record = fixtures["podway.decision-result/v1"].clone();
    incomplete_record["record"]
        .as_object_mut()
        .unwrap()
        .remove("procedure_snapshot_id");
    assert_invalid("schemas/decision-result-v1.schema.json", &incomplete_record);

    let mut missing_target_attempt = fixtures["podway.decision-result/v1"].clone();
    missing_target_attempt
        .as_object_mut()
        .unwrap()
        .remove("target_attempt_id");
    assert_invalid(
        "schemas/decision-result-v1.schema.json",
        &missing_target_attempt,
    );

    let mut completed_decision = fixtures["podway.decision-result/v1"].clone();
    completed_decision["session_state"] = json!("completed");
    assert_invalid(
        "schemas/decision-result-v1.schema.json",
        &completed_decision,
    );

    let mut stale_snapshot = fixtures["podway.decision-result/v1"].clone();
    stale_snapshot["record"]["references"] = json!([{
        "source_graph_node_id":"build", "source_attempt_id":UUID,
        "source_attempt_number":1, "items_digest":DIGEST, "state":"stale"
    }]);
    assert_invalid("schemas/decision-result-v1.schema.json", &stale_snapshot);

    let mut partial_assessment = fixtures["podway.decision-result/v1"].clone();
    partial_assessment["record"]["goal_outcome"] = json!("achieved");
    assert_invalid(
        "schemas/decision-result-v1.schema.json",
        &partial_assessment,
    );

    let mut complete_assessment = fixtures["podway.decision-result/v1"].clone();
    complete_assessment["record"]["goal_revision"] = json!(1);
    complete_assessment["record"]["assessment"] = json!("session_goal");
    complete_assessment["record"]["assessment_mode"] = json!("assessment");
    complete_assessment["record"]["goal_outcome"] = json!("achieved");
    complete_assessment["record"]["criterion_results"] = json!([{
        "criterion_id":"tests", "status":"satisfied", "reason":"verified", "citations":[]
    }]);
    assert_valid(
        "schemas/decision-result-v1.schema.json",
        &complete_assessment,
    );

    let mut achieved_with_unsatisfied = complete_assessment.clone();
    achieved_with_unsatisfied["record"]["criterion_results"][0]["status"] = json!("unsatisfied");
    assert_invalid(
        "schemas/decision-result-v1.schema.json",
        &achieved_with_unsatisfied,
    );

    let mut not_achieved_without_unsatisfied = complete_assessment.clone();
    not_achieved_without_unsatisfied["record"]["goal_outcome"] = json!("not_achieved");
    assert_invalid(
        "schemas/decision-result-v1.schema.json",
        &not_achieved_without_unsatisfied,
    );

    let mut superseded_with_citation = complete_assessment.clone();
    superseded_with_citation["record"]["assessment_mode"] = json!("applicability");
    superseded_with_citation["record"]["goal_outcome"] = json!("superseded");
    superseded_with_citation["record"]["criterion_results"][0]["status"] = json!("not_applicable");
    superseded_with_citation["record"]["criterion_results"][0]["citations"] =
        json!([{"local_item_id":"reviewed"}]);
    assert_invalid(
        "schemas/decision-result-v1.schema.json",
        &superseded_with_citation,
    );

    for citation in [
        json!({"reference_graph_node_id":"build"}),
        json!({"local_item_id":"reviewed"}),
    ] {
        let mut cited = complete_assessment.clone();
        cited["record"]["criterion_results"][0]["citations"] = json!([citation]);
        assert_valid("schemas/decision-result-v1.schema.json", &cited);
    }
    for citation in [
        json!({"item_id":"reviewed"}),
        json!({"reference_graph_node_id":"build","local_item_id":"reviewed"}),
    ] {
        let mut invalid_citation = complete_assessment.clone();
        invalid_citation["record"]["criterion_results"][0]["citations"] = json!([citation]);
        assert_invalid("schemas/decision-result-v1.schema.json", &invalid_citation);
    }

    let public_u64_max = json!(u64::MAX);
    let mut max_criterion_revision = fixtures["podway.criterion-assessment-result/v1"].clone();
    max_criterion_revision["goal_revision"] = public_u64_max.clone();
    assert_valid(
        "schemas/criterion-assessment-result-v1.schema.json",
        &max_criterion_revision,
    );
    let mut max_decision_attempt = fixtures["podway.decision-result/v1"].clone();
    max_decision_attempt["attempt_number"] = public_u64_max.clone();
    assert_valid(
        "schemas/decision-result-v1.schema.json",
        &max_decision_attempt,
    );
    let mut max_goal_revision = fixtures["podway.goal-revision-result/v1"].clone();
    max_goal_revision["goal_revision"] = public_u64_max.clone();
    assert_valid(
        "schemas/goal-revision-result-v1.schema.json",
        &max_goal_revision,
    );
    let mut max_item_attempt = fixtures["podway.item-mutation-result/v2"].clone();
    max_item_attempt["attempt_number"] = public_u64_max.clone();
    assert_valid(
        "schemas/item-mutation-result-v2.schema.json",
        &max_item_attempt,
    );
    let mut max_next_goal_revision = fixtures["podway.next-result/v2"].clone();
    max_next_goal_revision["goal_tracking"] = json!(true);
    max_next_goal_revision["goal_defined"] = json!(true);
    max_next_goal_revision["goal_revision"] = public_u64_max.clone();
    max_next_goal_revision["goal"] = json!({
        "revision": u64::MAX,
        "statement": "Ship safely",
        "criteria": [{"criterion_id":"tests","statement":"Tests pass","status":"unassessed"}]
    });
    assert_valid(
        "schemas/next-result-v2.schema.json",
        &max_next_goal_revision,
    );

    let mut invalid_block = fixtures["podway.stage-transition-result/v2"].clone();
    invalid_block["transition"] = json!("block");
    invalid_block["session_state"] = json!("running");
    assert_invalid(
        "schemas/stage-transition-result-v2.schema.json",
        &invalid_block,
    );

    let running_skip = json!({
        "schema":"podway.stage-transition-result/v2", "admission":admission(), "transition":"skip",
        "from_graph_node_id":"work", "from_attempt_id":UUID,
        "to_graph_node_id":"finish", "to_attempt_id":UUID,
        "revision":2, "session_state":"running"
    });
    assert_valid(
        "schemas/stage-transition-result-v2.schema.json",
        &running_skip,
    );
    let completed_skip = json!({
        "schema":"podway.stage-transition-result/v2", "admission":admission(), "transition":"skip",
        "from_graph_node_id":"work", "from_attempt_id":UUID,
        "revision":2, "session_state":"completed"
    });
    assert_valid(
        "schemas/stage-transition-result-v2.schema.json",
        &completed_skip,
    );
    let mut reasoned_skip = running_skip;
    reasoned_skip["reason"] = json!("optional explanation");
    assert_valid(
        "schemas/stage-transition-result-v2.schema.json",
        &reasoned_skip,
    );
    let retry_without_reason = json!({
        "schema":"podway.stage-transition-result/v2", "admission":admission(), "transition":"retry",
        "from_graph_node_id":"work", "from_attempt_id":UUID,
        "to_graph_node_id":"work", "to_attempt_id":UUID,
        "revision":2, "session_state":"running"
    });
    assert_invalid(
        "schemas/stage-transition-result-v2.schema.json",
        &retry_without_reason,
    );

    for (schema, schema_path) in [
        (
            "podway.session-start-result/v2",
            "schemas/session-start-result-v2.schema.json",
        ),
        (
            "podway.next-result/v2",
            "schemas/next-result-v2.schema.json",
        ),
        (
            "podway.stage-transition-result/v2",
            "schemas/stage-transition-result-v2.schema.json",
        ),
        (
            "podway.item-mutation-result/v2",
            "schemas/item-mutation-result-v2.schema.json",
        ),
        (
            "podway.decision-result/v1",
            "schemas/decision-result-v1.schema.json",
        ),
        (
            "podway.rework-result/v1",
            "schemas/rework-result-v1.schema.json",
        ),
        (
            "podway.goal-definition-result/v1",
            "schemas/goal-definition-result-v1.schema.json",
        ),
        (
            "podway.goal-revision-result/v1",
            "schemas/goal-revision-result-v1.schema.json",
        ),
        (
            "podway.criterion-assessment-result/v1",
            "schemas/criterion-assessment-result-v1.schema.json",
        ),
    ] {
        let mut zero_revision = fixtures[schema].clone();
        zero_revision["revision"] = json!(0);
        assert_invalid(schema_path, &zero_revision);
    }
    for (schema, schema_path) in [
        (
            "podway.compact-status-result/v2",
            "schemas/compact-status-result-v2.schema.json",
        ),
        (
            "podway.status-result/v2",
            "schemas/status-result-v2.schema.json",
        ),
    ] {
        let mut zero_revision = fixtures[schema].clone();
        zero_revision["session"]["revision"] = json!(0);
        assert_invalid(schema_path, &zero_revision);
    }
    let mut zero_record_revision = fixtures["podway.decision-result/v1"].clone();
    zero_record_revision["record"]["session_revision"] = json!(0);
    assert_invalid(
        "schemas/decision-result-v1.schema.json",
        &zero_record_revision,
    );
    let mut zero_item_revision = fixtures["podway.compact-status-result/v2"].clone();
    zero_item_revision["items"] = json!([{
        "item_id":"done", "type":"confirm", "required":true,
        "satisfied":false, "revision":0
    }]);
    assert_valid(
        "schemas/compact-status-result-v2.schema.json",
        &zero_item_revision,
    );

    let mut standard_history = fixtures["podway.status-result/v2"].clone();
    standard_history["current_trace_history"] = json!({"entries":[],"trace_truncated":false,"trace_window":{"first_sequence":1,"last_sequence":1}});
    assert_invalid("schemas/status-result-v2.schema.json", &standard_history);

    let mut false_skip = fixtures["podway.status-result/v2"].clone();
    false_skip["skip"] = json!({"allowed":false,"reason_required":true});
    assert_invalid("schemas/status-result-v2.schema.json", &false_skip);

    let mut bad_blocker = fixtures["podway.status-result/v2"].clone();
    bad_blocker["blocker_window"] = json!([{
        "blocker_id":"not-a-uuid", "reason":"blocked", "created_at":"2026-08-04T00:00:00.000Z"
    }]);
    assert_invalid("schemas/status-result-v2.schema.json", &bad_blocker);

    let mut mismatched_readback = fixtures["podway.next-result/v2"].clone();
    mismatched_readback["readback"] = json!([{
        "source_graph_node_id":"build","source_title":"Build","source_attempt_id":UUID,
        "source_attempt_number":1,"items_digest":DIGEST,"state":"resolved",
        "items":[{"item_id":"done","type":"confirm","value":"yes"}]
    }]);
    assert_invalid("schemas/next-result-v2.schema.json", &mismatched_readback);

    let mut wrong_mode = fixtures["podway.criterion-assessment-result/v1"].clone();
    wrong_mode["mode"] = json!("applicability");
    assert_invalid(
        "schemas/criterion-assessment-result-v1.schema.json",
        &wrong_mode,
    );

    let mut incomplete = fixtures["podway.criterion-assessment-result/v1"].clone();
    incomplete["complete"] = json!(false);
    assert_invalid(
        "schemas/criterion-assessment-result-v1.schema.json",
        &incomplete,
    );

    let mut applicability_achieved = fixtures["podway.criterion-assessment-result/v1"].clone();
    applicability_achieved["mode"] = json!("applicability");
    applicability_achieved["result"]["status"] = json!("not_applicable");
    applicability_achieved["determined_outcome"] = json!("achieved");
    assert_invalid(
        "schemas/criterion-assessment-result-v1.schema.json",
        &applicability_achieved,
    );

    let mut assessment_superseded = fixtures["podway.criterion-assessment-result/v1"].clone();
    assessment_superseded["determined_outcome"] = json!("superseded");
    assert_invalid(
        "schemas/criterion-assessment-result-v1.schema.json",
        &assessment_superseded,
    );

    let mut unsatisfied_achieved = fixtures["podway.criterion-assessment-result/v1"].clone();
    unsatisfied_achieved["result"]["status"] = json!("unsatisfied");
    unsatisfied_achieved["determined_outcome"] = json!("achieved");
    assert_invalid(
        "schemas/criterion-assessment-result-v1.schema.json",
        &unsatisfied_achieved,
    );

    let mut satisfied_not_achieved = fixtures["podway.criterion-assessment-result/v1"].clone();
    satisfied_not_achieved["determined_outcome"] = json!("not_achieved");
    assert_valid(
        "schemas/criterion-assessment-result-v1.schema.json",
        &satisfied_not_achieved,
    );

    let reset = json!({"schema":"podway.stage-transition-result/v2","admission":admission(),"transition":"reset","reset":true,"revision":3});
    assert_valid("schemas/stage-transition-result-v2.schema.json", &reset);
    let unblock_all = json!({"schema":"podway.stage-transition-result/v2","admission":admission(),"transition":"unblock","from_graph_node_id":"work","from_attempt_id":UUID,"all":true,"revision":3,"session_state":"running"});
    assert_valid(
        "schemas/stage-transition-result-v2.schema.json",
        &unblock_all,
    );
    let invalid_cancel = json!({"schema":"podway.stage-transition-result/v2","admission":admission(),"transition":"cancel","from_graph_node_id":"work","from_attempt_id":UUID,"revision":3,"session_state":"running"});
    assert_invalid(
        "schemas/stage-transition-result-v2.schema.json",
        &invalid_cancel,
    );

    let mut local_citation = fixtures["podway.criterion-assessment-result/v1"].clone();
    local_citation["result"]["citations"] = json!([{"local_item_id":"reviewed"}]);
    assert_valid(
        "schemas/criterion-assessment-result-v1.schema.json",
        &local_citation,
    );

    let mut verbose = fixtures["podway.status-result/v2"].clone();
    verbose["tier"] = json!("verbose");
    for field in [
        "current_trace_history",
        "stale_attempt_history",
        "decision_history",
        "rework_history",
        "stale_goal_revision_history",
        "stale_goal_assessment_history",
    ] {
        verbose[field] = json!({"entries":[],"trace_truncated":false,"trace_window":null});
    }
    assert_valid("schemas/status-result-v2.schema.json", &verbose);

    let mut active_stale_reference = fixtures["podway.status-result/v2"].clone();
    active_stale_reference["references"] = json!([{
        "source_graph_node_id":"build", "source_title":"Build", "source_attempt_id":UUID,
        "source_attempt_number":1, "items_digest":DIGEST, "state":"stale"
    }]);
    assert_invalid(
        "schemas/status-result-v2.schema.json",
        &active_stale_reference,
    );

    let stale_attempt = json!({
        "trace_sequence":1, "graph_node_id":"work", "node_definition_id":"work",
        "attempt_id":UUID, "attempt_number":1, "goal_revision":null,
        "lifecycle":"abandoned", "validity":"stale",
        "started_at":"2026-08-04T00:00:00.000Z",
        "finished_at":"2026-08-04T00:01:00.000Z", "terminal_reason":"reworked",
        "items":[], "items_total":0, "items_truncated":false,
        "references":[
            {"source_graph_node_id":"build", "source_title":"Build", "source_attempt_id":UUID,
             "source_attempt_number":1, "items_digest":DIGEST, "state":"stale"},
            {"source_graph_node_id":"optional", "state":"unresolved"}
        ]
    });
    let mut historical_stale_references = verbose.clone();
    historical_stale_references["stale_attempt_history"] = json!({
        "entries":[stale_attempt.clone()], "trace_truncated":false,
        "trace_window":{"first_sequence":1,"last_sequence":1}
    });
    assert_valid(
        "schemas/status-result-v2.schema.json",
        &historical_stale_references,
    );

    let mut historical_resolved_reference = historical_stale_references.clone();
    historical_resolved_reference["stale_attempt_history"]["entries"][0]["references"][0]["state"] =
        json!("resolved");
    assert_invalid(
        "schemas/status-result-v2.schema.json",
        &historical_resolved_reference,
    );

    let mut historical_missing_references = historical_stale_references.clone();
    historical_missing_references["stale_attempt_history"]["entries"][0]
        .as_object_mut()
        .unwrap()
        .remove("references");
    assert_invalid(
        "schemas/status-result-v2.schema.json",
        &historical_missing_references,
    );

    let mut legacy_history_markers = verbose.clone();
    legacy_history_markers["current_trace_history"] =
        json!({"entries":[],"truncated":false,"window":null});
    assert_invalid(
        "schemas/status-result-v2.schema.json",
        &legacy_history_markers,
    );

    let mut max_trace_window = verbose.clone();
    max_trace_window["current_trace_history"] = json!({
        "entries":[{
            "trace_sequence":u64::MAX,"graph_node_id":"work","node_definition_id":"work",
            "attempt_id":UUID,"attempt_number":1,"goal_revision":null,"lifecycle":"active",
            "validity":"valid","started_at":"2026-08-04T00:00:00.000Z"
        }],
        "trace_truncated":false,
        "trace_window":{"first_sequence":u64::MAX,"last_sequence":u64::MAX}
    });
    assert_valid("schemas/status-result-v2.schema.json", &max_trace_window);

    let mut decision_history = verbose.clone();
    decision_history["decision_history"] = json!({
        "entries":[fixtures["podway.decision-result/v1"]["record"].clone()],
        "trace_truncated":false,
        "trace_window":{"first_sequence":1,"last_sequence":1}
    });
    assert_valid("schemas/status-result-v2.schema.json", &decision_history);

    let mut rework_history = verbose.clone();
    rework_history["rework_history"] = json!({
        "entries":[{
            "trace_sequence":1,"kind":"manual","from_graph_node_id":"finish",
            "to_graph_node_id":"work","target_attempt_id":UUID,"reason":"retry",
            "reactivated":true,"recorded_at":"2026-08-04T00:00:00.000Z"
        }],
        "trace_truncated":false,
        "trace_window":{"first_sequence":1,"last_sequence":1}
    });
    assert_valid("schemas/status-result-v2.schema.json", &rework_history);

    let inadmissible_preview = json!({
        "schema":"podway.procedure-preview-result/v1","file":"workflow.yaml",
        "admissible":false,"checks":{"validate":false,"vet":false,"lint":false},
        "diagnostics":[{
            "code":"AUTHORING_SCHEMA_INVALID","severity":"error",
            "schema":"podway.procedure/v2","source_path":"workflow.yaml",
            "location":{"line":1,"column":1,"end_line":1,"end_column":1},
            "field":"$","message":"The document is invalid.",
            "hint":"Correct the document and retry."
        }],
        "diagnostics_truncated":false,"diagnostics_total":1
    });
    assert_valid(
        "schemas/procedure-preview-result-v1.schema.json",
        &inadmissible_preview,
    );
    for field in [
        "procedure_schema",
        "procedure_id",
        "procedure_version",
        "purpose",
        "procedure_digest",
        "goal_tracking",
        "goal_assessment_graph_node_ids",
        "summary",
        "graph",
        "mermaid",
        "start_suggestion",
    ] {
        let mut candidate = inadmissible_preview.clone();
        candidate[field] = fixtures["podway.procedure-preview-result/v1"][field].clone();
        assert_invalid(
            "schemas/procedure-preview-result-v1.schema.json",
            &candidate,
        );
    }
    for checks in [
        json!({"validate":false,"vet":true,"lint":false}),
        json!({"validate":false,"vet":false,"lint":true}),
        json!({"validate":true,"vet":true,"lint":false}),
    ] {
        let mut candidate = inadmissible_preview.clone();
        candidate["checks"] = checks;
        assert_invalid(
            "schemas/procedure-preview-result-v1.schema.json",
            &candidate,
        );
    }
    for (field, value) in [("diagnostics", json!([])), ("diagnostics_total", json!(0))] {
        let mut candidate = inadmissible_preview.clone();
        candidate[field] = value;
        assert_invalid(
            "schemas/procedure-preview-result-v1.schema.json",
            &candidate,
        );
    }
    let mut false_inadmissible = fixtures["podway.procedure-preview-result/v1"].clone();
    false_inadmissible["admissible"] = json!(false);
    false_inadmissible
        .as_object_mut()
        .unwrap()
        .remove("start_suggestion");
    assert_invalid(
        "schemas/procedure-preview-result-v1.schema.json",
        &false_inadmissible,
    );

    let mut invalid_goal_assessment_summary = verbose;
    invalid_goal_assessment_summary["stale_goal_assessment_history"] = json!({
        "entries":[{
            "trace_sequence":1,"session_id":UUID,"session_revision":1,"procedure_snapshot_id":UUID,
            "procedure_digest":DIGEST,"graph_node_id":"review","node_definition_id":"review",
            "attempt_id":UUID,"attempt_number":1,"goal_revision":1,"assessment":"session_goal",
            "option_id":"accept","effect":"advance","target_graph_node_id":"finish",
            "mode":"assessment","outcome":"achieved",
            "criterion_statuses":[{"criterion_id":"tests","status":"unsatisfied","citations":[]}],
            "references":[],"actor":null,"recorded_at":"2026-08-04T00:00:00.000Z",
            "record_digest":DIGEST
        }],
        "trace_truncated":false,
        "trace_window":{"first_sequence":1,"last_sequence":1}
    });
    assert_invalid(
        "schemas/status-result-v2.schema.json",
        &invalid_goal_assessment_summary,
    );

    let mut invalid_not_achieved_summary = invalid_goal_assessment_summary.clone();
    invalid_not_achieved_summary["stale_goal_assessment_history"]["entries"][0]["outcome"] =
        json!("not_achieved");
    invalid_not_achieved_summary["stale_goal_assessment_history"]["entries"][0]["criterion_statuses"]
        [0]["status"] = json!("satisfied");
    assert_invalid(
        "schemas/status-result-v2.schema.json",
        &invalid_not_achieved_summary,
    );

    let mut invalid_superseded_summary = invalid_goal_assessment_summary;
    invalid_superseded_summary["stale_goal_assessment_history"]["entries"][0]["mode"] =
        json!("applicability");
    invalid_superseded_summary["stale_goal_assessment_history"]["entries"][0]["outcome"] =
        json!("superseded");
    invalid_superseded_summary["stale_goal_assessment_history"]["entries"][0]["criterion_statuses"]
        [0]["status"] = json!("not_applicable");
    invalid_superseded_summary["stale_goal_assessment_history"]["entries"][0]["criterion_statuses"]
        [0]["citations"] = json!([{"local_item_id":"reviewed"}]);
    assert_invalid(
        "schemas/status-result-v2.schema.json",
        &invalid_superseded_summary,
    );
}

#[test]
fn v2ctr003_mutation_successes_use_the_bounded_v2_admission_contract() {
    for schema in [
        "podway.detached-admission-result/v2",
        "podway.session-start-result/v2",
        "podway.stage-transition-result/v2",
        "podway.item-mutation-result/v2",
        "podway.decision-result/v1",
        "podway.rework-result/v1",
        "podway.goal-definition-result/v1",
        "podway.goal-revision-result/v1",
        "podway.criterion-assessment-result/v1",
    ] {
        let contract = EXISTING_ROUTE_RESULT_SCHEMAS_V2
            .iter()
            .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
            .find(|contract| contract.schema == schema)
            .unwrap();
        let result = examples()[schema].clone();
        assert_valid(contract.schema_path, &result);

        let mut missing = result.clone();
        missing.as_object_mut().unwrap().remove("admission");
        assert_invalid(contract.schema_path, &missing);

        let mut not_admitted = result.clone();
        not_admitted["admission"] = json!({"admitted":false});
        assert_invalid(contract.schema_path, &not_admitted);

        let mut open = result;
        open["admission"]["idempotency_key"] = json!("legacy-shape");
        assert_invalid(contract.schema_path, &open);

        let mut maximum = examples()[schema].clone();
        maximum["admission"]["workspace_sequence"] = json!(u64::MAX);
        assert_valid(contract.schema_path, &maximum);

        let mut overflow = maximum;
        overflow["admission"]["workspace_sequence"] =
            serde_json::from_str("18446744073709551616").unwrap();
        assert_invalid(contract.schema_path, &overflow);
    }

    let mut detached = examples()["podway.detached-admission-result/v2"].clone();
    detached["job"] = json!({"job_id":UUID});
    assert_invalid(
        "schemas/detached-admission-result-v2.schema.json",
        &detached,
    );

    let mut output = json!({
        "schema":OUTPUT_SCHEMA_V2,"request_id":UUID,"command":"session.complete",
        "generated_at":"2026-08-04T00:00:00.000Z",
        "result":examples()["podway.stage-transition-result/v2"].clone(),"warnings":[]
    });
    add_admitted_envelope_metadata(&mut output);
    assert_valid("schemas/output-v2.schema.json", &output);
    assert!(admission_matches_job(&output));

    output["job"]["sequence"] = json!(2);
    assert!(!admission_matches_job(&output));
}

#[test]
fn v2ctr003_retained_envelope_warnings_are_bounded_and_framing_is_separate() {
    let warning = Map::from_iter([
        ("code".to_owned(), json!("ADVISORY")),
        ("path".to_owned(), json!("workflow.yaml")),
        ("message".to_owned(), json!("Review this field.")),
    ]);
    assert!(validate_v2_output_warnings(std::slice::from_ref(&warning)));
    assert!(!validate_v2_output_warnings(&vec![warning.clone(); 5]));

    let mut oversized = warning.clone();
    oversized.insert("message".to_owned(), json!("x".repeat(513)));
    assert!(!validate_v2_output_warnings(&[oversized]));

    let mut open = warning;
    open.insert("unknown".to_owned(), json!(true));
    assert!(!validate_v2_output_warnings(&[open]));

    let result = examples()["podway.procedure-validation-result/v2"].clone();
    let output = json!({
        "schema":OUTPUT_SCHEMA_V2, "request_id":UUID,
        "command":"procedure.validate", "generated_at":"2026-08-04T00:00:00.000Z",
        "result":result, "warnings":[]
    });
    assert_valid("schemas/output-v2.schema.json", &output);
    let encoded = serde_json::to_vec(&output).unwrap();
    assert!(validate_frame_payload_length(encoded.len()).is_ok());

    let mut legacy_output = output.clone();
    legacy_output["schema"] = json!("podway.output/v1");
    assert_invalid("schemas/output-v2.schema.json", &legacy_output);

    let mut wrong_route = output.clone();
    wrong_route["command"] = json!("procedure.graph");
    assert_invalid("schemas/output-v2.schema.json", &wrong_route);

    let mut oversized = output;
    oversized["padding"] = json!("x".repeat(1_048_576));
    assert_valid("schemas/output-v2.schema.json", &oversized);
    assert!(validate_frame_payload_length(serde_json::to_vec(&oversized).unwrap().len()).is_err());
}

fn result_map(value: &Value) -> Map<String, Value> {
    value.as_object().expect("result object").clone()
}

fn warning(code: &str) -> Map<String, Value> {
    result_map(&json!({
        "code": code, "path": "workflow.yaml", "message": "Review this field."
    }))
}

fn authoring_input(command: &str, result: &Value) -> OutputEnvelopeInputV2 {
    OutputEnvelopeInputV2 {
        request_id: RequestIdV1::new(UUID).unwrap(),
        command: CommandNameV1::new(command).unwrap(),
        generated_at: Rfc3339MillisV1::new("2026-08-04T00:00:00.000Z").unwrap(),
        workspace: None,
        job: None,
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
        let envelope = OutputEnvelopeV2::new(input).unwrap();
        assert_eq!(envelope.command().as_str(), command);
        assert_eq!(envelope.result(), &result_map(&result));
        assert_eq!(envelope.warnings(), &[warning("ADVISORY")]);

        let line = serde_json::to_string(&envelope).unwrap();
        assert!(
            line.starts_with(&format!(
                "{{\"schema\":\"{OUTPUT_SCHEMA_V2}\",\"request_id\":\"{UUID}\",\"command\":\"{command}\",\"generated_at\":"
            )),
            "unexpected canonical field order: {line}"
        );
        assert!(validate_frame_payload_length(line.len()).is_ok());

        let value: Value = serde_json::from_str(&line).unwrap();
        assert_valid("schemas/output-v2.schema.json", &value);
        for absent in ["workspace", "job", "session"] {
            assert!(
                value.get(absent).is_none(),
                "local authoring output must omit {absent}"
            );
        }
        assert_eq!(
            serde_json::from_str::<OutputEnvelopeV2>(&line).unwrap(),
            envelope
        );
    }
}

/// A representative `podway_core::AuthoringDiagnostic`, populated with its optional
/// `node_definition_id`, `graph_node_id`, and `related_graph_node_ids` fields — unlike the
/// hand-written JSON fixture in `v2ctr003_authoring_diagnostic_is_standalone_closed_and_bounded`,
/// which leaves `node_definition_id` absent.
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

    let envelope = OutputEnvelopeV2::new(authoring_input("procedure.lint", &result)).unwrap();
    let value = serde_json::to_value(&envelope).unwrap();
    // And it round-trips intact through the full v2 output envelope, resolving the schema's
    // cross-file `$ref` from `procedure-diagnostics-result-v1` into `authoring-diagnostic-v1`.
    assert_valid("schemas/output-v2.schema.json", &value);
    assert_eq!(value["result"]["diagnostics"], json!([diagnostic_value]));
}

#[test]
fn v2aut001_output_v2_envelope_rejects_unbound_results_and_open_warnings() {
    let source = examples()["podway.procedure-source-result/v1"].clone();
    let valid = serde_json::to_value(
        OutputEnvelopeV2::new(authoring_input("procedure.format", &source)).unwrap(),
    )
    .unwrap();
    assert_valid("schemas/output-v2.schema.json", &valid);

    let mut legacy = valid.clone();
    legacy["schema"] = json!("podway.output/v1");
    assert_invalid("schemas/output-v2.schema.json", &legacy);
    assert!(serde_json::from_value::<OutputEnvelopeV2>(legacy).is_err());

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
            examples()["podway.status-result/v2"].clone(),
        ),
        ("procedure.format", unknown_family),
        ("procedure.format", extra_field),
        ("procedure.format", released_v1_family),
    ] {
        assert_eq!(
            OutputEnvelopeV2::new(authoring_input(command, &result)),
            Err(ProtocolError::InvalidCommandResult {
                command: command.to_owned()
            }),
            "{command} accepted {result}"
        );
        let mut rejected = valid.clone();
        rejected["command"] = json!(command);
        rejected["result"] = result;
        assert_invalid("schemas/output-v2.schema.json", &rejected);
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
            OutputEnvelopeV2::new(input),
            Err(ProtocolError::InvalidOutputWarnings)
        );
        let mut rejected = valid.clone();
        rejected["warnings"] = warnings;
        assert_invalid("schemas/output-v2.schema.json", &rejected);
    }
}

#[test]
fn v2aut001_validate_command_result_v2_binds_registered_families_to_their_routes() {
    let examples = examples();
    for contract in EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
    {
        let result = result_map(&examples[contract.schema]);
        for command in contract.commands {
            assert!(
                validate_command_result_v2(command, &result).is_ok(),
                "{command} rejected {}",
                contract.schema
            );
        }
    }

    let source = result_map(&examples["podway.procedure-source-result/v1"]);
    let diagnostics = result_map(&diagnostics_result("format"));
    assert!(validate_command_result_v2("procedure.format", &source).is_ok());
    assert!(validate_command_result_v2("procedure.format", &diagnostics).is_ok());
    assert!(validate_command_result_v2("procedure.lint", &diagnostics).is_ok());
    assert!(validate_command_result_v2("procedure.lint", &source).is_err());
    assert!(validate_command_result_v2("procedure.graph", &source).is_err());

    let mut released_v1_family = source.clone();
    released_v1_family.insert(
        "schema".to_owned(),
        json!("podway.procedure-validation-result/v1"),
    );
    assert!(validate_command_result_v2("procedure.validate", &released_v1_family).is_err());

    let mut missing = source;
    missing.remove("schema");
    assert!(validate_command_result_v2("procedure.format", &missing).is_err());
}

// ---------------------------------------------------------------------------------------------
// V2AUT-008: `procedure.validate` carries two closed families
// ---------------------------------------------------------------------------------------------

/// A real `podway.procedure-validation-result/v1` body: the family `procedure validate` reports for
/// a Procedure v1 document, unchanged by V2AUT-008.
///
/// Captured verbatim from `podway --json procedure validate` on the smallest legal v1 document, so
/// its `canonical_json`, `procedure`, and `digest` genuinely agree — the three the v1 validator
/// re-derives and cross-checks. A hand-written body would only ever prove the validator rejects it.
fn validation_result_v1() -> Map<String, Value> {
    result_map(&json!({
        "schema": "podway.procedure-validation-result/v1",
        "file": "workflow.yaml",
        "digest": "sha256:40265a5ce34cd76f257b1c7cbc783b30ebaa6702bc00dc161954975fed1dee77",
        "procedure": {
            "id": "release",
            "name": "Release",
            "rework": {"allow_return_to": ["prepare"]},
            "schema": "podway.procedure/v1",
            "stages": [{
                "id": "prepare",
                "instructions": [],
                "items": [],
                "title": "Prepare",
            }],
            "version": "1",
        },
        "warnings": [{
            "code": "stage_has_no_required_items",
            "message": "procedure warning: stage_has_no_required_items",
            "path": "stages/prepare",
        }],
        "canonical_json": "{\"id\":\"release\",\"name\":\"Release\",\"rework\":{\"allow_return_to\":[\"prepare\"]},\"schema\":\"podway.procedure/v1\",\"stages\":[{\"id\":\"prepare\",\"instructions\":[],\"items\":[],\"title\":\"Prepare\"}],\"version\":\"1\"}",
    }))
}

#[test]
fn v2aut008_the_v1_selector_abstains_on_a_procedure_validate_diagnostics_result() {
    // `procedure validate` reports the v1 validation family for a Procedure v1 document and the
    // shared authoring diagnostics family for a Procedure v2 one. The v1 selector must not claim
    // the second: stamping `podway.procedure-validation-result/v1` onto a diagnostics body, or
    // decoding one as a v1 result, would both corrupt a correct result.
    let diagnostics = result_map(&diagnostics_result("validate"));

    let mut stamped = diagnostics.clone();
    ensure_command_result_schema_v1("procedure.validate", &mut stamped);
    assert_eq!(
        stamped.get("schema").and_then(Value::as_str),
        Some(PROCEDURE_DIAGNOSTICS_RESULT_SCHEMA_V1),
        "the diagnostics family survives the v1 schema stamp untouched",
    );
    assert_eq!(stamped, diagnostics, "nothing else is added either");

    assert!(
        validate_command_result_v1("procedure.validate", &diagnostics).is_ok(),
        "the v1 validator abstains rather than rejecting a family it does not own",
    );
    // The family is still validated — by the v2 registry, which binds it to this route.
    assert!(validate_command_result_v2("procedure.validate", &diagnostics).is_ok());
}

#[test]
fn v2aut008_the_v1_selector_still_owns_the_procedure_validate_v1_family() {
    // The other direction: abstention is conditional on the diagnostics discriminator, so a v1
    // validation result is selected, stamped, and validated exactly as before.
    let expected = validation_result_v1();

    let mut without_schema = expected.clone();
    without_schema.remove("schema");
    ensure_command_result_schema_v1("procedure.validate", &mut without_schema);
    assert_eq!(
        without_schema.get("schema").and_then(Value::as_str),
        Some("podway.procedure-validation-result/v1"),
    );
    assert!(validate_command_result_v1("procedure.validate", &expected).is_ok());

    // A v1 body carrying the wrong known v1 schema is still a rejection, and a diagnostics body is
    // still not admissible as the v1 family under any other route.
    let mut wrong_family = expected;
    wrong_family.insert("schema".to_owned(), json!("podway.version-result/v1"));
    assert!(validate_command_result_v1("procedure.validate", &wrong_family).is_err());
    assert!(
        validate_command_result_v2(
            "procedure.show",
            &result_map(&diagnostics_result("validate"))
        )
        .is_err(),
        "abstention does not make the diagnostics family routable anywhere",
    );
}
