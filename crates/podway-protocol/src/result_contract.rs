#![allow(dead_code)]

use std::{
    collections::HashMap,
    error::Error,
    io,
    sync::{Arc, OnceLock},
};

use jsonschema::{Retrieve, Uri, Validator};
use podway_core::{JobId, Revision, Sha256Digest};
use serde::{Deserialize, Deserializer, Serialize, de, de::Error as _};
use serde_json::{Map, Value};

use crate::{
    CommandNameV1, ErrorEnvelopeV1, JobOutputV1, JobStateV1, OptionalField, ProtocolError,
    RequestIdV1, Rfc3339MillisV1, SessionOutputV1, WorkspaceOutputV1,
    validate_admission_metadata_v1, validate_json_document_depth, validate_json_map_depth,
};

mod embedded_schemas {
    include!(concat!(env!("OUT_DIR"), "/embedded_json_schemas.rs"));
}

const EMBEDDED_SCHEMA_BASE_V2: &str = "https://podway.invalid/schemas/";

#[derive(Clone)]
struct EmbeddedSchemaRetrieverV2(Arc<HashMap<String, Value>>);

impl Retrieve for EmbeddedSchemaRetrieverV2 {
    fn retrieve(&self, uri: &Uri<String>) -> Result<Value, Box<dyn Error + Send + Sync>> {
        self.0.get(uri.as_str()).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("unregistered embedded schema URI: {uri}"),
            )
            .into()
        })
    }
}

static RESULT_VALIDATORS_V2: OnceLock<Result<HashMap<&'static str, Validator>, String>> =
    OnceLock::new();

fn embedded_schema_resources_v2() -> Result<EmbeddedSchemaRetrieverV2, String> {
    let mut resources = HashMap::new();
    for (id, filename, source) in embedded_schemas::EMBEDDED_JSON_SCHEMAS_V1 {
        let schema: Value = serde_json::from_str(source)
            .map_err(|error| format!("embedded schema {filename} is invalid: {error}"))?;
        if resources.insert((*id).to_owned(), schema.clone()).is_some() {
            return Err(format!("duplicate embedded schema identifier {id}"));
        }
        let local_uri = format!("{EMBEDDED_SCHEMA_BASE_V2}{filename}");
        if resources.insert(local_uri.clone(), schema).is_some() {
            return Err(format!("duplicate embedded schema URI {local_uri}"));
        }
    }
    Ok(EmbeddedSchemaRetrieverV2(Arc::new(resources)))
}

fn build_result_validators_v2() -> Result<HashMap<&'static str, Validator>, String> {
    let retriever = embedded_schema_resources_v2()?;
    let sources = embedded_schemas::EMBEDDED_JSON_SCHEMAS_V1
        .iter()
        .map(|(_, filename, source)| (*filename, *source))
        .collect::<HashMap<_, _>>();
    let mut validators = HashMap::new();
    for contract in EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
    {
        let filename = contract
            .schema_path
            .strip_prefix("schemas/")
            .ok_or_else(|| format!("invalid result schema path {}", contract.schema_path))?;
        let source = sources
            .get(filename)
            .ok_or_else(|| format!("embedded result schema {filename} is missing"))?;
        let mut schema: Value = serde_json::from_str(source)
            .map_err(|error| format!("embedded result schema {filename} is invalid: {error}"))?;
        schema
            .as_object_mut()
            .ok_or_else(|| format!("embedded result schema {filename} is not an object"))?
            .remove("$id");
        let validator = jsonschema::draft202012::options()
            .with_base_uri(format!("{EMBEDDED_SCHEMA_BASE_V2}{filename}"))
            .with_retriever(retriever.clone())
            .should_validate_formats(true)
            .build(&schema)
            .map_err(|error| {
                format!("embedded result schema {filename} does not compile: {error}")
            })?;
        validators.insert(contract.schema, validator);
    }
    let filename = "output-v3.schema.json";
    let source = sources
        .get(filename)
        .ok_or_else(|| format!("embedded output schema {filename} is missing"))?;
    let mut schema: Value = serde_json::from_str(source)
        .map_err(|error| format!("embedded output schema {filename} is invalid: {error}"))?;
    schema
        .as_object_mut()
        .ok_or_else(|| format!("embedded output schema {filename} is not an object"))?
        .remove("$id");
    let validator = jsonschema::draft202012::options()
        .with_base_uri(format!("{EMBEDDED_SCHEMA_BASE_V2}{filename}"))
        .with_retriever(retriever)
        .should_validate_formats(true)
        .build(&schema)
        .map_err(|error| format!("embedded output schema {filename} does not compile: {error}"))?;
    validators.insert(OUTPUT_SCHEMA_V3, validator);
    Ok(validators)
}

fn validate_embedded_result_schema_v2(schema: &str, result: &Map<String, Value>) -> bool {
    validate_embedded_schema_v2(schema, &Value::Object(result.clone()))
}

fn validate_embedded_schema_v2(schema: &str, value: &Value) -> bool {
    RESULT_VALIDATORS_V2
        .get_or_init(build_result_validators_v2)
        .as_ref()
        .ok()
        .and_then(|validators| validators.get(schema))
        .is_some_and(|validator| validator.is_valid(value))
}

/// A closed result family reserved by the Procedure v2 public contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResultSchemaContractV2 {
    pub schema: &'static str,
    pub schema_path: &'static str,
    pub commands: &'static [&'static str],
}

/// Mutation routes whose v2 terminal responses and errors carry admission metadata.
pub const V2_MUTATION_COMMANDS: &[&str] = &[
    "session.start",
    "session.start_replace",
    "session.complete",
    "session.skip",
    "session.retry",
    "session.block",
    "session.unblock",
    "session.cancel",
    "session.reset",
    "session.decide",
    "session.rework",
    "goal.define",
    "goal.revise",
    "goal.assess_criterion",
    "item.check",
    "item.uncheck",
    "item.set",
    "item.add",
    "item.remove",
    "item.attach",
    "item.clear",
];

