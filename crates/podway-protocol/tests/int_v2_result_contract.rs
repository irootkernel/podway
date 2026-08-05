use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use jsonschema::{Retrieve, Uri};
use podway_protocol::{
    EXISTING_ROUTE_RESULT_SCHEMAS_V2, ErrorEnvelopeV1, MAX_V2_RUNTIME_ERROR_MESSAGE_CHARS_V1,
    NEW_ROUTE_RESULT_SCHEMAS_V1, OUTPUT_SCHEMA_V2, V2_RUNTIME_ERROR_CODES_V1,
    decode_result_schema_contract_v2, result_schema_top_level_fields_v2,
    validate_frame_payload_length, validate_v2_output_warnings,
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
            json!({"schema":"podway.next-result/v2","procedure_schema":"podway.procedure/v2","procedure_digest":DIGEST,"goal_tracking":false,"goal_defined":false,"node":node,"attempt":attempt,"trace_length":1,"counters":[{"graph_node_id":"work","attempt_count":1,"rework_traversal_count":0}],"queue":queue,"revision":1,"readiness":readiness,"title":"Work","intent":"Do work","instructions":[],"missing_required_item_count":0,"missing_required_items":[],"blockers_total":0,"blockers":[],"blockers_truncated":false,"terminal":true,"allowed_actions":["session.complete"],"suggestions":[{"command":"session.complete","argv":["complete"]}],"references":[],"readback":[],"allowed_manual_rework_targets":[]}),
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
            json!({"schema":"podway.procedure-preview-result/v1","file":"workflow.yaml","admissible":true,"procedure_schema":"podway.procedure/v2","procedure_id":"workflow","procedure_version":"1","purpose":"test","procedure_digest":DIGEST,"goal_tracking":false,"goal_assessment_graph_node_ids":[],"summary":{"definition_count":1,"graph_node_count":1,"action_node_count":1,"decision_node_count":0,"route_count":0,"cycle_count":0,"evidence_reference_count":0,"skippable_node_count":0,"manual_rework_target_count":0},"checks":{"validate":true,"vet":true,"lint":true},"graph":{"entry_graph_node_id":"work","terminal_graph_node_ids":["work"],"nodes":[{"graph_node_id":"work","node_definition_id":"work","node_type":"action","terminal":true,"skippable":false}],"edges":[]},"mermaid":"flowchart TD\n  work","start_suggestion":{"command":"session.start","argv":["start","--procedure","workflow.yaml","--confirm-digest",DIGEST]},"diagnostics":[],"diagnostics_truncated":false,"diagnostics_total":0}),
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
            }
            assert_valid("schemas/output-v2.schema.json", &output);
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
    assert_eq!(schema_pairs.len(), 52);

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
        "diagnostics":[],"diagnostics_truncated":false,"diagnostics_total":0
    });
    assert_valid(
        "schemas/procedure-preview-result-v1.schema.json",
        &inadmissible_preview,
    );
    let mut inadmissible_with_start = inadmissible_preview;
    inadmissible_with_start["start_suggestion"] =
        fixtures["podway.procedure-preview-result/v1"]["start_suggestion"].clone();
    assert_invalid(
        "schemas/procedure-preview-result-v1.schema.json",
        &inadmissible_with_start,
    );
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
