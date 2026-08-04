use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use jsonschema::{Retrieve, Uri};
use podway_protocol::{
    EXISTING_ROUTE_RESULT_SCHEMAS_V2, NEW_ROUTE_RESULT_SCHEMAS_V1,
    decode_result_schema_contract_v2, result_schema_top_level_fields_v2,
    validate_v2_output_envelope_value, validate_v2_output_warnings,
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
            json!({"schema":"podway.detached-admission-result/v2","detached":true,"admission":{"job_id":UUID,"workspace_sequence":1,"idempotency_key":"key"},"job":{"job_id":UUID,"command":"session.complete","state":"queued","workspace_sequence":1,"created_at":"2026-08-04T00:00:00.000Z"}}),
        ),
        (
            "podway.session-start-result/v2",
            json!({"schema":"podway.session-start-result/v2","procedure_schema":"podway.procedure/v2","procedure_digest":DIGEST,"dry_run":false,"goal_tracking":false,"session_id":UUID,"revision":1,"entry_graph_node_id":"work"}),
        ),
        (
            "podway.compact-status-result/v2",
            json!({"schema":"podway.compact-status-result/v2","procedure":{"schema":"podway.procedure/v2","id":"workflow","version":"1","digest":DIGEST},"session":{"id":UUID,"lifecycle":"running","revision":1},"current":{"node":node,"attempt":attempt,"readiness":readiness,"missing_required_item_count":0,"blockers_total":0},"goal_tracking":false,"goal_defined":false,"trace_length":1,"counters":[],"items":[],"blockers":[],"queue":queue}),
        ),
        (
            "podway.status-result/v2",
            json!({"schema":"podway.status-result/v2","tier":"standard","procedure":{"schema":"podway.procedure/v2","id":"workflow","version":"1","digest":DIGEST},"session":{"id":UUID,"lifecycle":"running","revision":1},"current":{"node":node,"attempt":attempt,"readiness":readiness,"missing_required_item_count":0,"blockers_total":0},"purpose":"test","goal_tracking":false,"goal_defined":false,"trace_length":1,"counters":[],"items":[],"blockers":[],"queue":queue,"missing_required_item_ids":[],"blocker_window":[],"blockers_truncated":false,"item_values":[],"items_total":0,"items_truncated":false,"references":[],"allowed_option_ids":[],"terminal":true,"allowed_manual_rework_targets":[]}),
        ),
        (
            "podway.next-result/v2",
            json!({"schema":"podway.next-result/v2","procedure_schema":"podway.procedure/v2","procedure_digest":DIGEST,"goal_tracking":false,"goal_defined":false,"node":node,"attempt":attempt,"trace_length":1,"counters":[],"queue":queue,"revision":1,"readiness":readiness,"title":"Work","intent":"Do work","instructions":[],"missing_required_item_count":0,"missing_required_items":[],"blockers_total":0,"blockers":[],"blockers_truncated":false,"terminal":true,"allowed_actions":["session.complete"],"suggestions":[{"command":"session.complete","argv":["complete"]}],"references":[],"readback":[],"allowed_manual_rework_targets":[]}),
        ),
        (
            "podway.stage-transition-result/v2",
            json!({"schema":"podway.stage-transition-result/v2","transition":"complete","from_graph_node_id":"work","from_attempt_id":UUID,"revision":2,"session_state":"completed"}),
        ),
        (
            "podway.item-mutation-result/v2",
            json!({"schema":"podway.item-mutation-result/v2","changed":true,"graph_node_id":"work","attempt_id":UUID,"attempt_number":1,"item_id":"done","revision":2}),
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
            json!({"schema":"podway.decision-result/v1","graph_node_id":"review","attempt_id":UUID,"attempt_number":1,"option_id":"accept","effect":"advance","revision":2,"session_state":"running","record":{"trace_sequence":1,"session_id":UUID,"session_revision":1,"procedure_schema":"podway.procedure/v2","procedure_snapshot_id":UUID,"procedure_digest":DIGEST,"graph_node_id":"review","node_definition_id":"review","attempt_id":UUID,"attempt_number":1,"goal_revision":null,"option_id":"accept","effect":"advance","target_graph_node_id":"finish","reason":"accepted","recorded_at":"2026-08-04T00:00:00.000Z","references":[]}}),
        ),
        (
            "podway.rework-result/v1",
            json!({"schema":"podway.rework-result/v1","from_graph_node_id":"finish","to_graph_node_id":"work","target_attempt_id":UUID,"reason":"retry","reactivated":true,"revision":2}),
        ),
        (
            "podway.goal-definition-result/v1",
            json!({"schema":"podway.goal-definition-result/v1","goal_revision":1,"statement":"Ship safely","criteria":[{"criterion_id":"tests","statement":"Tests pass"}],"actor":"master","recorded_at":"2026-08-04T00:00:00.000Z","revision":2}),
        ),
        (
            "podway.goal-revision-result/v1",
            json!({"schema":"podway.goal-revision-result/v1","goal_revision":2,"statement":"Ship safely","criteria":[{"criterion_id":"tests","statement":"Tests pass"}],"reason":"clarify","actor":"master","recorded_at":"2026-08-04T00:00:00.000Z","rework_to":"work","reactivated":false,"revision":3}),
        ),
        (
            "podway.criterion-assessment-result/v1",
            json!({"schema":"podway.criterion-assessment-result/v1","graph_node_id":"assess","attempt_id":UUID,"goal_revision":1,"mode":"assessment","result":{"criterion_id":"tests","status":"satisfied","reason":"verified","citations":[]},"complete":true,"determined_outcome":"achieved","revision":3}),
        ),
    ])
}