/// Result families for existing routes whose v2-session shape breaks from v1.
pub const EXISTING_ROUTE_RESULT_SCHEMAS_V2: &[ResultSchemaContractV2] = &[
    result_schema_v2(
        "podway.procedure-validation-result/v2",
        "schemas/procedure-validation-result-v2.schema.json",
        &["procedure.validate"],
    ),
    result_schema_v2(
        "podway.detached-admission-result/v2",
        "schemas/detached-admission-result-v2.schema.json",
        &[
            "session.start",
            "session.start_replace",
            "session.complete",
            "session.skip",
            "session.retry",
            "session.block",
            "session.unblock",
            "session.cancel",
            "session.reset",
            "session.decide",
            "session.rework",
            "goal.define",
            "goal.revise",
            "goal.assess_criterion",
            "item.check",
            "item.uncheck",
            "item.set",
            "item.add",
            "item.remove",
            "item.attach",
            "item.clear",
        ],
    ),
    result_schema_v2(
        "podway.session-start-result/v2",
        "schemas/session-start-result-v2.schema.json",
        &["session.start", "session.start_replace"],
    ),
    result_schema_v2(
        "podway.status-result/v2",
        "schemas/status-result-v2.schema.json",
        &["session.status"],
    ),
    result_schema_v2(
        "podway.compact-status-result/v2",
        "schemas/compact-status-result-v2.schema.json",
        &["session.status"],
    ),
    result_schema_v2(
        "podway.next-result/v2",
        "schemas/next-result-v2.schema.json",
        &["session.next"],
    ),
    result_schema_v2(
        "podway.stage-transition-result/v2",
        "schemas/stage-transition-result-v2.schema.json",
        &[
            "session.complete",
            "session.skip",
            "session.retry",
            "session.block",
            "session.unblock",
            "session.cancel",
            "session.reset",
        ],
    ),
    result_schema_v2(
        "podway.item-mutation-result/v2",
        "schemas/item-mutation-result-v2.schema.json",
        &[
            "item.check",
            "item.uncheck",
            "item.set",
            "item.add",
            "item.remove",
            "item.attach",
            "item.clear",
        ],
    ),
    result_schema_v2(
        "podway.job-lookup-result/v3",
        "schemas/job-lookup-result-v3.schema.json",
        &["job.lookup"],
    ),
    result_schema_v2(
        "podway.job-result/v3",
        "schemas/job-result-v3.schema.json",
        &["job.status", "job.wait"],
    ),
];

/// The shared Procedure v2 authoring diagnostics family.
pub const PROCEDURE_DIAGNOSTICS_RESULT_SCHEMA_V1: &str = "podway.procedure-diagnostics-result/v1";

/// Result families for v2-only routes. New command surfaces begin at `/v1`.
pub const NEW_ROUTE_RESULT_SCHEMAS_V1: &[ResultSchemaContractV2] = &[
    result_schema_v2(
        "podway.observation-result/v1",
        "schemas/observation-result-v1.schema.json",
        &["session.observe"],
    ),
    result_schema_v2(
        "podway.procedure-source-result/v1",
        "schemas/procedure-source-result-v1.schema.json",
        &["procedure.format", "procedure.scaffold"],
    ),
    result_schema_v2(
        PROCEDURE_DIAGNOSTICS_RESULT_SCHEMA_V1,
        "schemas/procedure-diagnostics-result-v1.schema.json",
        &[
            "procedure.format",
            "procedure.validate",
            "procedure.vet",
            "procedure.lint",
            "procedure.check",
            "procedure.graph",
            "procedure.scaffold",
        ],
    ),
    result_schema_v2(
        "podway.procedure-graph-result/v1",
        "schemas/procedure-graph-result-v1.schema.json",
        &["procedure.graph"],
    ),
    result_schema_v2(
        "podway.procedure-preview-result/v1",
        "schemas/procedure-preview-result-v1.schema.json",
        &["procedure.preview"],
    ),
    result_schema_v2(
        "podway.decision-result/v1",
        "schemas/decision-result-v1.schema.json",
        &["session.decide"],
    ),
    result_schema_v2(
        "podway.rework-result/v1",
        "schemas/rework-result-v1.schema.json",
        &["session.rework"],
    ),
    result_schema_v2(
        "podway.goal-definition-result/v1",
        "schemas/goal-definition-result-v1.schema.json",
        &["goal.define"],
    ),
    result_schema_v2(
        "podway.goal-revision-result/v1",
        "schemas/goal-revision-result-v1.schema.json",
        &["goal.revise"],
    ),
    result_schema_v2(
        "podway.criterion-assessment-result/v1",
        "schemas/criterion-assessment-result-v1.schema.json",
        &["goal.assess_criterion"],
    ),
];

const fn result_schema_v2(
    schema: &'static str,
    schema_path: &'static str,
    commands: &'static [&'static str],
) -> ResultSchemaContractV2 {
    ResultSchemaContractV2 {
        schema,
        schema_path,
        commands,
    }
}

/// Decodes a result discriminator against the reserved v2-aware registry.
///
/// This does not make any future command routable. It only lets later route and
/// runtime work select the exact closed family without extending a v1 shape.
pub fn decode_result_schema_contract_v2(
    result: &Map<String, Value>,
) -> Option<&'static ResultSchemaContractV2> {
    let schema = result.get("schema")?.as_str()?;
    let contract = EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
        .find(|contract| contract.schema == schema)?;
    let required = required_result_fields_v2(schema);
    let allowed = allowed_result_fields_v2(schema);
    (required.iter().all(|field| result.contains_key(*field))
        && result.keys().all(|field| allowed.contains(&field.as_str()))
        && validate_json_map_depth(result, 1).is_ok()
        && validate_embedded_result_schema_v2(schema, result)
        && validate_result_correlations_v2(schema, result))
    .then_some(contract)
}

