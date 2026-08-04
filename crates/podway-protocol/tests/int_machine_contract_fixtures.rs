use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use jsonschema::{Retrieve, Uri};
use podway_core::{MAX_OPEN_BLOCKERS_PER_ATTEMPT_V1, MAX_STAGE_ITEMS};
use podway_protocol::{
    EXISTING_ROUTE_RESULT_SCHEMAS_V2, MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1,
    NEW_ROUTE_RESULT_SCHEMAS_V1, RequestEnvelopeV1, ResponseEnvelopeV1,
    SUPPORTED_RESULT_SCHEMAS_V1, error_code_catalog_v1, validate_command_result_v1,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const SCHEMA_BASE: &str = "https://podway.invalid/schemas/";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscriminatorFixture {
    schema_version: String,
    result_fixtures: BTreeMap<String, ContractFixture>,
    result_bindings: Vec<ResultBinding>,
    error_fixtures: BTreeMap<String, ErrorFixture>,
    error_bindings: Vec<ErrorBinding>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractFixture {
    schema: String,
    schema_file: String,
    malformed_type_pointer: String,
    value: Option<Value>,
    source: Option<String>,
    pointer: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorFixture {
    schema: String,
    schema_file: String,
    malformed_type_pointer: String,
    details: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultBinding {
    command: String,
    fixtures: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorBinding {
    code: String,
    fixture: String,
}

#[derive(Debug, Deserialize)]
struct ErrorCatalog {
    errors: Vec<ErrorCatalogEntry>,
}

#[derive(Clone, Debug, Deserialize)]
struct ErrorCatalogEntry {
    code: String,
    details_schema: Option<String>,
    exit_code: u8,
    retryable: bool,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    assets: Vec<ManifestAsset>,
}

#[derive(Debug, Deserialize)]
struct ManifestAsset {
    kind: String,
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct V2FixtureCatalog {
    schema: String,
    version: u8,
    fixture_root: String,
    source_acceptance_matrix: String,
    source_compatibility_matrix: String,
    source_payload_matrix: String,
    evidence_policy: String,
    fixtures: Vec<V2FixtureAsset>,
    cases: Vec<V2FixtureCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct V2FixtureAsset {
    id: String,
    path: String,
    media_type: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct V2FixtureCase {
    id: String,
    fixture_class: String,
    fixture_ids: Vec<String>,
    acceptance_ids: Vec<String>,
    specialized_ids: Vec<String>,
    owning_tasks: Vec<String>,
    evidence_level: String,
    implementation_status: String,
}

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

fn read_json(relative: &str) -> Value {
    let relative = relative
        .strip_prefix("schemas/")
        .map(|name| PathBuf::from("assets/schemas").join(name))
        .unwrap_or_else(|| PathBuf::from(relative));
    serde_json::from_slice(&fs::read(root().join(relative)).unwrap()).unwrap()
}

fn fixture() -> DiscriminatorFixture {
    serde_json::from_value(read_json(
        "tests/fixtures/contract/discriminator-catalog-v1.json",
    ))
    .unwrap()
}

fn materialize(contract: &ContractFixture) -> Value {
    match (&contract.value, &contract.source) {
        (Some(value), None) => value.clone(),
        (None, Some(source)) => {
            let value = read_json(source);
            contract
                .pointer
                .as_deref()
                .map_or(value.clone(), |pointer| {
                    value.pointer(pointer).unwrap().clone()
                })
        }
        _ => panic!("fixture must select exactly one of value or source"),
    }
}

fn result_map(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn assert_result_invalid(contract: &ContractFixture, command: &str, value: Value) {
    assert_schema_invalid(&contract.schema_file, &value);
    assert!(
        validate_command_result_v1(command, &result_map(value)).is_err(),
        "runtime accepted a schema-invalid {command} result"
    );
}

fn contains_const(value: &Value, expected: &str) -> bool {
    match value {
        Value::Object(object) => {
            object.get("const").and_then(Value::as_str) == Some(expected)
                || object.values().any(|value| contains_const(value, expected))
        }
        Value::Array(values) => values.iter().any(|value| contains_const(value, expected)),
        _ => false,
    }
}

fn parse_command_catalog() -> BTreeMap<String, Vec<String>> {
    let source =
        fs::read_to_string(root().join("assets/specifications/command-catalog.yaml")).unwrap();
    let mut bindings = BTreeMap::new();
    let mut command = None;
    let mut schemas = Vec::new();
    let mut in_schemas = false;
    for line in source.lines() {
        if let Some(name) = line.strip_prefix("- name: ") {
            if let Some(previous) = command.replace(name.to_owned())
                && !schemas.is_empty()
            {
                assert!(
                    bindings
                        .insert(previous, std::mem::take(&mut schemas))
                        .is_none()
                );
            }
            in_schemas = false;
        } else if line == "  result_schemas:" {
            in_schemas = true;
        } else if in_schemas {
            if let Some(schema) = line.strip_prefix("  - podway.") {
                schemas.push(format!("podway.{schema}"));
            } else if !line.starts_with("  - ") {
                in_schemas = false;
            }
        }
    }
    if let Some(command) = command
        && !schemas.is_empty()
    {
        assert!(bindings.insert(command, schemas).is_none());
    }
    bindings
}

fn parse_error_catalog() -> BTreeMap<String, ErrorCatalogEntry> {
    serde_json::from_value::<ErrorCatalog>(read_json("assets/specifications/error-codes.json"))
        .unwrap()
        .errors
        .into_iter()
        .filter(|entry| entry.details_schema.is_some())
        .map(|entry| (entry.code.clone(), entry))
        .collect()
}

#[test]
fn mcont006_runtime_and_authoring_catalogs_are_frozen_disjoint_and_decoder_bound() {
    let runtime =
        serde_json::from_value::<ErrorCatalog>(read_json("assets/specifications/error-codes.json"))
            .unwrap();
    let expected = runtime
        .errors
        .iter()
        .map(|entry| (entry.code.as_str(), entry.exit_code, entry.retryable))
        .collect::<Vec<_>>();
    assert_eq!(error_code_catalog_v1().collect::<Vec<_>>(), expected);
    assert_eq!(expected.len(), 91);

    let authoring = read_json("assets/specifications/authoring-diagnostics.json");
    let diagnostic_codes = authoring["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["code"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(diagnostic_codes.len(), 51);
    let runtime_codes = expected
        .iter()
        .map(|(code, _, _)| *code)
        .collect::<BTreeSet<_>>();
    assert!(runtime_codes.is_disjoint(&diagnostic_codes));
    for mandatory in [
        "EVIDENCE_SOURCE_DOES_NOT_DOMINATE_CONSUMER",
        "SKIPPABLE_EVIDENCE_SOURCE",
        "EVIDENCE_SELECTOR_UNKNOWN_ITEM",
        "READBACK_BUDGET_EXCEEDED",
        "NEXT_STATIC_BUDGET_EXCEEDED",
        "REWORK_TARGET_NOT_DOMINATING",
        "NO_REACTIVATION_PATH",
    ] {
        assert!(diagnostic_codes.contains(mandatory));
    }
}

fn local_schemas() -> LocalSchemas {
    let mut resources = HashMap::new();
    for entry in fs::read_dir(root().join("assets/schemas")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let schema: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let identifier = schema["$id"].as_str().unwrap().to_owned();
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(resources.insert(identifier, schema.clone()).is_none());
        assert!(
            resources
                .insert(format!("{SCHEMA_BASE}{filename}"), schema)
                .is_none()
        );
    }
    LocalSchemas(Arc::new(resources))
}

fn schema_errors(relative: &str, instance: &Value) -> Vec<String> {
    let mut schema = read_json(relative);
    schema.as_object_mut().unwrap().remove("$id");
    let filename = Path::new(relative).file_name().unwrap().to_str().unwrap();
    let validator = jsonschema::draft202012::options()
        .with_base_uri(format!("{SCHEMA_BASE}{filename}"))
        .with_retriever(local_schemas())
        .should_validate_formats(true)
        .build(&schema)
        .unwrap_or_else(|error| panic!("{relative} does not compile: {error}"));
    validator
        .iter_errors(instance)
        .map(|error| error.to_string())
        .collect()
}

fn assert_schema_valid(relative: &str, instance: &Value) {
    let errors = schema_errors(relative, instance);
    assert!(
        errors.is_empty(),
        "{relative} rejected fixture: {errors:#?}"
    );
}

fn assert_schema_invalid(relative: &str, instance: &Value) {
    assert!(
        !schema_errors(relative, instance).is_empty(),
        "{relative} accepted invalid fixture: {instance}"
    );
}

fn mutations(value: &Value, discriminator: &str, malformed_type_pointer: &str) -> Vec<Value> {
    let mut unknown = value.clone();
    unknown["unknown_contract_field"] = Value::Bool(true);
    let mut missing = value.clone();
    missing.as_object_mut().unwrap().remove("schema");
    let mut wrong_discriminator = value.clone();
    wrong_discriminator["schema"] = Value::String(format!("{discriminator}.future"));
    let mut wrong_type = value.clone();
    *wrong_type.pointer_mut(malformed_type_pointer).unwrap() = json!({"wrong": "type"});
    vec![unknown, missing, wrong_discriminator, wrong_type]
}

fn refresh_procedure_canonical_fields(value: &mut Value) {
    let canonical = podway_core::canonicalize_json_v1(&value["procedure"]).unwrap();
    value["digest"] = Value::String(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())));
    value["canonical_json"] = Value::String(canonical);
}

fn error_envelope(code: &str, entry: &ErrorCatalogEntry, details: Value) -> Value {
    json!({
        "schema": "podway.error/v1",
        "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
        "command": "version",
        "generated_at": "2026-07-13T03:10:04.123Z",
        "code": code,
        "message": "fixture error",
        "retryable": entry.retryable,
        "exit_code": entry.exit_code,
        "details": details
    })
}

#[test]
fn mcont006_result_fixtures_lock_catalog_schemas_and_runtime_decoders() {
    let fixture = fixture();
    assert_eq!(
        fixture.schema_version,
        "podway.discriminator-catalog-known-answers/v1"
    );
    let runtime_schemas = SUPPORTED_RESULT_SCHEMAS_V1
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(runtime_schemas.len(), SUPPORTED_RESULT_SCHEMAS_V1.len());
    assert_eq!(
        runtime_schemas,
        fixture
            .result_fixtures
            .values()
            .map(|fixture| fixture.schema.as_str())
            .collect()
    );
    let observed = fixture
        .result_bindings
        .iter()
        .map(|binding| {
            let schemas = binding
                .fixtures
                .iter()
                .map(|name| fixture.result_fixtures.get(name).unwrap().schema.clone())
                .collect::<BTreeSet<_>>();
            (binding.command.clone(), schemas)
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(observed.len(), fixture.result_bindings.len());
    assert_eq!(
        observed,
        parse_command_catalog()
            .into_iter()
            .filter_map(|(command, schemas)| {
                let schemas = schemas
                    .into_iter()
                    .filter(|schema| SUPPORTED_RESULT_SCHEMAS_V1.contains(&schema.as_str()))
                    .collect::<BTreeSet<_>>();
                (!schemas.is_empty()).then_some((command, schemas))
            })
            .collect()
    );

    let mut registered_v2 = BTreeMap::<String, BTreeSet<String>>::new();
    for contract in EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
    {
        for command in contract.commands {
            registered_v2
                .entry((*command).to_owned())
                .or_default()
                .insert(contract.schema.to_owned());
        }
    }
    let observed_v2 = parse_command_catalog()
        .into_iter()
        .filter_map(|(command, schemas)| {
            let schemas = schemas
                .into_iter()
                .filter(|schema| {
                    EXISTING_ROUTE_RESULT_SCHEMAS_V2
                        .iter()
                        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
                        .any(|contract| contract.schema == schema)
                })
                .collect::<BTreeSet<_>>();
            (!schemas.is_empty()).then_some((command, schemas))
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(observed_v2, registered_v2);

    let mut exercised = BTreeSet::new();
    for binding in &fixture.result_bindings {
        for name in &binding.fixtures {
            let contract = fixture.result_fixtures.get(name).unwrap();
            let value = materialize(contract);
            assert_eq!(value["schema"], contract.schema);
            assert_schema_valid(&contract.schema_file, &value);
            assert_schema_valid(
                "schemas/output-v1.schema.json",
                &json!({
                    "schema": "podway.output/v1",
                    "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
                    "command": binding.command,
                    "generated_at": "2026-07-13T03:10:04.123Z",
                    "result": value.clone(),
                    "warnings": []
                }),
            );
            validate_command_result_v1(&binding.command, &result_map(value.clone())).unwrap();
            exercised.insert(name);
            for (mutation_index, mutation) in
                mutations(&value, &contract.schema, &contract.malformed_type_pointer)
                    .into_iter()
                    .enumerate()
            {
                assert_schema_invalid(&contract.schema_file, &mutation);
                assert!(
                    validate_command_result_v1(&binding.command, &result_map(mutation)).is_err(),
                    "runtime accepted {name} mutation {mutation_index} for {}",
                    binding.command
                );
            }
        }
    }
    assert_eq!(exercised.len(), fixture.result_fixtures.len());

    let detached = materialize(fixture.result_fixtures.get("detached_mutation").unwrap());
    assert!(validate_command_result_v1("version", &result_map(detached.clone())).is_err());
    let cross_command = json!({
        "schema": "podway.output/v1",
        "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
        "command": "version",
        "generated_at": "2026-07-13T03:10:04.123Z",
        "result": detached.clone(),
        "warnings": []
    });
    assert_schema_invalid("schemas/output-v1.schema.json", &cross_command);

    let contract = fixture.result_fixtures.get("detached_mutation").unwrap();
    for mutation in mutations(
        &detached,
        &contract.schema,
        &contract.malformed_type_pointer,
    ) {
        let envelope = json!({
            "schema": "podway.output/v1",
            "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
            "command": "workspace.init",
            "generated_at": "2026-07-13T03:10:04.123Z",
            "result": mutation,
            "warnings": []
        });
        assert_schema_invalid("schemas/output-v1.schema.json", &envelope);
    }

    let mut start_without_digest =
        materialize(fixture.result_fixtures.get("detached_start").unwrap());
    start_without_digest
        .as_object_mut()
        .unwrap()
        .remove("procedure_digest");
    assert!(
        validate_command_result_v1("session.start", &result_map(start_without_digest.clone()))
            .is_err()
    );
    assert_schema_invalid(
        "schemas/output-v1.schema.json",
        &json!({
            "schema": "podway.output/v1", "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
            "command": "session.start", "generated_at": "2026-07-13T03:10:04.123Z",
            "result": start_without_digest, "warnings": []
        }),
    );

    let mut mutation_with_digest = detached.clone();
    mutation_with_digest["procedure_digest"] = Value::String(format!("sha256:{}", "a".repeat(64)));
    assert!(
        validate_command_result_v1("item.set", &result_map(mutation_with_digest.clone())).is_err()
    );
    assert_schema_invalid(
        "schemas/output-v1.schema.json",
        &json!({
            "schema": "podway.output/v1", "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
            "command": "item.set", "generated_at": "2026-07-13T03:10:04.123Z",
            "result": mutation_with_digest, "warnings": []
        }),
    );

    let reset = materialize(fixture.result_fixtures.get("reset_transition").unwrap());
    assert!(validate_command_result_v1("session.complete", &result_map(reset.clone())).is_err());
    assert_schema_invalid(
        "schemas/output-v1.schema.json",
        &json!({
            "schema": "podway.output/v1", "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
            "command": "session.complete", "generated_at": "2026-07-13T03:10:04.123Z",
            "result": reset, "warnings": []
        }),
    );

    let version = materialize(fixture.result_fixtures.get("version").unwrap());
    assert!(validate_command_result_v1("job.list", &result_map(version.clone())).is_err());
    assert_schema_invalid(
        "schemas/output-v1.schema.json",
        &json!({
            "schema": "podway.output/v1", "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
            "command": "job.list", "generated_at": "2026-07-13T03:10:04.123Z",
            "result": version, "warnings": []
        }),
    );

    for (name, command, pointer) in [
        ("detached_mutation", "workspace.init", "/detached"),
        ("reset_transition", "session.reset", "/reset"),
        ("session_start_dry_run", "session.start", "/dry_run"),
        ("stage_preview", "session.return", "/preview"),
        ("job_cancelled", "job.status", "/job/payload/cancelled"),
    ] {
        let contract = fixture.result_fixtures.get(name).unwrap();
        let mut mutation = materialize(contract);
        *mutation.pointer_mut(pointer).unwrap() = Value::Bool(false);
        assert_schema_invalid(&contract.schema_file, &mutation);
        assert!(
            validate_command_result_v1(command, &result_map(mutation)).is_err(),
            "runtime accepted false const at {pointer} for {name}"
        );
    }

    let version_contract = fixture.result_fixtures.get("version").unwrap();
    let mut version_without_source_commit = materialize(version_contract);
    version_without_source_commit
        .as_object_mut()
        .unwrap()
        .remove("source_commit");
    assert_schema_invalid(
        &version_contract.schema_file,
        &version_without_source_commit,
    );
    assert!(
        validate_command_result_v1("version", &result_map(version_without_source_commit)).is_err()
    );
}

#[test]
fn mcont006_status_item_values_are_closed_and_coupled_to_item_types() {
    let fixture = fixture();
    let contract = fixture.result_fixtures.get("status").unwrap();
    let mut status = materialize(contract);
    status["items"] = json!([
        {"id":"confirm","type":"confirm","prompt":"Confirm","required":true,"satisfied":true,"revision":1,"value":true},
        {"id":"text","type":"text","prompt":"Text","required":true,"satisfied":true,"revision":1,"value":"done"},
        {"id":"choice","type":"choice","prompt":"Choice","required":true,"satisfied":true,"revision":1,"value":"first"},
        {"id":"integer","type":"integer","prompt":"Integer","required":true,"satisfied":true,"revision":1,"value":7},
        {"id":"list","type":"list","prompt":"List","required":true,"satisfied":true,"revision":1,"value":["one","two"]},
        {"id":"artifact","type":"artifact","prompt":"Artifact","required":true,"satisfied":true,"revision":1,"value":{
            "location_type":"path","location":"reports/result.txt",
            "sha256_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "size_bytes":42,"media_type":"text/plain"
        }}
    ]);
    assert_schema_valid(&contract.schema_file, &status);
    validate_command_result_v1("session.status", &result_map(status.clone())).unwrap();

    let mut false_confirm = status.clone();
    false_confirm["items"][0]["value"] = Value::Bool(false);
    assert_schema_invalid(&contract.schema_file, &false_confirm);
    assert!(validate_command_result_v1("session.status", &result_map(false_confirm)).is_err());

    let mut mismatched_type = status.clone();
    mismatched_type["items"][3]["value"] = Value::String("7".to_owned());
    assert_schema_invalid(&contract.schema_file, &mismatched_type);
    assert!(validate_command_result_v1("session.status", &result_map(mismatched_type)).is_err());

    let mut nested_drift = status;
    nested_drift["items"][5]["value"]["future"] = Value::Bool(true);
    assert_schema_invalid(&contract.schema_file, &nested_drift);
    assert!(validate_command_result_v1("session.status", &result_map(nested_drift)).is_err());
}

#[test]
fn mcont006_procedure_validation_rejects_nested_drift_noncanonical_bytes_and_bad_digest() {
    let fixture = fixture();
    let contract = fixture.result_fixtures.get("procedure_validation").unwrap();
    let valid = materialize(contract);
    assert_schema_valid(&contract.schema_file, &valid);
    validate_command_result_v1("procedure.validate", &result_map(valid.clone())).unwrap();

    let mut nested_drift = valid.clone();
    nested_drift["procedure"]["stages"][0]["future"] = Value::Bool(true);
    refresh_procedure_canonical_fields(&mut nested_drift);
    assert_schema_invalid(&contract.schema_file, &nested_drift);
    assert!(validate_command_result_v1("procedure.validate", &result_map(nested_drift)).is_err());

    let mut noncanonical = valid.clone();
    let noncanonical_json = format!(" {}", noncanonical["canonical_json"].as_str().unwrap());
    noncanonical["canonical_json"] = Value::String(noncanonical_json.clone());
    noncanonical["digest"] = Value::String(format!(
        "sha256:{:x}",
        Sha256::digest(noncanonical_json.as_bytes())
    ));
    assert_schema_valid(&contract.schema_file, &noncanonical);
    assert!(validate_command_result_v1("procedure.validate", &result_map(noncanonical)).is_err());

    let mut bad_digest = valid.clone();
    bad_digest["digest"] = Value::String(format!("sha256:{}", "0".repeat(64)));
    assert_schema_valid(&contract.schema_file, &bad_digest);
    assert!(validate_command_result_v1("procedure.validate", &result_map(bad_digest)).is_err());

    let mut mismatched_document = valid;
    mismatched_document["procedure"]["name"] = Value::String("Different".to_owned());
    assert_schema_valid(&contract.schema_file, &mismatched_document);
    assert!(
        validate_command_result_v1("procedure.validate", &result_map(mismatched_document)).is_err()
    );
}

#[test]
fn mcont006_runtime_rejects_all_closed_result_boundary_drift() {
    let fixture = fixture();
    let mut cases = Vec::new();

    let mut version = materialize(fixture.result_fixtures.get("version").unwrap());
    version["supported_ipc_ids"] = json!([]);
    cases.push(("version", "version", version));

    for pointer in [
        "/pid",
        "/executable_path",
        "/socket_path",
        "/configured_socket_path",
        "/effective_socket_path",
    ] {
        let mut daemon = materialize(fixture.result_fixtures.get("daemon_status").unwrap());
        *daemon.pointer_mut(pointer).unwrap() = if pointer == "/pid" {
            json!(0)
        } else {
            json!("relative/path")
        };
        cases.push(("daemon_status", "daemon.status", daemon));
    }

    for (pointer, value) in [
        ("/product", json!("another-product")),
        ("/build_identity", json!("not-a-digest")),
        (
            "/contract_manifest_schema",
            json!("podway.contract-manifest/future"),
        ),
        ("/protocol_versions", json!([])),
    ] {
        let mut daemon = materialize(fixture.result_fixtures.get("daemon_direct_status").unwrap());
        *daemon.pointer_mut(pointer).unwrap() = value;
        cases.push(("daemon_direct_status", "daemon.status", daemon));
    }

    for pointer in [
        "/product",
        "/daemon_version",
        "/target",
        "/build_identity",
        "/contract_manifest_schema",
        "/contract_manifest_digest",
        "/executable_path",
        "/socket_path",
        "/configured_socket_path",
    ] {
        let mut daemon = materialize(fixture.result_fixtures.get("daemon_status").unwrap());
        *daemon.pointer_mut(pointer).unwrap() = Value::Null;
        cases.push(("daemon_status", "daemon.status", daemon));
    }

    for (pointer, value) in [
        ("/product", json!("another-product")),
        ("/build_identity", json!("not-a-digest")),
        (
            "/contract_manifest_schema",
            json!("podway.contract-manifest/future"),
        ),
    ] {
        let mut daemon = materialize(fixture.result_fixtures.get("daemon_status").unwrap());
        *daemon.pointer_mut(pointer).unwrap() = value;
        cases.push(("daemon_status", "daemon.status", daemon));
    }

    let mut unreachable = materialize(fixture.result_fixtures.get("daemon_status").unwrap());
    unreachable["reachable"] = Value::Bool(false);
    unreachable["protocol_versions"] = json!([]);
    for pointer in [
        "/product",
        "/daemon_version",
        "/target",
        "/build_identity",
        "/source_commit",
        "/contract_manifest_schema",
        "/contract_manifest_digest",
        "/pid",
        "/process_id",
        "/executable_path",
        "/started_at",
        "/uptime_ms",
        "/socket_path",
        "/configured_socket_path",
        "/effective_socket_path",
    ] {
        *unreachable.pointer_mut(pointer).unwrap() = Value::Null;
    }
    assert_schema_valid("schemas/daemon-status-result-v1.schema.json", &unreachable);
    validate_command_result_v1("daemon.status", &result_map(unreachable)).unwrap();

    let mut start = materialize(fixture.result_fixtures.get("session_start").unwrap());
    start["revision_after"] = json!(0);
    cases.push(("session_start", "session.start", start));

    for pointer in ["/task", "/source/preset", "/first_stage/title"] {
        let mut dry_run = materialize(
            fixture
                .result_fixtures
                .get("session_start_dry_run")
                .unwrap(),
        );
        *dry_run.pointer_mut(pointer).unwrap() = json!("");
        cases.push(("session_start_dry_run", "session.start", dry_run));
    }

    let mut reset = materialize(fixture.result_fixtures.get("reset_transition").unwrap());
    reset["revision"] = json!(0);
    cases.push(("reset_transition", "session.reset", reset));

    let mut status = materialize(fixture.result_fixtures.get("status").unwrap());
    status["blockers"] = json!([{
        "id": "00000000-0000-4000-8000-000000000801",
        "attempt_id": "6f8e7dc4-6502-4857-9d38-1a4afedb50e4",
        "reason": ""
    }]);
    cases.push(("status", "session.status", status));

    for pointer in ["/items/0/value/location", "/items/0/value/media_type"] {
        let mut status = materialize(fixture.result_fixtures.get("status").unwrap());
        status["items"] = json!([{
            "id": "artifact", "type": "artifact", "prompt": "Artifact",
            "required": true, "satisfied": true, "revision": 1,
            "value": {
                "location_type": "path", "location": "artifact.txt",
                "sha256_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "size_bytes": 1, "media_type": "text/plain"
            }
        }]);
        *status.pointer_mut(pointer).unwrap() = json!("");
        cases.push(("status", "session.status", status));
    }

    for pointer in ["/stage/title", "/next_stage_after_completion/title"] {
        let mut next = materialize(fixture.result_fixtures.get("next").unwrap());
        *next.pointer_mut(pointer).unwrap() = json!("");
        cases.push(("next", "session.next", next));
    }
    let mut next = materialize(fixture.result_fixtures.get("next").unwrap());
    next["blockers"] = json!([{
        "id": "00000000-0000-4000-8000-000000000802",
        "attempt_id": "6f8e7dc4-6502-4857-9d38-1a4afedb50e4",
        "reason": ""
    }]);
    cases.push(("next", "session.next", next));

    let mut lookup = materialize(fixture.result_fixtures.get("job_lookup_found").unwrap());
    lookup["job"]["command"] = json!("");
    cases.push(("job_lookup_found", "job.lookup", lookup));

    for (fixture_name, command, value) in cases {
        assert_result_invalid(
            fixture.result_fixtures.get(fixture_name).unwrap(),
            command,
            value,
        );
    }
}

#[test]
fn mcont006_stage_preview_requires_present_nullable_affected_stage_fields() {
    let fixture = fixture();
    let contract = fixture.result_fixtures.get("stage_preview").unwrap();
    let valid = materialize(contract);
    assert_schema_valid(&contract.schema_file, &valid);
    validate_command_result_v1("session.return", &result_map(valid.clone())).unwrap();

    for field in ["before", "after", "before_attempt", "after_attempt"] {
        let mut missing = valid.clone();
        missing["affected_stages"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert_result_invalid(contract, "session.return", missing);
    }
}

#[test]
fn mcont006_error_fixtures_lock_catalog_schemas_and_runtime_decoders() {
    let fixture = fixture();
    let catalog = parse_error_catalog();
    let observed = fixture
        .error_bindings
        .iter()
        .map(|binding| {
            let schema = fixture
                .error_fixtures
                .get(&binding.fixture)
                .unwrap()
                .schema
                .clone();
            (binding.code.clone(), schema)
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(observed.len(), fixture.error_bindings.len());
    assert_eq!(
        observed,
        catalog
            .iter()
            .map(|(code, entry)| (code.clone(), entry.details_schema.clone().unwrap()))
            .collect()
    );
    assert_eq!(
        fixture
            .error_bindings
            .iter()
            .map(|binding| binding.fixture.as_str())
            .collect::<BTreeSet<_>>(),
        fixture.error_fixtures.keys().map(String::as_str).collect()
    );

    for binding in &fixture.error_bindings {
        let contract = fixture.error_fixtures.get(&binding.fixture).unwrap();
        assert_eq!(contract.details["schema"], contract.schema);
        assert!(contains_const(
            &read_json(&contract.schema_file),
            &contract.schema
        ));
        let entry = catalog.get(&binding.code).unwrap();
        let envelope = error_envelope(&binding.code, entry, contract.details.clone());
        assert_schema_valid("schemas/error-v1.schema.json", &envelope);
        serde_json::from_value::<ResponseEnvelopeV1>(envelope).unwrap();
        for details in mutations(
            &contract.details,
            &contract.schema,
            &contract.malformed_type_pointer,
        ) {
            let mutation = error_envelope(&binding.code, entry, details);
            assert_schema_invalid("schemas/error-v1.schema.json", &mutation);
            assert!(serde_json::from_value::<ResponseEnvelopeV1>(mutation).is_err());
        }
    }
}

#[test]
fn mcont006_envelopes_are_additive_while_conflict_details_stay_coupled() {
    let mut output = read_json("docs/examples/json/output-complete.json");
    output["future_envelope_field"] = json!({"enabled": true});
    for field in ["workspace", "job", "session"] {
        output[field]["future_field"] = json!(true);
    }
    assert_schema_valid("schemas/output-v1.schema.json", &output);
    serde_json::from_value::<ResponseEnvelopeV1>(output).unwrap();

    let catalog = parse_error_catalog();
    let blocker_entry = catalog.get("BLOCKER_LIMIT_REACHED").unwrap();
    let mut error = error_envelope(
        "BLOCKER_LIMIT_REACHED",
        blocker_entry,
        json!({
            "schema": "podway.blocker-limit-details/v1",
            "maximum_open_blockers": 1024,
            "admission": {"admitted": false}
        }),
    );
    error["future_envelope_field"] = json!({"enabled": true});
    assert_schema_valid("schemas/error-v1.schema.json", &error);
    serde_json::from_value::<ResponseEnvelopeV1>(error).unwrap();

    let revision_entry = catalog.get("SESSION_REVISION_CONFLICT").unwrap();
    let uncoupled_revision = error_envelope(
        "SESSION_REVISION_CONFLICT",
        revision_entry,
        json!({
            "schema": "podway.revision-conflict-details/v1",
            "expected_revision": 4,
            "current_revision": 5,
            "admission": {
                "admitted": true,
                "job_id": "00000000-0000-4000-8000-000000000701",
                "workspace_sequence": 7
            }
        }),
    );
    assert_schema_invalid("schemas/error-v1.schema.json", &uncoupled_revision);
    assert!(serde_json::from_value::<ResponseEnvelopeV1>(uncoupled_revision).is_err());

    let attempt_entry = catalog.get("ATTEMPT_NOT_CURRENT").unwrap();
    let uncoupled_attempt = error_envelope(
        "ATTEMPT_NOT_CURRENT",
        attempt_entry,
        json!({
            "schema": "podway.attempt-conflict-details/v1",
            "expected_attempt_id": "00000000-0000-4000-8000-000000000702",
            "job_id": "00000000-0000-4000-8000-000000000701",
            "job_sequence": 7
        }),
    );
    assert_schema_invalid("schemas/error-v1.schema.json", &uncoupled_attempt);
    assert!(serde_json::from_value::<ResponseEnvelopeV1>(uncoupled_attempt).is_err());

    let mismatched_revision = error_envelope(
        "SESSION_REVISION_CONFLICT",
        revision_entry,
        json!({
            "schema": "podway.revision-conflict-details/v1",
            "expected_revision": 4,
            "current_revision": 5,
            "job_id": "00000000-0000-4000-8000-000000000701",
            "job_sequence": 8,
            "admission": {
                "admitted": true,
                "job_id": "00000000-0000-4000-8000-000000000701",
                "workspace_sequence": 7
            }
        }),
    );
    assert_schema_valid("schemas/error-v1.schema.json", &mismatched_revision);
    assert!(serde_json::from_value::<ResponseEnvelopeV1>(mismatched_revision).is_err());
}

fn contains_key(value: &Value, expected: &str) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key(expected)
                || object.values().any(|value| contains_key(value, expected))
        }
        Value::Array(values) => values.iter().any(|value| contains_key(value, expected)),
        _ => false,
    }
}

#[test]
fn mcont006_compact_known_answer_is_closed_and_within_exact_envelope_limit() {
    let compact = read_json("docs/examples/json/compact-status-output.json");
    let response = serde_json::from_value::<ResponseEnvelopeV1>(compact.clone()).unwrap();
    assert!(serde_json::to_vec(&response).unwrap().len() <= MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1);
    for forbidden in [
        "instructions",
        "prompt",
        "title",
        "value",
        "reason",
        "previous_attempts",
    ] {
        assert!(!contains_key(&compact["result"], forbidden));
    }

    let mut maximum = compact;
    maximum["result"]["items"] = Value::Array(
        (0..MAX_STAGE_ITEMS)
            .map(|index| {
                json!({
                    "id": format!("item-{index:058}"), "type": "confirm", "required": true,
                    "satisfied": false, "revision": 0
                })
            })
            .collect(),
    );
    maximum["result"]["blockers"] = Value::Array(
        (0..MAX_OPEN_BLOCKERS_PER_ATTEMPT_V1)
            .map(|index| {
                json!({
                    "id": format!("00000000-0000-4000-8000-{index:012x}"),
                    "attempt_id": "6f8e7dc4-6502-4857-9d38-1a4afedb50e4", "state": "open"
                })
            })
            .collect(),
    );
    assert_schema_valid("schemas/output-v1.schema.json", &maximum);
    let response = serde_json::from_value::<ResponseEnvelopeV1>(maximum.clone()).unwrap();
    assert!(serde_json::to_vec(&response).unwrap().len() < MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1);

    maximum["result"]["blockers"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id": "00000000-0000-4000-8000-000000000400",
            "attempt_id": "6f8e7dc4-6502-4857-9d38-1a4afedb50e4", "state": "open"
        }));
    assert_schema_invalid("schemas/output-v1.schema.json", &maximum);
    assert!(serde_json::from_value::<ResponseEnvelopeV1>(maximum).is_err());
}

#[test]
fn mcont006_compact_limits_track_domain_limits_and_lifecycle_invariants() {
    let compact_schema = read_json("schemas/compact-status-result-v1.schema.json");
    let procedure_schema = read_json("schemas/procedure-v1.schema.json");
    assert_eq!(
        compact_schema["properties"]["items"]["maxItems"].as_u64(),
        u64::try_from(MAX_STAGE_ITEMS).ok()
    );
    assert_eq!(
        compact_schema["properties"]["blockers"]["maxItems"].as_u64(),
        u64::try_from(MAX_OPEN_BLOCKERS_PER_ATTEMPT_V1).ok()
    );
    assert_eq!(
        procedure_schema["$defs"]["stage"]["properties"]["items"]["maxItems"].as_u64(),
        u64::try_from(MAX_STAGE_ITEMS).ok()
    );

    let fixture = fixture();
    let contract = fixture.result_fixtures.get("compact_status").unwrap();
    let compact = materialize(contract);
    for mutation in [
        {
            let mut value = compact.clone();
            value["current"] = Value::Null;
            value
        },
        {
            let mut value = compact.clone();
            value["session"]["lifecycle"] = json!("completed");
            value
        },
        {
            let mut value = compact;
            value["session"]["lifecycle"] = json!("cancelled");
            value["current"] = Value::Null;
            value["blockers"] = json!([]);
            value
        },
    ] {
        assert_schema_invalid(&contract.schema_file, &mutation);
        assert!(validate_command_result_v1("session.status", &result_map(mutation)).is_err());
    }
}

#[test]
fn mcont006_published_envelope_and_registry_known_answers_validate() {
    let request = read_json("docs/examples/json/ipc-complete-request.json");
    assert_schema_valid("schemas/ipc-request-v1.schema.json", &request);
    serde_json::from_value::<RequestEnvelopeV1>(request).unwrap();

    for relative in [
        "docs/examples/json/output-complete.json",
        "docs/examples/json/error-required-items.json",
    ] {
        let response = read_json(relative);
        let schema = if response["schema"] == "podway.output/v1" {
            "schemas/output-v1.schema.json"
        } else {
            "schemas/error-v1.schema.json"
        };
        assert_schema_valid(schema, &response);
        serde_json::from_value::<ResponseEnvelopeV1>(response).unwrap();
    }

    assert_schema_valid(
        "schemas/registry-v1.schema.json",
        &read_json("docs/examples/json/registry.json"),
    );
}

fn sha256(path: &Path) -> String {
    format!("sha256:{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn discover_json_files(relative: &str) -> BTreeSet<String> {
    fn visit(repository: &Path, directory: &Path, found: &mut BTreeSet<String>) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(
                !metadata.file_type().is_symlink(),
                "known-answer symlink: {}",
                path.display()
            );
            if metadata.is_dir() {
                visit(repository, &path, found);
            } else {
                assert!(
                    metadata.is_file(),
                    "non-regular known-answer entry: {}",
                    path.display()
                );
                if path.extension().and_then(|value| value.to_str()) == Some("json") {
                    let relative = path.strip_prefix(repository).unwrap();
                    let normalized = relative
                        .components()
                        .map(|part| {
                            part.as_os_str()
                                .to_str()
                                .expect("known-answer path must be UTF-8")
                        })
                        .collect::<Vec<_>>()
                        .join("/");
                    assert!(found.insert(normalized));
                }
            }
        }
    }

    let repository = root();
    let mut directory = repository.clone();
    for component in Path::new(relative).components() {
        directory.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&directory).unwrap();
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    }
    let mut found = BTreeSet::new();
    visit(&repository, &directory, &mut found);
    found
}

fn discover_regular_files(relative: &str) -> BTreeSet<String> {
    fn visit(repository: &Path, directory: &Path, found: &mut BTreeSet<String>) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(
                !metadata.file_type().is_symlink(),
                "fixture symlink: {}",
                path.display()
            );
            if metadata.is_dir() {
                visit(repository, &path, found);
            } else {
                assert!(
                    metadata.is_file(),
                    "non-regular fixture entry: {}",
                    path.display()
                );
                let relative = path.strip_prefix(repository).unwrap();
                let normalized = relative
                    .components()
                    .map(|part| {
                        part.as_os_str()
                            .to_str()
                            .expect("fixture path must be UTF-8")
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                assert!(found.insert(normalized));
            }
        }
    }

    let repository = root();
    let directory = repository.join(relative);
    let metadata = fs::symlink_metadata(&directory).unwrap();
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    let mut found = BTreeSet::new();
    visit(&repository, &directory, &mut found);
    found
}

#[test]
fn mcont006_manifest_recursively_covers_known_answers_with_exact_digests() {
    let manifest: Manifest =
        serde_json::from_value(read_json("contracts/contract-manifest-v1.json")).unwrap();
    let mut assets = BTreeMap::new();
    for asset in manifest
        .assets
        .into_iter()
        .filter(|asset| asset.kind == "known_answer_fixture")
    {
        assert!(assets.insert(asset.path, asset.sha256).is_none());
    }
    let expected = discover_json_files("docs/examples/json")
        .into_iter()
        .chain(discover_json_files("tests/fixtures/contract"))
        .chain(discover_regular_files("tests/fixtures/v2"))
        .collect::<BTreeSet<_>>();
    assert_eq!(assets.keys().cloned().collect::<BTreeSet<_>>(), expected);
    for (relative, digest) in assets {
        assert_eq!(digest, sha256(&root().join(relative)));
    }
}

fn collect_expected_codes(value: &Value, codes: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(code) = object.get("expected_code").and_then(Value::as_str) {
                codes.insert(code.to_owned());
            }
            object
                .values()
                .for_each(|value| collect_expected_codes(value, codes));
        }
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_expected_codes(value, codes)),
        _ => {}
    }
}

fn boundary_pairs(value: &Value, field: &str) -> BTreeMap<String, (u64, u64)> {
    value[field]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["dimension"].as_str().unwrap().to_owned(),
                (
                    entry["at_limit"].as_u64().unwrap(),
                    entry["one_over"].as_u64().unwrap(),
                ),
            )
        })
        .collect()
}

fn recipe_case_ids(value: &Value) -> BTreeSet<String> {
    value["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap().to_owned())
        .collect()
}

fn recipe_expected_codes(value: &Value) -> BTreeMap<String, String> {
    value["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| {
            entry["expected_code"]
                .as_str()
                .map(|code| (entry["id"].as_str().unwrap().to_owned(), code.to_owned()))
        })
        .collect()
}

fn validate_v2_fixture_catalog(value: &Value) -> Result<(usize, usize), String> {
    let catalog: V2FixtureCatalog =
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    if catalog.schema != "podway.v2-fixture-catalog/v1"
        || catalog.version != 1
        || catalog.fixture_root != "tests/fixtures/v2"
        || catalog.source_acceptance_matrix != "quality/v2-acceptance-matrix-v1.json"
        || catalog.source_compatibility_matrix != "quality/v2-compatibility-matrix-v1.json"
        || catalog.source_payload_matrix != "quality/v2-payload-matrix-v1.json"
        || catalog.evidence_policy
            != "Every entry is manifest-bound with planned runtime ownership. The YAML/JSON pair and result-family values are validated now only as schema known answers; no entry proves future parser, graph, runtime, payload, compatibility, admission, or release behavior."
    {
        return Err("v2 fixture catalog identity or evidence policy drift".to_owned());
    }

    let physical_paths = discover_regular_files(&catalog.fixture_root);
    let mut fixture_ids = BTreeSet::new();
    let mut fixture_paths = BTreeSet::new();
    let mut fixture_media = BTreeMap::new();
    for fixture in &catalog.fixtures {
        if fixture.id.is_empty() || !fixture_ids.insert(fixture.id.clone()) {
            return Err("v2 fixture IDs must be unique and non-empty".to_owned());
        }
        let path = Path::new(&fixture.path);
        if path.is_absolute()
            || fixture.path.contains('\\')
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::CurDir
                        | std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
            || !fixture.path.starts_with("tests/fixtures/v2/")
            || !fixture_paths.insert(fixture.path.clone())
        {
            return Err("v2 fixture paths must be normalized, unique, and rooted".to_owned());
        }
        let source = root().join(path);
        let metadata = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("v2 fixture path must resolve to a regular non-symlink file".to_owned());
        }
        if fixture.sha256 != sha256(&source) {
            return Err(format!("v2 fixture digest drift: {}", fixture.id));
        }
        let expected_media = match path.extension().and_then(|value| value.to_str()) {
            Some("json") => "application/json",
            Some("yaml" | "yml") => "application/yaml",
            _ => "application/octet-stream",
        };
        if fixture.media_type != expected_media {
            return Err(format!("v2 fixture media type drift: {}", fixture.id));
        }
        fixture_media.insert(fixture.id.clone(), fixture.media_type.clone());
    }
    if fixture_paths != physical_paths {
        return Err("v2 fixture inventory does not exactly cover physical files".to_owned());
    }

    let acceptance = read_json(&catalog.source_acceptance_matrix);
    let mut expected_acceptance = BTreeSet::new();
    let mut acceptance_owners = BTreeMap::<String, BTreeSet<String>>::new();
    for criterion in acceptance["criteria"]
        .as_array()
        .ok_or("acceptance criteria missing")?
    {
        let section = criterion["section"]
            .as_str()
            .ok_or("acceptance section missing")?;
        if matches!(section, "17.1" | "17.2" | "17.7" | "17.9") {
            let id = criterion["id"]
                .as_str()
                .ok_or("acceptance ID missing")?
                .to_owned();
            expected_acceptance.insert(id.clone());
            acceptance_owners.insert(
                id,
                criterion["owning_tasks"]
                    .as_array()
                    .ok_or("acceptance owners missing")?
                    .iter()
                    .map(|owner| {
                        owner
                            .as_str()
                            .ok_or("acceptance owner malformed")
                            .map(str::to_owned)
                    })
                    .collect::<Result<_, _>>()?,
            );
        }
    }
    let compatibility = read_json(&catalog.source_compatibility_matrix);
    let payload = read_json(&catalog.source_payload_matrix);
    let specialized_entries = compatibility["requirements"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            compatibility["surface_cases"]
                .as_array()
                .into_iter()
                .flatten(),
        )
        .chain(payload["requirements"].as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    let mut specialized_metadata = BTreeMap::<String, (Option<String>, BTreeSet<String>)>::new();
    for entry in specialized_entries {
        let id = entry["id"]
            .as_str()
            .ok_or("specialized fixture ID missing")?
            .to_owned();
        let acceptance_id = entry
            .get("acceptance_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let owners = entry["owning_tasks"]
            .as_array()
            .ok_or("specialized owners missing")?
            .iter()
            .map(|owner| {
                owner
                    .as_str()
                    .ok_or("specialized owner malformed")
                    .map(str::to_owned)
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if specialized_metadata
            .insert(id, (acceptance_id, owners))
            .is_some()
        {
            return Err("specialized fixture ID is duplicated".to_owned());
        }
    }
    let expected_specialized = specialized_metadata
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut case_ids = BTreeSet::new();
    let mut used_fixtures = BTreeSet::new();
    let mut observed_acceptance = BTreeSet::new();
    let mut observed_specialized = BTreeSet::new();
    let mut classes = BTreeSet::new();
    for case in &catalog.cases {
        if case.id.is_empty() || !case_ids.insert(case.id.clone()) {
            return Err("v2 fixture case IDs must be unique and non-empty".to_owned());
        }
        if !matches!(
            case.fixture_class.as_str(),
            "known-answer" | "negative" | "graph" | "compatibility" | "maximum-size"
        ) || !classes.insert(case.fixture_class.clone()) && case.fixture_class != "maximum-size"
        {
            return Err("v2 fixture class is invalid or unexpectedly repeated".to_owned());
        }
        if case.evidence_level != "contract-recipe" || case.implementation_status != "planned" {
            return Err("v2 fixture case falsely promotes planned evidence".to_owned());
        }
        if case.fixture_ids.is_empty() || case.acceptance_ids.is_empty() {
            return Err("v2 fixture case mapping must be non-empty".to_owned());
        }
        for fixture_id in &case.fixture_ids {
            if !fixture_ids.contains(fixture_id) || !used_fixtures.insert(fixture_id.clone()) {
                return Err("v2 fixture must be referenced by exactly one case".to_owned());
            }
        }
        let mut expected_owners = BTreeSet::new();
        for acceptance_id in &case.acceptance_ids {
            if !expected_acceptance.contains(acceptance_id)
                || !observed_acceptance.insert(acceptance_id.clone())
            {
                return Err("v2 fixture acceptance mapping is unknown or duplicated".to_owned());
            }
            expected_owners.extend(acceptance_owners[acceptance_id].iter().cloned());
        }
        for specialized_id in &case.specialized_ids {
            if !expected_specialized.contains(specialized_id)
                || !observed_specialized.insert(specialized_id.clone())
            {
                return Err("v2 fixture specialized mapping is unknown or duplicated".to_owned());
            }
            let (acceptance_id, owners) = &specialized_metadata[specialized_id];
            if acceptance_id
                .as_ref()
                .is_some_and(|acceptance_id| !case.acceptance_ids.contains(acceptance_id))
            {
                return Err("v2 fixture specialized mapping crosses acceptance cases".to_owned());
            }
            expected_owners.extend(owners.iter().cloned());
        }
        if case.owning_tasks.iter().cloned().collect::<BTreeSet<_>>() != expected_owners
            || case.owning_tasks.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err("v2 fixture owning task mapping drift".to_owned());
        }
    }
    if used_fixtures != fixture_ids
        || observed_acceptance != expected_acceptance
        || observed_specialized != expected_specialized
    {
        return Err("v2 fixture mappings are not exhaustive".to_owned());
    }
    if classes
        != [
            "compatibility",
            "graph",
            "known-answer",
            "negative",
            "maximum-size",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    {
        return Err("v2 fixture class coverage drift".to_owned());
    }
    let observed_case_fixtures = catalog
        .cases
        .iter()
        .map(|case| (case.id.clone(), case.fixture_ids.clone()))
        .collect::<BTreeMap<_, _>>();
    let expected_case_fixtures = BTreeMap::from([
        (
            "V2FIX-PROCEDURE-KNOWN-ANSWER".to_owned(),
            vec![
                "procedure-equivalence-contract".to_owned(),
                "procedure-equivalent-json".to_owned(),
                "procedure-equivalent-yaml".to_owned(),
            ],
        ),
        (
            "V2FIX-PROCEDURE-NEGATIVE".to_owned(),
            vec!["procedure-malformed-inputs".to_owned()],
        ),
        (
            "V2FIX-GRAPH".to_owned(),
            vec![
                "graph-negative-cases".to_owned(),
                "graph-valid-cases".to_owned(),
            ],
        ),
        (
            "V2FIX-COMPATIBILITY".to_owned(),
            vec![
                "compatibility-admission-fence".to_owned(),
                "compatibility-unsupported-peer".to_owned(),
                "compatibility-v1-boundaries".to_owned(),
                "protocol-result-families".to_owned(),
            ],
        ),
        (
            "V2FIX-PAYLOAD-NEXT".to_owned(),
            vec!["payload-maximum-next".to_owned()],
        ),
        (
            "V2FIX-PAYLOAD-STATUS".to_owned(),
            vec!["payload-maximum-status".to_owned()],
        ),
        (
            "V2FIX-PAYLOAD-TRUNCATION".to_owned(),
            vec!["payload-truncation-overflow".to_owned()],
        ),
    ]);
    if observed_case_fixtures != expected_case_fixtures {
        return Err("v2 fixture case-to-file mapping drift".to_owned());
    }

    let authoring_catalog = read_json("assets/specifications/authoring-diagnostics.json");
    let runtime_catalog = read_json("assets/specifications/error-codes.json");
    let authoring_codes = authoring_catalog["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["code"].as_str().unwrap().to_owned());
    let runtime_codes = runtime_catalog["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["code"].as_str().unwrap().to_owned());
    let registered_codes = authoring_codes
        .chain(runtime_codes)
        .collect::<BTreeSet<_>>();
    for fixture in &catalog.fixtures {
        if fixture_media[&fixture.id] != "application/json" {
            continue;
        }
        let document = read_json(&fixture.path);
        let mut declared_codes = BTreeSet::new();
        collect_expected_codes(&document, &mut declared_codes);
        if !declared_codes.is_subset(&registered_codes) {
            return Err(format!(
                "v2 fixture declares an unregistered code: {}",
                fixture.id
            ));
        }
        if document.get("schema").and_then(Value::as_str) == Some("podway.v2-fixture-recipe/v1")
            && (document.get("evidence_level").and_then(Value::as_str) != Some("contract-recipe")
                || document
                    .get("implementation_status")
                    .and_then(Value::as_str)
                    != Some("planned"))
        {
            return Err(format!("v2 fixture recipe evidence drift: {}", fixture.id));
        }
    }

    Ok((catalog.fixtures.len(), catalog.cases.len()))
}

#[test]
fn v2ctr006_fixture_catalog_is_exact_bounded_and_not_runtime_evidence() {
    let catalog = read_json("tests/fixtures/contract/v2-fixture-catalog-v1.json");
    assert_eq!(validate_v2_fixture_catalog(&catalog).unwrap(), (13, 7));
    let catalog_cases = catalog["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| (case["id"].as_str().unwrap(), case))
        .collect::<BTreeMap<_, _>>();
    let numbered_ids = |prefix: &str, start: u8, end: u8| {
        (start..=end)
            .map(|number| format!("{prefix}-{number:03}"))
            .collect::<Vec<_>>()
    };
    for (case_id, expected) in [
        (
            "V2FIX-PROCEDURE-KNOWN-ANSWER",
            vec!["V2ACC-001", "V2ACC-003", "V2ACC-004", "V2ACC-006"],
        ),
        ("V2FIX-PROCEDURE-NEGATIVE", vec!["V2ACC-002", "V2ACC-005"]),
    ] {
        assert_eq!(catalog_cases[case_id]["acceptance_ids"], json!(expected));
    }
    assert_eq!(
        catalog_cases["V2FIX-GRAPH"]["acceptance_ids"],
        json!(numbered_ids("V2ACC", 7, 20))
    );
    assert_eq!(
        catalog_cases["V2FIX-COMPATIBILITY"]["acceptance_ids"],
        json!(numbered_ids("V2ACC", 66, 72))
    );
    let mut expected_compatibility = numbered_ids("V2COMP", 1, 7);
    expected_compatibility.extend(numbered_ids("V2COMP-SURFACE", 1, 8));
    assert_eq!(
        catalog_cases["V2FIX-COMPATIBILITY"]["specialized_ids"],
        json!(expected_compatibility)
    );
    assert_eq!(
        catalog_cases["V2FIX-PAYLOAD-NEXT"]["acceptance_ids"],
        json!(numbered_ids("V2ACC", 77, 79))
    );
    assert_eq!(
        catalog_cases["V2FIX-PAYLOAD-STATUS"]["acceptance_ids"],
        json!(numbered_ids("V2ACC", 80, 84))
    );
    assert_eq!(
        catalog_cases["V2FIX-PAYLOAD-TRUNCATION"]["acceptance_ids"],
        json!(numbered_ids("V2ACC", 85, 87))
    );
    assert_eq!(
        catalog_cases["V2FIX-PAYLOAD-NEXT"]["specialized_ids"],
        json!(numbered_ids("V2PAY", 1, 3))
    );
    assert_eq!(
        catalog_cases["V2FIX-PAYLOAD-STATUS"]["specialized_ids"],
        json!(numbered_ids("V2PAY", 4, 8))
    );
    assert_eq!(
        catalog_cases["V2FIX-PAYLOAD-TRUNCATION"]["specialized_ids"],
        json!(numbered_ids("V2PAY", 9, 11))
    );

    let json_document = read_json("tests/fixtures/v2/procedures/equivalent-procedure.json");
    let yaml_document: Value = serde_yaml::from_slice(
        &fs::read(root().join("tests/fixtures/v2/procedures/equivalent-procedure.yaml")).unwrap(),
    )
    .unwrap();
    assert_eq!(yaml_document, json_document);
    assert_schema_valid("schemas/procedure-v2.schema.json", &json_document);
    let equivalence_contract = read_json("tests/fixtures/v2/procedures/equivalence-contract.json");
    let canonical = podway_core::canonicalize_json_v1(&json_document).unwrap();
    assert_eq!(
        equivalence_contract["canonical_sha256"],
        json!(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
    );
    assert_eq!(
        equivalence_contract["future_assertions"],
        json!([
            "YAML and JSON parse to the same structural value and canonical bytes",
            "field ordering and non-semantic formatting preserve canonical_sha256",
            "podway.procedure/v1 dispatch remains v1 and podway.procedure/v2 dispatch remains v2",
            "purpose objective evidence_guidance evidence_from skip and assessment mappings retain closed canonical forms"
        ])
    );

    let malformed = read_json("tests/fixtures/v2/procedures/malformed-inputs.json");
    let expected_authoring_bounds = [
        ("identifier characters", 64, 65),
        ("procedure version characters", 64, 65),
        ("procedure name characters", 120, 121),
        ("procedure purpose characters", 500, 501),
        ("procedure description characters", 1000, 1001),
        ("source document bytes", 1_048_576, 1_048_577),
        ("source nesting depth", 64, 65),
        ("parsed nodes", 100_000, 100_001),
        ("graph nodes", 64, 65),
        ("node definitions", 64, 65),
        ("definition title characters", 120, 121),
        ("definition intent characters", 300, 301),
        ("definition description characters", 1000, 1001),
        ("decision objective characters", 300, 301),
        ("decision prompt characters", 500, 501),
        ("reason-policy prompt characters", 300, 301),
        ("instructions per definition", 16, 17),
        ("instruction characters", 1000, 1001),
        ("items per definition", 64, 65),
        ("item prompt characters", 300, 301),
        ("item help characters", 1000, 1001),
        ("text item max_length", 16_384, 16_385),
        ("list entries", 100, 101),
        ("list entry characters", 1000, 1001),
        ("choice count", 32, 33),
        ("choice characters", 120, 121),
        ("evidence_from entries", 8, 9),
        ("selected evidence items", 16, 17),
        ("decision options", 8, 9),
        ("option label characters", 120, 121),
        ("option criteria characters", 500, 501),
        ("evidence guidance entries", 8, 9),
        ("evidence guidance characters", 200, 201),
    ]
    .into_iter()
    .map(|(name, at_limit, one_over)| (name.to_owned(), (at_limit, one_over)))
    .collect::<BTreeMap<_, _>>();
    assert_eq!(
        boundary_pairs(&malformed, "boundary_cases"),
        expected_authoring_bounds
    );
    assert_eq!(
        recipe_case_ids(&malformed),
        [
            "unknown-field",
            "duplicate-yaml-key",
            "duplicate-json-key",
            "yaml-alias",
            "yaml-tag",
            "include",
            "multiple-yaml-documents",
            "trailing-json",
            "malformed-json",
            "invalid-utf8",
            "goal-tracking-false",
            "goal-tracking-string",
            "goal-tracking-list",
            "goal-tracking-object",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    assert_eq!(
        recipe_expected_codes(&malformed),
        [
            ("unknown-field", "AUTHORING_SCHEMA_INVALID"),
            ("duplicate-yaml-key", "SOURCE_CONSTRUCT_UNSUPPORTED"),
            ("duplicate-json-key", "SOURCE_CONSTRUCT_UNSUPPORTED"),
            ("yaml-alias", "SOURCE_CONSTRUCT_UNSUPPORTED"),
            ("yaml-tag", "SOURCE_CONSTRUCT_UNSUPPORTED"),
            ("include", "AUTHORING_SCHEMA_INVALID"),
            ("multiple-yaml-documents", "SOURCE_CONSTRUCT_UNSUPPORTED"),
            ("trailing-json", "SOURCE_CONSTRUCT_UNSUPPORTED"),
            ("malformed-json", "SOURCE_CONSTRUCT_UNSUPPORTED"),
            ("invalid-utf8", "SOURCE_CONSTRUCT_UNSUPPORTED"),
            ("goal-tracking-false", "AUTHORING_SCHEMA_INVALID"),
            ("goal-tracking-string", "AUTHORING_SCHEMA_INVALID"),
            ("goal-tracking-list", "AUTHORING_SCHEMA_INVALID"),
            ("goal-tracking-object", "AUTHORING_SCHEMA_INVALID"),
        ]
        .into_iter()
        .map(|(id, code)| (id.to_owned(), code.to_owned()))
        .collect()
    );

    let graph_valid = read_json("tests/fixtures/v2/graphs/valid-cases.json");
    assert_eq!(
        recipe_case_ids(&graph_valid),
        [
            "linear-terminal",
            "unbounded-valid-rework-cycle",
            "maximum-reachable-readback",
            "dominance-preservation-property",
            "deterministic-diagnostics",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    let graph_negative = read_json("tests/fixtures/v2/graphs/negative-cases.json");
    assert_eq!(recipe_case_ids(&graph_negative).len(), 19);
    assert_eq!(
        recipe_expected_codes(&graph_negative),
        [
            (
                "duplicate-definition-source-key",
                "SOURCE_CONSTRUCT_UNSUPPORTED"
            ),
            ("ambiguous-placement-id", "AMBIGUOUS_GRAPH_REFERENCE"),
            ("unknown-entry", "ENTRY_NODE_INVALID"),
            ("no-terminal-route", "NO_TERMINAL_PATH"),
            ("unreachable-node", "UNREACHABLE_GRAPH_NODE"),
            ("option-without-route", "DECISION_OPTION_ROUTE_MISSING"),
            (
                "route-for-undefined-option",
                "DECISION_ROUTE_OPTION_UNDEFINED"
            ),
            ("invalid-cycle", "GRAPH_CYCLE_INVALID"),
            (
                "rework-target-not-dominating",
                "REWORK_TARGET_NOT_DOMINATING"
            ),
            ("unknown-evidence-source", "EVIDENCE_SOURCE_UNKNOWN"),
            ("self-evidence-source", "EVIDENCE_SOURCE_SELF_REFERENCE"),
            (
                "evidence-source-not-dominating",
                "EVIDENCE_SOURCE_DOES_NOT_DOMINATE_CONSUMER"
            ),
            ("skippable-required-source", "SKIPPABLE_EVIDENCE_SOURCE"),
            ("unknown-evidence-item", "EVIDENCE_SELECTOR_UNKNOWN_ITEM"),
            ("readback-over-budget", "READBACK_BUDGET_EXCEEDED"),
            (
                "goal-assessment-not-dominating",
                "GOAL_ASSESSMENT_NOT_DOMINATING_TERMINAL"
            ),
            (
                "assessment-option-unmapped",
                "GOAL_ASSESSMENT_OPTION_UNMAPPED"
            ),
            (
                "assessment-outcome-unreachable",
                "GOAL_ASSESSMENT_OUTCOME_UNREACHABLE"
            ),
            (
                "manual-rework-target-invalid",
                "MANUAL_REWORK_TARGET_UNKNOWN"
            ),
        ]
        .into_iter()
        .map(|(id, code)| (id.to_owned(), code.to_owned()))
        .collect()
    );
    for (path, expected) in [
        (
            "tests/fixtures/v2/compatibility/v1-boundaries.json",
            vec![
                "released-v1-fixtures",
                "v1-storage-migration",
                "v1-command-dispatch",
                "v1-reopen",
                "current-task-retention",
                "existing-route-v2-result-family",
                "new-route-v1-result-family",
                "v2-never-extends-v1-result-family",
                "manifest-digest-capability-discovery",
            ],
        ),
        (
            "tests/fixtures/v2/compatibility/unsupported-peer.json",
            vec![
                "v2-command-to-v1-peer",
                "registered-but-unserved",
                "absent-route",
            ],
        ),
        (
            "tests/fixtures/v2/compatibility/admission-fence.json",
            vec![
                "release-build",
                "development-unlock",
                "installed-daemon",
                "non-disposable-workspace",
            ],
        ),
    ] {
        assert_eq!(
            recipe_case_ids(&read_json(path)),
            expected.into_iter().map(str::to_owned).collect()
        );
    }

    let result_catalog = read_json("tests/fixtures/v2/protocol/result-families.json");
    assert_eq!(
        result_catalog["schema"],
        json!("podway.v2-result-known-answers/v1")
    );
    let result_fixtures = result_catalog["fixtures"].as_object().unwrap();
    let compatibility_matrix = read_json("quality/v2-compatibility-matrix-v1.json");
    let result_schema_paths =
        compatibility_matrix["result_family_inventories"]["existing_route_v2"]
            .as_array()
            .unwrap()
            .iter()
            .chain(
                compatibility_matrix["result_family_inventories"]["new_route_v1"]
                    .as_array()
                    .unwrap(),
            )
            .map(|path| path.as_str().unwrap())
            .collect::<Vec<_>>();
    let expected_result_ids = result_schema_paths
        .iter()
        .map(|path| {
            read_json(path)["properties"]["schema"]["const"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        result_fixtures.keys().cloned().collect::<BTreeSet<_>>(),
        expected_result_ids
    );
    for schema_path in result_schema_paths {
        let schema_id = read_json(schema_path)["properties"]["schema"]["const"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_schema_valid(
            &schema_path.replacen("assets/schemas/", "schemas/", 1),
            &result_fixtures[&schema_id],
        );
    }

    let maximum_next = read_json("tests/fixtures/v2/payload/maximum-next-recipe.json");
    let payload_matrix = read_json("quality/v2-payload-matrix-v1.json");
    assert_eq!(maximum_next["frame_bytes"], payload_matrix["frame_bytes"]);
    assert_eq!(
        maximum_next["charged_bytes"],
        payload_matrix["next_budget_bytes"]
    );
    assert_eq!(
        maximum_next["headroom_bytes"],
        payload_matrix["headroom_bytes"]
    );
    let matrix_components = payload_matrix["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["constant"].as_str().unwrap().to_owned(),
                entry["bytes"].clone(),
            )
        })
        .collect::<Map<_, _>>();
    assert_eq!(maximum_next["components"], Value::Object(matrix_components));
    assert_eq!(
        maximum_next["construction"],
        json!([
            "materialize every next-result-v2 field from the payload matrix exactly once",
            "fill procedure-static content to NEXT_STATIC_BUDGET",
            "fill read-back to READBACK_BUDGET",
            "include 16 criteria and their runtime suggestion argv",
            "include 64 blockers, 64 graph-node counters, and maximal warnings",
            "use escape-heavy control characters for the six-byte accounting factor"
        ])
    );

    let maximum_status = read_json("tests/fixtures/v2/payload/maximum-status-recipe.json");
    assert_eq!(
        maximum_status["projections"],
        json!([
            {"tier":"compact","maximum_bytes":262144,"includes":["counters","trace_length"],"excludes":["trace_entries","history","windows","readback_values","prompts","instructions","statements","suggestion_argv"]},
            {"tier":"standard","maximum_bytes":1048576,"status_values_max_bytes":262144,"item_value_max_characters":2048,"value_marker":"value_truncated","window_markers":["items_total","items_truncated"]},
            {"tier":"verbose","maximum_bytes":1048576,"trace_window_max_bytes":65536,"trace_window_count":6,"window_markers":["trace_truncated","trace_window"]}
        ])
    );
    assert_eq!(
        maximum_status["projections"][0]["excludes"],
        payload_matrix["status_contract"]["compact_exclusions"]
    );
    assert_eq!(
        maximum_status["blocker_window_max_bytes"],
        payload_matrix["status_contract"]["blocker_window_max_bytes"]
    );
    assert_eq!(
        maximum_status["blocker_window_order"],
        payload_matrix["status_contract"]["blocker_window_order"]
    );
    assert_eq!(
        maximum_status["blocker_window_markers"],
        payload_matrix["status_contract"]["blocker_window_markers"]
    );
    assert_eq!(
        maximum_status["verbose_history_shape"],
        json!({"entries":[],"trace_truncated":false,"trace_window":null})
    );

    let payload = read_json("tests/fixtures/v2/payload/truncation-and-overflow-recipe.json");
    let expected_domain_bounds = [
        ("goal statement characters", 1000, 1001),
        ("criterion identifier characters", 64, 65),
        ("criterion statement characters", 300, 301),
        ("goal revision reason characters", 1000, 1001),
        ("criterion assessment reason characters", 2000, 2001),
        ("criteria per goal revision", 16, 17),
        ("open blockers per attempt", 64, 65),
        ("blocker reason characters", 1000, 1001),
        (
            "decision retry rework or skip reason characters",
            2000,
            2001,
        ),
        ("actor attribution characters", 256, 257),
        ("citations per criterion assessment", 4, 5),
    ]
    .into_iter()
    .map(|(name, at_limit, one_over)| (name.to_owned(), (at_limit, one_over)))
    .collect::<BTreeMap<_, _>>();
    assert_eq!(
        boundary_pairs(&payload, "domain_boundary_cases"),
        expected_domain_bounds
    );
    assert_eq!(
        recipe_case_ids(&payload),
        [
            "item-value-boundary",
            "status-values-window",
            "blocker-window",
            "history-windows",
            "readback-is-next-only",
            "readback-item-bounds-and-selectors",
            "built-in-static-budget",
            "arbitrary-cycle-traversal",
            "integrity-classification",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );

    let mut sentinels = Vec::new();
    let mut extra = catalog.clone();
    let extra_fixture = extra["fixtures"][0].clone();
    extra["fixtures"]
        .as_array_mut()
        .unwrap()
        .push(extra_fixture);
    sentinels.push(extra);
    let mut missing = catalog.clone();
    missing["fixtures"].as_array_mut().unwrap().pop();
    sentinels.push(missing);
    let mut duplicate = catalog.clone();
    duplicate["fixtures"][1]["id"] = duplicate["fixtures"][0]["id"].clone();
    sentinels.push(duplicate);
    let mut digest = catalog.clone();
    digest["fixtures"][0]["sha256"] = json!(format!("sha256:{}", "0".repeat(64)));
    sentinels.push(digest);
    let mut traversal = catalog.clone();
    traversal["fixtures"][0]["path"] = json!("tests/fixtures/v2/../contract/fixture.json");
    sentinels.push(traversal);
    let mut acceptance = catalog.clone();
    acceptance["cases"][0]["acceptance_ids"][0] = json!("V2ACC-087");
    sentinels.push(acceptance);
    let mut owner = catalog.clone();
    owner["cases"][0]["owning_tasks"] = json!(["V2REL-006"]);
    sentinels.push(owner);
    let mut specialized_swap = catalog.clone();
    let compatibility_id = specialized_swap["cases"][3]["specialized_ids"][0].clone();
    let payload_id = specialized_swap["cases"][4]["specialized_ids"][0].clone();
    specialized_swap["cases"][3]["specialized_ids"][0] = payload_id;
    specialized_swap["cases"][4]["specialized_ids"][0] = compatibility_id;
    sentinels.push(specialized_swap);
    let mut fixture_swap = catalog.clone();
    let procedure_fixture = fixture_swap["cases"][0]["fixture_ids"][0].clone();
    let graph_fixture = fixture_swap["cases"][2]["fixture_ids"][0].clone();
    fixture_swap["cases"][0]["fixture_ids"][0] = graph_fixture;
    fixture_swap["cases"][2]["fixture_ids"][0] = procedure_fixture;
    sentinels.push(fixture_swap);
    let mut promotion = catalog;
    promotion["cases"][0]["implementation_status"] = json!("automated");
    sentinels.push(promotion);
    assert!(
        sentinels
            .iter()
            .all(|value| validate_v2_fixture_catalog(value).is_err())
    );
}

fn procedure_v2_fixture() -> Value {
    json!({
        "schema": "podway.procedure/v2",
        "id": "bounded-v2",
        "version": "2",
        "name": "Bounded v2 procedure",
        "purpose": "Exercise the complete structural authoring contract.",
        "description": "A schema known answer.",
        "goal_tracking": true,
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "Do the work",
                "intent": "Record bounded values.",
                "description": "An action definition.",
                "instructions": ["Work outside Podway."],
                "items": [
                    {"id":"confirm","type":"confirm","prompt":"Confirm.","help":"Confirmation guidance.","required":true},
                    {"id":"text","type":"text","prompt":"Text.","required":true,"max_length":16384},
                    {"id":"choice","type":"choice","prompt":"Choose.","required":false,"choices":["one","two"]},
                    {"id":"integer","type":"integer","prompt":"Number.","required":false,"minimum":-1,"maximum":1},
                    {"id":"list","type":"list","prompt":"List.","required":false,"max_items":100,"max_item_length":1000},
                    {"id":"artifact","type":"artifact","prompt":"Artifact.","required":false,"allowed_media_types":["application/json"]}
                ]
            },
            "choose": {
                "type": "decision",
                "title": "Choose an outcome",
                "description": "A decision definition.",
                "objective": "Select the supported outcome.",
                "prompt": "Which outcome is supported?",
                "evidence_guidance": ["Consult the recorded values."],
                "options": [
                    {"id":"achieved","label":"Achieved","criteria":"All criteria are satisfied."},
                    {"id":"not-achieved","label":"Not achieved"},
                    {"id":"superseded","label":"Superseded"}
                ],
                "reason": {"required":true,"prompt":"Explain the selection."},
                "assessment": {
                    "target":"session_goal",
                    "outcomes": {
                        "achieved":"achieved",
                        "not-achieved":"not_achieved",
                        "superseded":"superseded"
                    }
                }
            }
        },
        "graph": {
            "entry": "perform",
            "nodes": [
                {
                    "id":"perform",
                    "use":"work",
                    "skip":{"allowed":true,"reason_required":false},
                    "next":"decide"
                },
                {
                    "id":"decide",
                    "use":"choose",
                    "evidence_from":[{"node":"perform","required":false,"items":["text"]}],
                    "routes": {
                        "achieved":{"to":"finish","effect":"advance"},
                        "not-achieved":{"to":"finish","effect":"advance"},
                        "superseded":{"to":"finish","effect":"advance"}
                    }
                },
                {"id":"finish","use":"work","terminal":true}
            ]
        },
        "manual_rework": {"allowed_targets":["perform"]}
    })
}

#[test]
fn v2ctr002_procedure_v2_schema_accepts_complete_closed_authoring_shape() {
    assert_schema_valid("schemas/procedure-v2.schema.json", &procedure_v2_fixture());
}

#[test]
fn v2ctr002_procedure_v2_schema_accepts_exact_authoring_boundaries() {
    let mut maximum = procedure_v2_fixture();
    maximum["id"] = json!(format!("a{}", "a".repeat(63)));
    maximum["version"] = json!("v".repeat(64));
    maximum["name"] = json!("n".repeat(120));
    maximum["purpose"] = json!("p".repeat(500));
    maximum["description"] = json!("d".repeat(1000));

    maximum["node_definitions"]["work"]["title"] = json!("t".repeat(120));
    maximum["node_definitions"]["work"]["intent"] = json!("i".repeat(300));
    maximum["node_definitions"]["work"]["description"] = json!("d".repeat(1000));
    maximum["node_definitions"]["work"]["instructions"] = json!(vec!["i".repeat(1000); 16]);
    let mut maximum_items = maximum["node_definitions"]["work"]["items"]
        .as_array()
        .unwrap()
        .clone();
    for index in maximum_items.len()..64 {
        maximum_items.push(json!({
            "id":format!("item-{index}"),
            "type":"confirm",
            "prompt":"p".repeat(300),
            "help":"h".repeat(1000),
            "required":false
        }));
    }
    maximum["node_definitions"]["work"]["items"] = json!(maximum_items);
    maximum["node_definitions"]["work"]["items"][1]["max_length"] = json!(16384);
    maximum["node_definitions"]["work"]["items"][2]["choices"] = json!(
        (0..32)
            .map(|index| format!("{index:02}{}", "c".repeat(118)))
            .collect::<Vec<_>>()
    );
    maximum["node_definitions"]["work"]["items"][4]["max_items"] = json!(100);
    maximum["node_definitions"]["work"]["items"][4]["max_item_length"] = json!(1000);
    maximum["node_definitions"]["work"]["items"][5]["allowed_media_types"] = json!(
        (0..64)
            .map(|index| format!("application/x-{index}"))
            .collect::<Vec<_>>()
    );

    maximum["node_definitions"]["choose"]["title"] = json!("t".repeat(120));
    maximum["node_definitions"]["choose"]["description"] = json!("d".repeat(1000));
    maximum["node_definitions"]["choose"]["objective"] = json!("o".repeat(300));
    maximum["node_definitions"]["choose"]["prompt"] = json!("p".repeat(500));
    maximum["node_definitions"]["choose"]["evidence_guidance"] = json!(vec!["g".repeat(200); 8]);
    maximum["node_definitions"]["choose"]["reason"]["prompt"] = json!("r".repeat(300));
    maximum["node_definitions"]["choose"]["options"] = json!(
        (0..8)
            .map(|index| json!({
                "id":format!("option-{index}"),
                "label":"l".repeat(120),
                "criteria":"c".repeat(500)
            }))
            .collect::<Vec<_>>()
    );

    let mut definitions = maximum["node_definitions"].as_object().unwrap().clone();
    let action = definitions["work"].clone();
    for index in 0..62 {
        definitions.insert(format!("definition-{index}"), action.clone());
    }
    maximum["node_definitions"] = Value::Object(definitions);

    maximum["graph"]["nodes"][1]["evidence_from"] = json!(
        (0..8)
            .map(|index| json!({
                "node":format!("source-{index}"),
                "required":false,
                "items":(0..16).map(|item| format!("item-{item}")).collect::<Vec<_>>()
            }))
            .collect::<Vec<_>>()
    );
    let terminal = maximum["graph"]["nodes"][2].clone();
    let mut nodes = maximum["graph"]["nodes"].as_array().unwrap().clone();
    for index in nodes.len()..64 {
        let mut node = terminal.clone();
        node["id"] = json!(format!("terminal-{index}"));
        nodes.push(node);
    }
    maximum["graph"]["nodes"] = json!(nodes);
    maximum["manual_rework"]["allowed_targets"] = json!(
        (0..64)
            .map(|index| format!("target-{index}"))
            .collect::<Vec<_>>()
    );

    assert_schema_valid("schemas/procedure-v2.schema.json", &maximum);
}

#[test]
fn v2ctr002_procedure_v2_schema_rejects_unknown_fields_and_policy_drift() {
    let valid = procedure_v2_fixture();
    for (name, mut invalid) in [
        ("root unknown", valid.clone()),
        ("definition unknown", valid.clone()),
        ("item unknown", valid.clone()),
        ("placement display override", valid.clone()),
        ("route unknown", valid.clone()),
        ("option descriptive extension", valid.clone()),
    ] {
        match name {
            "root unknown" => invalid["unknown"] = json!(true),
            "definition unknown" => invalid["node_definitions"]["work"]["unknown"] = json!(true),
            "item unknown" => {
                invalid["node_definitions"]["work"]["items"][0]["unknown"] = json!(true)
            }
            "placement display override" => {
                invalid["graph"]["nodes"][0]["title"] = json!("Override")
            }
            "route unknown" => {
                invalid["graph"]["nodes"][1]["routes"]["achieved"]["unknown"] = json!(true)
            }
            "option descriptive extension" => {
                invalid["node_definitions"]["choose"]["options"][0]["description"] =
                    json!("No extension")
            }
            _ => unreachable!(),
        }
        assert_schema_invalid("schemas/procedure-v2.schema.json", &invalid);
    }

    let mut false_reason = valid.clone();
    false_reason["node_definitions"]["choose"]["reason"]["required"] = json!(false);
    assert_schema_invalid("schemas/procedure-v2.schema.json", &false_reason);

    let mut v1_reason_spelling = valid.clone();
    v1_reason_spelling["node_definitions"]["choose"]["reason"] = json!({"reason_required":true});
    assert_schema_invalid("schemas/procedure-v2.schema.json", &v1_reason_spelling);

    let mut false_skip = valid.clone();
    false_skip["graph"]["nodes"][0]["skip"]["allowed"] = json!(false);
    assert_schema_invalid("schemas/procedure-v2.schema.json", &false_skip);

    let mut reason_field_in_skip = valid;
    reason_field_in_skip["graph"]["nodes"][0]["skip"] = json!({"allowed":true,"required":true});
    assert_schema_invalid("schemas/procedure-v2.schema.json", &reason_field_in_skip);
}

#[test]
fn v2ctr002_procedure_v2_schema_enforces_collection_and_string_bounds() {
    let valid = procedure_v2_fixture();
    let mut invalid_cases = Vec::new();

    let mut empty_version = valid.clone();
    empty_version["version"] = json!("");
    invalid_cases.push(empty_version);
    let mut long_version = valid.clone();
    long_version["version"] = json!("v".repeat(65));
    invalid_cases.push(long_version);
    for (pointer, value) in [
        ("/id", json!(format!("a{}", "a".repeat(64)))),
        ("/name", json!("n".repeat(121))),
        ("/purpose", json!("p".repeat(501))),
        ("/description", json!("d".repeat(1001))),
        ("/node_definitions/work/title", json!("t".repeat(121))),
        ("/node_definitions/work/intent", json!("i".repeat(301))),
        (
            "/node_definitions/work/description",
            json!("d".repeat(1001)),
        ),
        ("/node_definitions/choose/title", json!("t".repeat(121))),
        ("/node_definitions/choose/objective", json!("o".repeat(301))),
        ("/node_definitions/choose/prompt", json!("p".repeat(501))),
        (
            "/node_definitions/choose/reason/prompt",
            json!("r".repeat(301)),
        ),
        (
            "/node_definitions/choose/options/0/label",
            json!("l".repeat(121)),
        ),
        (
            "/node_definitions/choose/options/0/criteria",
            json!("c".repeat(501)),
        ),
        (
            "/node_definitions/work/items/0/prompt",
            json!("p".repeat(301)),
        ),
        (
            "/node_definitions/work/items/0/help",
            json!("h".repeat(1001)),
        ),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        invalid_cases.push(invalid);
    }
    let mut missing_action_intent = valid.clone();
    missing_action_intent["node_definitions"]["work"]
        .as_object_mut()
        .unwrap()
        .remove("intent");
    invalid_cases.push(missing_action_intent);
    let mut empty_optional_collection = valid.clone();
    empty_optional_collection["node_definitions"]["work"]["instructions"] = json!([]);
    invalid_cases.push(empty_optional_collection);

    let mut too_many_instructions = valid.clone();
    too_many_instructions["node_definitions"]["work"]["instructions"] =
        json!(vec!["instruction"; 17]);
    invalid_cases.push(too_many_instructions);
    let mut long_instruction = valid.clone();
    long_instruction["node_definitions"]["work"]["instructions"][0] = json!("i".repeat(1001));
    invalid_cases.push(long_instruction);
    let mut too_many_items = valid.clone();
    let item = valid["node_definitions"]["work"]["items"][0].clone();
    too_many_items["node_definitions"]["work"]["items"] = json!(vec![item; 65]);
    invalid_cases.push(too_many_items);
    let mut too_many_choices = valid.clone();
    too_many_choices["node_definitions"]["work"]["items"][2]["choices"] = json!(
        (0..33)
            .map(|index| format!("choice-{index}"))
            .collect::<Vec<_>>()
    );
    invalid_cases.push(too_many_choices);
    let mut text_too_large = valid.clone();
    text_too_large["node_definitions"]["work"]["items"][1]["max_length"] = json!(16385);
    invalid_cases.push(text_too_large);
    let mut list_too_large = valid.clone();
    list_too_large["node_definitions"]["work"]["items"][4]["max_items"] = json!(101);
    invalid_cases.push(list_too_large);
    let mut list_item_too_large = valid.clone();
    list_item_too_large["node_definitions"]["work"]["items"][4]["max_item_length"] = json!(1001);
    invalid_cases.push(list_item_too_large);

    let mut too_many_options = valid.clone();
    let option = valid["node_definitions"]["choose"]["options"][0].clone();
    too_many_options["node_definitions"]["choose"]["options"] = json!(vec![option; 9]);
    invalid_cases.push(too_many_options);
    let mut too_much_guidance = valid.clone();
    too_much_guidance["node_definitions"]["choose"]["evidence_guidance"] =
        json!(vec!["guidance"; 9]);
    invalid_cases.push(too_much_guidance);
    let mut long_guidance = valid.clone();
    long_guidance["node_definitions"]["choose"]["evidence_guidance"][0] = json!("g".repeat(201));
    invalid_cases.push(long_guidance);
    let mut too_many_references = valid.clone();
    too_many_references["graph"]["nodes"][1]["evidence_from"] =
        json!(vec![json!({"node":"perform"}); 9]);
    invalid_cases.push(too_many_references);
    let mut too_many_selected_items = valid.clone();
    too_many_selected_items["graph"]["nodes"][1]["evidence_from"][0]["items"] = json!(
        (0..17)
            .map(|index| format!("item-{index}"))
            .collect::<Vec<_>>()
    );
    invalid_cases.push(too_many_selected_items);
    let mut too_many_nodes = valid.clone();
    let node = valid["graph"]["nodes"][2].clone();
    too_many_nodes["graph"]["nodes"] = json!(vec![node; 65]);
    invalid_cases.push(too_many_nodes);
    let mut too_many_definitions = valid.clone();
    let definition = valid["node_definitions"]["work"].clone();
    let definitions = (0..65)
        .map(|index| (format!("definition-{index}"), definition.clone()))
        .collect::<Map<String, Value>>();
    too_many_definitions["node_definitions"] = Value::Object(definitions);
    invalid_cases.push(too_many_definitions);
    let mut too_many_manual_targets = valid.clone();
    too_many_manual_targets["manual_rework"]["allowed_targets"] = json!(
        (0..65)
            .map(|index| format!("target-{index}"))
            .collect::<Vec<_>>()
    );
    invalid_cases.push(too_many_manual_targets);

    for invalid in invalid_cases {
        assert_schema_invalid("schemas/procedure-v2.schema.json", &invalid);
    }
}