#[test]
fn v2ctr003_registry_is_versioned_and_covers_exactly_the_v2_authoring_routes() {
    assert_eq!(EXISTING_ROUTE_RESULT_SCHEMAS_V2.len(), 8);
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
    assert_eq!(schema_pairs.len(), 51);

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
    assert!(
        !EXISTING_ROUTE_RESULT_SCHEMAS_V2
            .iter()
            .any(|entry| entry.schema.starts_with("podway.job-"))
    );
    let mut incomplete = examples()["podway.next-result/v2"].clone();
    incomplete.as_object_mut().unwrap().remove("node");
    assert_eq!(
        decode_result_schema_contract_v2(incomplete.as_object().unwrap()),
        None
    );
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

    let mut dry_run = fixtures["podway.session-start-result/v2"].clone();
    dry_run["dry_run"] = json!(true);
    dry_run.as_object_mut().unwrap().remove("session_id");
    dry_run.as_object_mut().unwrap().remove("revision");
    dry_run
        .as_object_mut()
        .unwrap()
        .remove("entry_graph_node_id");
    assert_valid("schemas/session-start-result-v2.schema.json", &dry_run);

    let mut terminal_compact = fixtures["podway.compact-status-result/v2"].clone();
    terminal_compact["session"]["lifecycle"] = json!("completed");
    terminal_compact["current"] = Value::Null;
    assert_valid(
        "schemas/compact-status-result-v2.schema.json",
        &terminal_compact,
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

    let mut standard_history = fixtures["podway.status-result/v2"].clone();
    standard_history["current_trace_history"] = json!({"entries":[],"trace_truncated":false,"trace_window":{"first_sequence":1,"last_sequence":1}});
    assert_invalid("schemas/status-result-v2.schema.json", &standard_history);

    let mut false_skip = fixtures["podway.status-result/v2"].clone();
    false_skip["skip"] = json!({"allowed":false,"reason_required":true});
    assert_invalid("schemas/status-result-v2.schema.json", &false_skip);

    let mut bad_blocker = fixtures["podway.status-result/v2"].clone();
    bad_blocker["blockers"] = json!([{"blocker_id":"not-a-uuid","attempt_id":UUID,"state":"open"}]);
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

    let reset = json!({"schema":"podway.stage-transition-result/v2","transition":"reset","reset":true,"revision":3});
    assert_valid("schemas/stage-transition-result-v2.schema.json", &reset);
    let unblock_all = json!({"schema":"podway.stage-transition-result/v2","transition":"unblock","from_graph_node_id":"work","from_attempt_id":UUID,"all":true,"revision":3,"session_state":"running"});
    assert_valid(
        "schemas/stage-transition-result-v2.schema.json",
        &unblock_all,
    );
    let invalid_cancel = json!({"schema":"podway.stage-transition-result/v2","transition":"cancel","from_graph_node_id":"work","from_attempt_id":UUID,"revision":3,"session_state":"running"});
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
fn v2ctr003_retained_envelope_warnings_have_a_production_bound() {
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
        "schema":"podway.output/v1", "request_id":UUID,
        "command":"procedure.validate", "generated_at":"2026-08-04T00:00:00.000Z",
        "result":result, "warnings":[]
    });
    assert!(validate_v2_output_envelope_value(&output));

    let mut wrong_route = output.clone();
    wrong_route["command"] = json!("procedure.graph");
    assert!(!validate_v2_output_envelope_value(&wrong_route));

    let mut oversized = output;
    oversized["padding"] = json!("x".repeat(1_048_576));
    assert!(!validate_v2_output_envelope_value(&oversized));

    let compact = examples()["podway.compact-status-result/v2"].clone();
    let mut compact_output = json!({
        "schema":"podway.output/v1", "request_id":UUID,
        "command":"session.status", "generated_at":"2026-08-04T00:00:00.000Z",
        "result":compact, "warnings":[]
    });
    assert!(validate_v2_output_envelope_value(&compact_output));
    compact_output["padding"] = json!("");
    let base_length = serde_json::to_vec(&compact_output).unwrap().len() + 1;
    compact_output["padding"] = json!("x".repeat(262_145 - base_length));
    assert_eq!(
        serde_json::to_vec(&compact_output).unwrap().len() + 1,
        262_145
    );
    assert!(!validate_v2_output_envelope_value(&compact_output));

    let mut busy = examples()["podway.compact-status-result/v2"].clone();
    busy["queue"]["pending_mutations"] = json!(true);
    let busy_output = json!({
        "schema":"podway.output/v1", "request_id":UUID,
        "command":"session.status", "generated_at":"2026-08-04T00:00:00.000Z",
        "result":busy, "warnings":[]
    });
    assert!(!validate_v2_output_envelope_value(&busy_output));

    let mut verbose = examples()["podway.status-result/v2"].clone();
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
    verbose["current_trace_history"]["padding"] = json!("x".repeat(65_536));
    let verbose_output = json!({
        "schema":"podway.output/v1", "request_id":UUID,
        "command":"session.status", "generated_at":"2026-08-04T00:00:00.000Z",
        "result":verbose, "warnings":[]
    });
    assert!(!validate_v2_output_envelope_value(&verbose_output));
}