fn validate_result_correlations_v2(schema: &str, result: &Map<String, Value>) -> bool {
    match schema {
        "podway.decision-result/v1" => {
            let Some(record) = result.get("record").and_then(Value::as_object) else {
                return false;
            };
            [
                "graph_node_id",
                "attempt_id",
                "attempt_number",
                "option_id",
                "effect",
                "target_graph_node_id",
            ]
            .iter()
            .all(|field| {
                result
                    .get(*field)
                    .zip(record.get(*field))
                    .is_some_and(|(projected, recorded)| projected == recorded)
            })
        }
        "podway.job-result/v3" => {
            terminal_response_command_v2(result.get("job"))
                .is_some_and(|command| command.is_none_or(is_durable_job_command_v3))
                && result
                    .get("job")
                    .is_some_and(validate_terminal_response_typed_v2)
        }
        "podway.job-lookup-result/v3" => match result.get("found") {
            Some(Value::Bool(false)) => !result.contains_key("job"),
            Some(Value::Bool(true)) => {
                let Some(job) = result.get("job").and_then(Value::as_object) else {
                    return false;
                };
                let Some(job_command) = job.get("command").and_then(Value::as_str) else {
                    return false;
                };
                is_durable_job_command_v3(job_command)
                    && terminal_response_command_v2(job.get("terminal_response")).is_some_and(
                        |terminal_command| {
                            terminal_command.is_none_or(|value| value == job_command)
                        },
                    )
                    && terminal_response_identity_matches_job_v2(job)
                    && job
                        .get("terminal_response")
                        .is_some_and(validate_terminal_response_typed_v2)
            }
            _ => false,
        },
        _ => true,
    }
}

fn validate_terminal_response_typed_v2(response: &Value) -> bool {
    let Some(response) = response.as_object() else {
        return response.is_null();
    };
    match response.get("schema").and_then(Value::as_str) {
        Some(OUTPUT_SCHEMA_V3) => {
            serde_json::from_value::<OutputEnvelopeV3>(Value::Object(response.clone())).is_ok()
        }
        Some(crate::ERROR_SCHEMA_V1) => {
            serde_json::from_value::<ErrorEnvelopeV1>(Value::Object(response.clone())).is_ok()
                && validate_terminal_error_v2(response)
        }
        None => response.get("kind").and_then(Value::as_str) == Some("cancelled"),
        Some(_) => false,
    }
}

fn validate_terminal_error_v2(response: &Map<String, Value>) -> bool {
    const ALLOWED_FIELDS: &[&str] = &[
        "schema",
        "request_id",
        "command",
        "generated_at",
        "code",
        "message",
        "retryable",
        "exit_code",
        "workspace",
        "details",
    ];
    if !response
        .keys()
        .all(|field| ALLOWED_FIELDS.contains(&field.as_str()))
    {
        return false;
    }
    if serde_json::to_vec(response)
        .map_or(true, |encoded| encoded.len() > MAX_V2_TERMINAL_ERROR_BYTES)
    {
        return false;
    }
    if !response
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| {
            let characters = message.chars().count();
            (1..=crate::MAX_V2_RUNTIME_ERROR_MESSAGE_CHARS_V1).contains(&characters)
        })
    {
        return false;
    }
    let Some(workspace) = response.get("workspace") else {
        return false;
    };
    let Some(workspace) = workspace.as_object() else {
        return false;
    };
    workspace.len() == 3
        && ["uuid", "root", "latest_workspace_sequence"]
            .iter()
            .all(|field| workspace.contains_key(*field))
        && serde_json::from_value::<WorkspaceOutputV1>(Value::Object(workspace.clone())).is_ok()
}

fn terminal_response_identity_matches_job_v2(job: &Map<String, Value>) -> bool {
    let Some(response) = job.get("terminal_response") else {
        return false;
    };
    if response.is_null()
        || response
            .as_object()
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str)
            == Some("cancelled")
    {
        return true;
    }
    let Some(response) = response.as_object() else {
        return false;
    };
    let admission = match response.get("schema").and_then(Value::as_str) {
        Some(OUTPUT_SCHEMA_V3) => response
            .get("result")
            .and_then(Value::as_object)
            .and_then(|result| result.get("admission")),
        Some(crate::ERROR_SCHEMA_V1) => response
            .get("details")
            .and_then(Value::as_object)
            .and_then(|details| details.get("admission")),
        _ => return false,
    };
    let Some(admission) = admission.and_then(Value::as_object) else {
        return false;
    };
    if admission.get("admitted") != Some(&Value::Bool(true))
        || admission.get("job_id") != job.get("id")
        || admission.get("workspace_sequence") != job.get("sequence")
    {
        return false;
    }
    if response.get("schema").and_then(Value::as_str) == Some(OUTPUT_SCHEMA_V3) {
        let Some(response_job) = response.get("job").and_then(Value::as_object) else {
            return false;
        };
        return [
            "id",
            "sequence",
            "state",
            "submitted_at",
            "claimed_at",
            "finished_at",
        ]
        .iter()
        .all(|field| response_job.get(*field) == job.get(*field));
    }
    true
}

fn validate_job_result_output_v2(output: &OutputEnvelopeV3) -> bool {
    if output.result.get("schema").and_then(Value::as_str) != Some("podway.job-result/v3") {
        return true;
    }
    let Some(job) = output.job.as_ref() else {
        return false;
    };
    let Some(response) = output.result.get("job") else {
        return false;
    };
    let state_matches = match job.state() {
        JobStateV1::Queued | JobStateV1::Running => response.is_null(),
        JobStateV1::Succeeded => {
            response.get("schema").and_then(Value::as_str) == Some(OUTPUT_SCHEMA_V3)
        }
        JobStateV1::Failed => {
            response.get("schema").and_then(Value::as_str) == Some(crate::ERROR_SCHEMA_V1)
        }
        JobStateV1::Cancelled => response.get("kind").and_then(Value::as_str) == Some("cancelled"),
    };
    if !state_matches {
        return false;
    }
    let Ok(Value::Object(mut job_projection)) = serde_json::to_value(job) else {
        return false;
    };
    job_projection.insert("terminal_response".to_owned(), response.clone());
    terminal_response_identity_matches_job_v2(&job_projection)
}

fn terminal_response_command_v2(value: Option<&Value>) -> Option<Option<&str>> {
    match value {
        Some(Value::Null) => Some(None),
        Some(Value::Object(response))
            if response.get("kind").and_then(Value::as_str) == Some("cancelled") =>
        {
            Some(None)
        }
        Some(Value::Object(response)) => response.get("command").and_then(Value::as_str).map(Some),
        _ => None,
    }
}

fn is_v2_mutation_command(command: &str) -> bool {
    V2_MUTATION_COMMANDS.contains(&command)
}

fn is_durable_job_command_v3(command: &str) -> bool {
    matches!(command, "workspace.init" | "workspace.reset_all") || is_v2_mutation_command(command)
}

