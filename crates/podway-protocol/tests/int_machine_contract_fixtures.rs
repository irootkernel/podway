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
    MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1, RequestEnvelopeV1, ResponseEnvelopeV1,
    SUPPORTED_RESULT_SCHEMAS_V1, validate_command_result_v1,
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
            .map(|(command, schemas)| (command, schemas.into_iter().collect()))
            .collect()
    );

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
        .collect::<BTreeSet<_>>();
    assert_eq!(assets.keys().cloned().collect::<BTreeSet<_>>(), expected);
    for (relative, digest) in assets {
        assert_eq!(digest, sha256(&root().join(relative)));
    }
}