fn required_result_fields_v2(schema: &str) -> &'static [&'static str] {
    match schema {
        "podway.procedure-validation-result/v2" => {
            &["schema", "file", "procedure_schema", "digest", "valid"]
        }
        "podway.detached-admission-result/v2" => &["schema", "detached", "admission"],
        "podway.session-start-result/v2" => &[
            "schema",
            "procedure_schema",
            "procedure_digest",
            "dry_run",
            "goal_tracking",
            "goal_defined",
        ],
        "podway.compact-status-result/v2" => &[
            "schema",
            "procedure",
            "session",
            "current",
            "goal_tracking",
            "goal_defined",
            "trace_length",
            "counters",
            "items",
            "queue",
        ],
        "podway.status-result/v2" => &[
            "schema",
            "tier",
            "procedure",
            "session",
            "current",
            "purpose",
            "goal_tracking",
            "goal_defined",
            "trace_length",
            "counters",
            "items",
            "queue",
            "missing_required_item_ids",
            "blocker_window",
            "blockers_truncated",
            "item_values",
            "items_total",
            "items_truncated",
            "references",
            "allowed_option_ids",
            "allowed_manual_rework_targets",
        ],
        "podway.next-result/v2" => &[
            "schema",
            "procedure_schema",
            "procedure_digest",
            "goal_tracking",
            "goal_defined",
            "node",
            "attempt",
            "trace_length",
            "counters",
            "queue",
            "revision",
            "readiness",
            "missing_required_item_count",
            "blockers_total",
            "allowed_actions",
            "suggestions",
            "references",
            "readback",
            "allowed_manual_rework_targets",
        ],
        "podway.observation-result/v1" => &[
            "schema",
            "status",
            "guidance",
            "active_items",
            "mutation_templates",
        ],
        "podway.stage-transition-result/v2" => &["schema", "admission", "transition", "revision"],
        "podway.item-mutation-result/v2" => &[
            "schema",
            "admission",
            "changed",
            "graph_node_id",
            "attempt_id",
            "attempt_number",
            "item_id",
            "revision",
        ],
        "podway.job-lookup-result/v3" => &["schema", "found"],
        "podway.job-result/v3" => &["schema", "job"],
        "podway.procedure-source-result/v1" => &[
            "schema",
            "operation",
            "target_schema",
            "document",
            "target_digest",
        ],
        "podway.procedure-diagnostics-result/v1" => &[
            "schema",
            "operation",
            "procedure_schema",
            "file",
            "valid",
            "diagnostics",
            "diagnostics_truncated",
            "diagnostics_total",
        ],
        "podway.procedure-graph-result/v1" => &[
            "schema",
            "procedure_schema",
            "procedure_digest",
            "format",
            "projection_digest",
            "projection",
        ],
        "podway.procedure-preview-result/v1" => &[
            "schema",
            "file",
            "admissible",
            "checks",
            "diagnostics",
            "diagnostics_truncated",
            "diagnostics_total",
        ],
        "podway.decision-result/v1" => &[
            "schema",
            "admission",
            "graph_node_id",
            "attempt_id",
            "attempt_number",
            "option_id",
            "effect",
            "target_graph_node_id",
            "target_attempt_id",
            "revision",
            "session_state",
            "record",
        ],
        "podway.rework-result/v1" => &[
            "schema",
            "admission",
            "from_graph_node_id",
            "to_graph_node_id",
            "target_attempt_id",
            "reason",
            "reactivated",
            "revision",
        ],
        "podway.goal-definition-result/v1" => &[
            "schema",
            "admission",
            "goal_revision",
            "statement",
            "criteria",
            "recorded_at",
            "revision",
        ],
        "podway.goal-revision-result/v1" => &[
            "schema",
            "admission",
            "goal_revision",
            "statement",
            "criteria",
            "reason",
            "recorded_at",
            "rework_to",
            "reactivated",
            "revision",
        ],
        "podway.criterion-assessment-result/v1" => &[
            "schema",
            "admission",
            "graph_node_id",
            "attempt_id",
            "goal_revision",
            "mode",
            "result",
            "complete",
            "revision",
        ],
        _ => &[],
    }
}

fn allowed_result_fields_v2(schema: &str) -> &'static [&'static str] {
    match schema {
        "podway.detached-admission-result/v2" => {
            &["schema", "detached", "admission", "procedure_digest"]
        }
        "podway.session-start-result/v2" => &[
            "schema",
            "procedure_schema",
            "procedure_digest",
            "dry_run",
            "goal_tracking",
            "admission",
            "session_id",
            "revision",
            "entry_graph_node_id",
            "goal_defined",
        ],
        "podway.compact-status-result/v2" => &[
            "schema",
            "procedure",
            "session",
            "current",
            "goal_tracking",
            "goal_defined",
            "goal_revision",
            "latest_goal_outcome",
            "trace_length",
            "counters",
            "items",
            "queue",
        ],
        "podway.status-result/v2" => &[
            "schema",
            "tier",
            "procedure",
            "session",
            "current",
            "purpose",
            "goal_tracking",
            "goal_defined",
            "goal_revision",
            "latest_goal_outcome",
            "goal",
            "trace_length",
            "counters",
            "items",
            "queue",
            "missing_required_item_ids",
            "blocker_window",
            "blockers_truncated",
            "item_values",
            "items_total",
            "items_truncated",
            "references",
            "allowed_option_ids",
            "next_graph_node_id",
            "terminal",
            "skip",
            "allowed_manual_rework_targets",
            "current_trace_history",
            "stale_attempt_history",
            "decision_history",
            "rework_history",
            "stale_goal_revision_history",
            "stale_goal_assessment_history",
        ],
        "podway.next-result/v2" => &[
            "schema",
            "procedure_schema",
            "procedure_digest",
            "goal_tracking",
            "goal_defined",
            "goal_revision",
            "latest_goal_outcome",
            "goal",
            "node",
            "attempt",
            "trace_length",
            "counters",
            "queue",
            "revision",
            "readiness",
            "title",
            "intent",
            "description",
            "objective",
            "prompt",
            "reason_policy",
            "instructions",
            "missing_required_item_count",
            "missing_required_items",
            "options",
            "evidence_guidance",
            "next_graph_node_id",
            "terminal",
            "skip",
            "allowed_manual_rework_targets",
            "allowed_actions",
            "suggestions",
            "references",
            "readback",
            "blockers_total",
            "blockers",
            "blockers_truncated",
        ],
        "podway.observation-result/v1" => &[
            "schema",
            "status",
            "guidance",
            "active_items",
            "mutation_templates",
        ],
        "podway.stage-transition-result/v2" => &[
            "schema",
            "admission",
            "transition",
            "from_graph_node_id",
            "from_attempt_id",
            "to_graph_node_id",
            "to_attempt_id",
            "blocker_id",
            "all",
            "reason",
            "reset",
            "revision",
            "session_state",
        ],
        "podway.item-mutation-result/v2" => &[
            "schema",
            "admission",
            "changed",
            "graph_node_id",
            "attempt_id",
            "attempt_number",
            "item_id",
            "revision",
            "value_digest",
        ],
        "podway.job-lookup-result/v3" => &["schema", "found", "job"],
        "podway.job-result/v3" => &["schema", "job"],
        "podway.procedure-source-result/v1" => &[
            "schema",
            "operation",
            "target_schema",
            "document",
            "target_digest",
            "file",
            "mode",
            "changed",
            "template",
            "source_schema",
            "source_digest",
        ],
        "podway.procedure-diagnostics-result/v1" => &[
            "schema",
            "operation",
            "procedure_schema",
            "file",
            "digest",
            "valid",
            "diagnostics",
            "diagnostics_truncated",
            "diagnostics_total",
        ],
        "podway.procedure-preview-result/v1" => &[
            "schema",
            "file",
            "admissible",
            "procedure_schema",
            "procedure_id",
            "procedure_version",
            "purpose",
            "procedure_digest",
            "goal_tracking",
            "goal_assessment_graph_node_ids",
            "summary",
            "checks",
            "graph",
            "mermaid",
            "start_suggestion",
            "diagnostics",
            "diagnostics_truncated",
            "diagnostics_total",
        ],
        "podway.decision-result/v1" => &[
            "schema",
            "admission",
            "graph_node_id",
            "attempt_id",
            "attempt_number",
            "option_id",
            "effect",
            "target_graph_node_id",
            "target_attempt_id",
            "revision",
            "session_state",
            "record",
        ],
        "podway.goal-definition-result/v1" => &[
            "schema",
            "admission",
            "goal_revision",
            "statement",
            "criteria",
            "actor",
            "recorded_at",
            "revision",
        ],
        "podway.goal-revision-result/v1" => &[
            "schema",
            "admission",
            "goal_revision",
            "statement",
            "criteria",
            "reason",
            "actor",
            "recorded_at",
            "rework_to",
            "reactivated",
            "revision",
        ],
        "podway.criterion-assessment-result/v1" => &[
            "schema",
            "admission",
            "graph_node_id",
            "attempt_id",
            "goal_revision",
            "mode",
            "result",
            "complete",
            "determined_outcome",
            "revision",
        ],
        _ => required_result_fields_v2(schema),
    }
}

/// Returns the exact required and allowed top-level keys for a registered family.
pub fn result_schema_top_level_fields_v2(
    schema: &str,
) -> Option<(&'static [&'static str], &'static [&'static str])> {
    EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
        .any(|contract| contract.schema == schema)
        .then(|| {
            (
                required_result_fields_v2(schema),
                allowed_result_fields_v2(schema),
            )
        })
}

pub const MAX_V2_OUTPUT_WARNINGS: usize = 4;
pub const MAX_V2_WARNING_CODE_CHARS: usize = 64;
pub const MAX_V2_WARNING_PATH_CHARS: usize = 256;
pub const MAX_V2_WARNING_MESSAGE_CHARS: usize = 512;
/// Maximum encoded bytes for an error retained inside one v2 job-result wrapper.
pub const MAX_V2_TERMINAL_ERROR_BYTES: usize = 524_288;
pub const OUTPUT_SCHEMA_V3: &str = "podway.output/v3";
pub const SUPPORTED_OUTPUT_SCHEMAS_V3: &[&str] = &[OUTPUT_SCHEMA_V3];
const PROCEDURE_INDEPENDENT_OUTPUT_COMMANDS_V3: &[&str] = &[
    "help",
    "version",
    "completions",
    "preset.list",
    "preset.show",
    "preset.explain",
    "procedure.show",
    "daemon.install",
    "daemon.uninstall",
    "daemon.start",
    "daemon.stop",
    "daemon.restart",
    "daemon.status",
    "daemon.logs",
    "daemon.terminate",
    "workspace.init",
    "workspace.show",
    "workspace.doctor",
    "workspace.repair",
    "workspace.reset_all",
    "job.list",
    "job.cancel",
    "__complete",
];

/// Checks the production warning bound for the open v2 success envelope.
pub fn validate_v2_output_warnings(warnings: &[Map<String, Value>]) -> bool {
    warnings.len() <= MAX_V2_OUTPUT_WARNINGS
        && warnings.iter().all(|warning| {
            warning.len() == 3
                && warning
                    .keys()
                    .all(|field| matches!(field.as_str(), "code" | "path" | "message"))
                && warning
                    .get("code")
                    .and_then(Value::as_str)
                    .is_some_and(|value| {
                        !value.is_empty() && value.chars().count() <= MAX_V2_WARNING_CODE_CHARS
                    })
                && warning
                    .get("path")
                    .and_then(Value::as_str)
                    .is_some_and(|value| {
                        !value.is_empty() && value.chars().count() <= MAX_V2_WARNING_PATH_CHARS
                    })
                && warning
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|value| {
                        !value.is_empty() && value.chars().count() <= MAX_V2_WARNING_MESSAGE_CHARS
                    })
        })
}

/// Validates the closed v2 result family selected by a public command name.
///
/// A route bound to more than one registered family accepts any of them. Unlike
/// the v1 twin this never infers a discriminator: the caller must set the
/// result `schema` because a v2 route can carry two families.
pub fn validate_command_result_v2(
    command: &str,
    result: &Map<String, Value>,
) -> Result<(), ProtocolError> {
    if decode_result_schema_contract_v2(result)
        .is_some_and(|contract| contract.commands.contains(&command))
        || (PROCEDURE_INDEPENDENT_OUTPUT_COMMANDS_V3.contains(&command)
            && validate_command_result_v1(command, result).is_ok())
    {
        Ok(())
    } else {
        Err(ProtocolError::InvalidCommandResult {
            command: command.to_owned(),
        })
    }
}

/// Validated fields used to construct one current success envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputEnvelopeInputV3 {
    pub request_id: RequestIdV1,
    pub command: CommandNameV1,
    pub generated_at: Rfc3339MillisV1,
    pub workspace: Option<WorkspaceOutputV1>,
    pub job: Option<JobOutputV1>,
    pub session: Option<SessionOutputV1>,
    pub result: Map<String, Value>,
    pub warnings: Vec<Map<String, Value>>,
}

/// A validated `podway.output/v3` response envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OutputEnvelopeV3 {
    schema: String,
    request_id: RequestIdV1,
    command: CommandNameV1,
    generated_at: Rfc3339MillisV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace: Option<WorkspaceOutputV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job: Option<JobOutputV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<SessionOutputV1>,
    result: Map<String, Value>,
    warnings: Vec<Map<String, Value>>,
}

impl OutputEnvelopeV3 {
    pub fn new(input: OutputEnvelopeInputV3) -> Result<Self, ProtocolError> {
        let OutputEnvelopeInputV3 {
            request_id,
            command,
            generated_at,
            workspace,
            job,
            session,
            mut result,
            warnings,
        } = input;
        if PROCEDURE_INDEPENDENT_OUTPUT_COMMANDS_V3.contains(&command.as_str()) {
            ensure_procedure_independent_result_schema_v1(command.as_str(), &mut result);
        }
        let output = Self {
            schema: OUTPUT_SCHEMA_V3.to_owned(),
            request_id,
            command,
            generated_at,
            workspace,
            job,
            session,
            result,
            warnings,
        };
        output.validate()?;
        Ok(output)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.schema != OUTPUT_SCHEMA_V3 {
            return Err(ProtocolError::UnsupportedProtocol {
                received: self.schema.clone(),
                supported: SUPPORTED_OUTPUT_SCHEMAS_V3,
            });
        }
        self.request_id.validate()?;
        self.command.validate()?;
        self.generated_at.validate()?;
        if let Some(workspace) = &self.workspace {
            workspace.validate()?;
        }
        if let Some(job) = &self.job {
            job.validate()?;
        }
        if let Some(session) = &self.session {
            session.validate()?;
        }
        if !self.validate_procedure_independent_projection_v1() {
            return Err(ProtocolError::InvalidCommandResult {
                command: self.command.as_str().to_owned(),
            });
        }
        validate_json_map_depth(&self.result, 1)?;
        validate_command_result_v2(self.command.as_str(), &self.result)?;
        if !validate_job_result_output_v2(self) {
            return Err(ProtocolError::InvalidCommandResult {
                command: self.command.as_str().to_owned(),
            });
        }
        if let Some(admission) = self.result.get("admission") {
            match validate_admission_metadata_v1(admission, true)? {
                None if self.job.is_none() => {}
                Some((job_id, sequence))
                    if self
                        .job
                        .as_ref()
                        .is_some_and(|job| job.id() == &job_id && job.sequence() == sequence) => {}
                None | Some(_) => return Err(ProtocolError::InvalidAdmissionMetadata),
            }
        }
        if !validate_v2_output_warnings(&self.warnings) {
            return Err(ProtocolError::InvalidOutputWarnings);
        }
        for warning in &self.warnings {
            validate_json_map_depth(warning, 2)?;
        }
        let envelope_value =
            serde_json::to_value(self).map_err(|_| ProtocolError::InvalidCommandResult {
                command: self.command.as_str().to_owned(),
            })?;
        if !PROCEDURE_INDEPENDENT_OUTPUT_COMMANDS_V3.contains(&self.command.as_str())
            && !validate_embedded_schema_v2(OUTPUT_SCHEMA_V3, &envelope_value)
        {
            return Err(ProtocolError::InvalidCommandResult {
                command: self.command.as_str().to_owned(),
            });
        }
        Ok(())
    }

    fn validate_procedure_independent_projection_v1(&self) -> bool {
        match self.command.as_str() {
            "version" | "daemon.status" => {
                self.workspace.is_none() && self.job.is_none() && self.session.is_none()
            }
            "workspace.init" => {
                self.workspace.is_some() && self.job.is_some() && self.session.is_none()
            }
            _ => true,
        }
    }

    pub fn request_id(&self) -> &RequestIdV1 {
        &self.request_id
    }

    pub fn command(&self) -> &CommandNameV1 {
        &self.command
    }

    pub fn generated_at(&self) -> &Rfc3339MillisV1 {
        &self.generated_at
    }

    pub fn workspace(&self) -> Option<&WorkspaceOutputV1> {
        self.workspace.as_ref()
    }

    pub fn job(&self) -> Option<&JobOutputV1> {
        self.job.as_ref()
    }

    pub fn session(&self) -> Option<&SessionOutputV1> {
        self.session.as_ref()
    }

    pub fn result(&self) -> &Map<String, Value> {
        &self.result
    }

    pub fn warnings(&self) -> &[Map<String, Value>] {
        &self.warnings
    }
}
impl<'de> Deserialize<'de> for OutputEnvelopeV3 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawOutputEnvelopeV3 {
            schema: String,
            request_id: RequestIdV1,
            command: CommandNameV1,
            generated_at: Rfc3339MillisV1,
            #[serde(default)]
            workspace: OptionalField<WorkspaceOutputV1>,
            #[serde(default)]
            job: OptionalField<JobOutputV1>,
            #[serde(default)]
            session: OptionalField<SessionOutputV1>,
            result: Map<String, Value>,
            warnings: Vec<Map<String, Value>>,
        }

        let value = Value::deserialize(deserializer)?;
        validate_json_document_depth(&value).map_err(de::Error::custom)?;
        let raw = RawOutputEnvelopeV3::deserialize(value).map_err(de::Error::custom)?;
        let output = Self {
            schema: raw.schema,
            request_id: raw.request_id,
            command: raw.command,
            generated_at: raw.generated_at,
            workspace: raw.workspace.0,
            job: raw.job.0,
            session: raw.session.0,
            result: raw.result,
            warnings: raw.warnings,
        };
        output.validate().map_err(de::Error::custom)?;
        Ok(output)
    }
}

const VERSION_RESULT_SCHEMA_V1: &str = "podway.version-result/v1";
const DAEMON_STATUS_RESULT_SCHEMA_V1: &str = "podway.daemon-status-result/v1";
const WORKSPACE_INIT_RESULT_SCHEMA_V1: &str = "podway.workspace-init-result/v1";
const DETACHED_ADMISSION_RESULT_SCHEMA_V1: &str = "podway.detached-admission-result/v1";

/// Adds the schema identifier for a command-selected closed result family.
pub fn ensure_procedure_independent_result_schema_v1(
    command: &str,
    result: &mut Map<String, Value>,
) {
    if let Some(schema) = command_result_schema_v1(command, result) {
        result
            .entry("schema".to_owned())
            .or_insert_with(|| Value::String(schema.to_owned()));
    }
}

pub(crate) fn ensure_command_result_schema_v1(command: &str, result: &mut Map<String, Value>) {
    ensure_procedure_independent_result_schema_v1(command, result);
}

/// Validates the closed result family selected by a public command name.
pub fn validate_procedure_independent_result_v1(
    command: &str,
    result: &Map<String, Value>,
) -> Result<(), ProtocolError> {
    if !PROCEDURE_INDEPENDENT_OUTPUT_COMMANDS_V3.contains(&command) {
        return Err(ProtocolError::InvalidCommandResult {
            command: command.to_owned(),
        });
    }
    let expected_schema = command_result_schema_v1(command, result);
    let actual_schema = result.get("schema").and_then(Value::as_str);
    let Some(expected_schema) = expected_schema else {
        return if actual_schema.is_none() {
            Ok(())
        } else {
            Err(ProtocolError::InvalidCommandResult {
                command: command.to_owned(),
            })
        };
    };
    if actual_schema != Some(expected_schema) {
        return Err(ProtocolError::InvalidCommandResult {
            command: command.to_owned(),
        });
    }
    let mut content = result.clone();
    content.remove("schema");
    let value = Value::Object(content);
    let valid = match expected_schema {
        VERSION_RESULT_SCHEMA_V1 => validate_version_result(value),
        DAEMON_STATUS_RESULT_SCHEMA_V1 => {
            validate_daemon_status_result(value.clone())
                || validate_daemon_service_status_result(value)
        }
        WORKSPACE_INIT_RESULT_SCHEMA_V1 => decode::<WorkspaceInitResultV1>(value),
        DETACHED_ADMISSION_RESULT_SCHEMA_V1 => decode::<DetachedMutationResultV1>(value),
        _ => unreachable!("procedure-independent schema selection is closed"),
    };
    if valid {
        Ok(())
    } else {
        Err(ProtocolError::InvalidCommandResult {
            command: command.to_owned(),
        })
    }
}

pub(crate) fn validate_command_result_v1(
    command: &str,
    result: &Map<String, Value>,
) -> Result<(), ProtocolError> {
    validate_procedure_independent_result_v1(command, result)
}

fn command_result_schema_v1(command: &str, result: &Map<String, Value>) -> Option<&'static str> {
    if supports_detached(command)
        && (result.get("detached") == Some(&Value::Bool(true))
            || result.get("schema").and_then(Value::as_str)
                == Some(DETACHED_ADMISSION_RESULT_SCHEMA_V1))
    {
        return Some(DETACHED_ADMISSION_RESULT_SCHEMA_V1);
    }
    match command {
        "version" => Some(VERSION_RESULT_SCHEMA_V1),
        "daemon.status" => Some(DAEMON_STATUS_RESULT_SCHEMA_V1),
        "workspace.init" => Some(WORKSPACE_INIT_RESULT_SCHEMA_V1),
        _ => None,
    }
}

fn supports_detached(command: &str) -> bool {
    command == "workspace.init"
}

fn decode<T: for<'de> Deserialize<'de>>(value: Value) -> bool {
    serde_json::from_value::<T>(value).is_ok()
}

fn non_empty(value: &str) -> bool {
    !value.is_empty()
}

fn optional_absolute_path(path: &Option<String>) -> bool {
    path.as_deref().is_none_or(|path| path.starts_with('/'))
}

fn unique_non_empty_strings(values: &[String]) -> bool {
    values.iter().all(|value| non_empty(value))
        && values
            .iter()
            .enumerate()
            .all(|(index, value)| !values[..index].contains(value))
}

fn validate_version_result(value: Value) -> bool {
    serde_json::from_value::<VersionResultV1>(value).is_ok_and(|result| {
        result.product == "podway"
            && non_empty(&result.version)
            && non_empty(&result.target)
            && result.source_commit.as_deref().is_none_or(non_empty)
            && result.contract_manifest_schema == "podway.contract-manifest/v1"
            && !result.supported_ipc_ids.is_empty()
            && unique_non_empty_strings(&result.supported_ipc_ids)
    })
}

fn validate_daemon_status_result(value: Value) -> bool {
    serde_json::from_value::<DaemonStatusResultV1>(value).is_ok_and(|result| {
        result.product == "podway"
            && non_empty(&result.daemon_version)
            && non_empty(&result.target)
            && result.source_commit.as_deref().is_none_or(non_empty)
            && result.contract_manifest_schema == "podway.contract-manifest/v1"
            && !result.protocol_versions.is_empty()
            && unique_non_empty_strings(&result.protocol_versions)
            && result.pid > 0
            && result.executable_path.starts_with('/')
            && result.configured_socket_path.starts_with('/')
            && result.effective_socket_path.starts_with('/')
    })
}

fn validate_daemon_service_status_result(value: Value) -> bool {
    serde_json::from_value::<DaemonServiceStatusResultV1>(value).is_ok_and(|result| {
        matches!(
            result.status.as_str(),
            "not_installed" | "stopped" | "running"
        ) && result
            .product
            .as_deref()
            .is_none_or(|value| value == "podway")
            && result.daemon_version.as_deref().is_none_or(non_empty)
            && result.target.as_deref().is_none_or(non_empty)
            && result.source_commit.as_deref().is_none_or(non_empty)
            && result
                .contract_manifest_schema
                .as_deref()
                .is_none_or(|value| value == "podway.contract-manifest/v1")
            && unique_non_empty_strings(&result.protocol_versions)
            && result.pid.is_none_or(|pid| pid > 0)
            && optional_absolute_path(&result.executable_path)
            && optional_absolute_path(&result.socket_path)
            && optional_absolute_path(&result.configured_socket_path)
            && optional_absolute_path(&result.effective_socket_path)
            && if result.reachable {
                result.installed
                    && result.loaded
                    && result.product.is_some()
                    && result.daemon_version.is_some()
                    && result.target.is_some()
                    && result.build_identity.is_some()
                    && result.contract_manifest_schema.is_some()
                    && result.contract_manifest_digest.is_some()
                    && !result.protocol_versions.is_empty()
                    && result.pid.is_some()
                    && result.process_id.is_some()
                    && result.executable_path.is_some()
                    && result.started_at.is_some()
                    && result.uptime_ms.is_some()
                    && result.socket_path.is_some()
                    && result.configured_socket_path.is_some()
                    && result.effective_socket_path.is_some()
            } else {
                result.pid.is_none()
                    && result.process_id.is_none()
                    && result.started_at.is_none()
                    && result.uptime_ms.is_none()
                    && result.effective_socket_path.is_none()
            }
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionResultV1 {
    product: String,
    version: String,
    target: String,
    build_identity: Sha256Digest,
    #[serde(deserialize_with = "deserialize_required_option")]
    source_commit: Option<String>,
    contract_manifest_schema: String,
    contract_manifest_digest: Sha256Digest,
    supported_ipc_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DaemonStatusResultV1 {
    product: String,
    daemon_version: String,
    target: String,
    build_identity: Sha256Digest,
    #[serde(deserialize_with = "deserialize_required_option")]
    source_commit: Option<String>,
    contract_manifest_schema: String,
    contract_manifest_digest: Sha256Digest,
    protocol_versions: Vec<String>,
    pid: u32,
    process_id: JobId,
    executable_path: String,
    started_at: Rfc3339MillisV1,
    uptime_ms: u64,
    configured_socket_path: String,
    effective_socket_path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DaemonServiceStatusResultV1 {
    status: String,
    installed: bool,
    loaded: bool,
    reachable: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    product: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    daemon_version: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    target: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    build_identity: Option<Sha256Digest>,
    #[serde(deserialize_with = "deserialize_required_option")]
    source_commit: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    contract_manifest_schema: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    contract_manifest_digest: Option<Sha256Digest>,
    protocol_versions: Vec<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pid: Option<u32>,
    #[serde(deserialize_with = "deserialize_required_option")]
    process_id: Option<JobId>,
    #[serde(deserialize_with = "deserialize_required_option")]
    executable_path: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    started_at: Option<Rfc3339MillisV1>,
    #[serde(deserialize_with = "deserialize_required_option")]
    uptime_ms: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_option")]
    socket_path: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    configured_socket_path: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    effective_socket_path: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    registered_worktree_count: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_option")]
    active_scheduler_count: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_option")]
    queued_job_count: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_option")]
    running_job_count: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionResultV1 {
    #[serde(deserialize_with = "deserialize_true")]
    admitted: bool,
    job_id: JobId,
    #[serde(deserialize_with = "deserialize_positive_u64")]
    workspace_sequence: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceInitResultV1 {
    #[serde(deserialize_with = "deserialize_true")]
    initialized: bool,
    revision: Revision,
    admission: AdmissionResultV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DetachedMutationResultV1 {
    admission: AdmissionResultV1,
    #[serde(deserialize_with = "deserialize_true")]
    detached: bool,
    #[serde(default)]
    procedure_digest: Option<Sha256Digest>,
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn deserialize_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    match bool::deserialize(deserializer)? {
        true => Ok(true),
        false => Err(D::Error::custom("field must be true")),
    }
}

fn deserialize_positive_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    match u64::deserialize(deserializer)? {
        value if value > 0 => Ok(value),
        _ => Err(D::Error::custom("field must be positive")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{build_result_validators_v2, validate_command_result_v1};

    #[test]
    fn embedded_v2_only_result_schemas_compile() {
        build_result_validators_v2().expect("embedded v2-only result schemas must compile");
    }

    const JOB_ID: &str = "00000000-0000-4000-8000-000000000001";
    fn object(value: Value) -> serde_json::Map<String, Value> {
        value.as_object().expect("test result object").clone()
    }

    #[test]
    fn detached_admission_is_closed_and_uses_workspace_sequence() {
        let valid = object(json!({
            "schema": "podway.detached-admission-result/v1",
            "admission": {
                "admitted": true,
                "job_id": JOB_ID,
                "workspace_sequence": 1
            },
            "detached": true
        }));
        assert!(validate_command_result_v1("workspace.init", &valid).is_ok());

        for malformed in [
            json!({
                "schema": "podway.detached-admission-result/v1",
                "admission": {"admitted": true, "job_id": JOB_ID, "sequence": 1},
                "detached": true
            }),
            json!({
                "schema": "podway.detached-admission-result/v1",
                "admission": {
                    "admitted": true,
                    "job_id": JOB_ID,
                    "workspace_sequence": "1"
                },
                "detached": true
            }),
            json!({
                "schema": "podway.detached-admission-result/v1",
                "admission": {
                    "admitted": true,
                    "job_id": JOB_ID,
                    "workspace_sequence": 1,
                    "future": true
                },
                "detached": true
            }),
        ] {
            assert!(validate_command_result_v1("workspace.init", &object(malformed)).is_err());
        }
    }

    #[test]
    fn procedure_v1_result_commands_and_discriminators_are_rejected() {
        for (command, schema) in [
            ("session.start", "podway.session-start-result/v1"),
            ("session.status", "podway.status-result/v1"),
            ("session.status", "podway.compact-status-result/v1"),
            ("session.next", "podway.next-result/v1"),
            ("session.complete", "podway.stage-transition-result/v1"),
            ("item.set", "podway.item-mutation-result/v1"),
            ("job.lookup", "podway.job-lookup-result/v1"),
            ("job.status", "podway.job-result/v1"),
        ] {
            assert!(
                validate_command_result_v1(command, &object(json!({"schema": schema}))).is_err(),
                "removed {schema} must not remain accepted for {command}"
            );
        }

        assert!(validate_command_result_v1("help", &object(json!({"text": "help"}))).is_ok());
        assert!(
            validate_command_result_v1(
                "help",
                &object(json!({"schema": "podway.status-result/v1", "text": "help"}))
            )
            .is_err()
        );
    }
}
