#![allow(dead_code)]

use podway_core::{
    AttemptId, CanonicalProcedureJsonV1, ItemId, JobId, Revision, Sha256Digest, StageId,
    verify_canonical_procedure_document_v1,
};
use serde::{Deserialize, Deserializer, de::Error as _};
use serde_json::{Map, Value};

use crate::{
    CompactStatusResultV1, JobStateV1, NextResultV1, ProtocolError, ResponseEnvelopeV1,
    Rfc3339MillisV1, StatusResultV1,
};

/// A closed result family reserved by the Procedure v2 public contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResultSchemaContractV2 {
    pub schema: &'static str,
    pub schema_path: &'static str,
    pub commands: &'static [&'static str],
}

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
];

/// Result families for v2-only routes. New command surfaces begin at `/v1`.
pub const NEW_ROUTE_RESULT_SCHEMAS_V1: &[ResultSchemaContractV2] = &[
    result_schema_v2(
        "podway.procedure-source-result/v1",
        "schemas/procedure-source-result-v1.schema.json",
        &[
            "procedure.format",
            "procedure.scaffold",
            "procedure.convert",
        ],
    ),
    result_schema_v2(
        "podway.procedure-diagnostics-result/v1",
        "schemas/procedure-diagnostics-result-v1.schema.json",
        &["procedure.vet", "procedure.lint", "procedure.check"],
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
        && result.keys().all(|field| allowed.contains(&field.as_str())))
    .then_some(contract)
}

fn required_result_fields_v2(schema: &str) -> &'static [&'static str] {
    match schema {
        "podway.procedure-validation-result/v2" => {
            &["schema", "file", "procedure_schema", "digest", "valid"]
        }
        "podway.detached-admission-result/v2" => &["schema", "detached", "admission", "job"],
        "podway.session-start-result/v2" => &[
            "schema",
            "procedure_schema",
            "procedure_digest",
            "dry_run",
            "goal_tracking",
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
            "blockers",
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
            "blockers",
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
        "podway.stage-transition-result/v2" => &["schema", "transition", "revision"],
        "podway.item-mutation-result/v2" => &[
            "schema",
            "changed",
            "graph_node_id",
            "attempt_id",
            "attempt_number",
            "item_id",
            "revision",
        ],
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
            "procedure_schema",
            "procedure_digest",
            "entry_graph_node_id",
            "node_count",
            "edge_count",
            "terminal_graph_node_ids",
            "goal_tracking",
        ],
        "podway.decision-result/v1" => &[
            "schema",
            "graph_node_id",
            "attempt_id",
            "attempt_number",
            "option_id",
            "effect",
            "revision",
            "session_state",
            "record",
        ],
        "podway.rework-result/v1" => &[
            "schema",
            "from_graph_node_id",
            "to_graph_node_id",
            "target_attempt_id",
            "reason",
            "reactivated",
            "revision",
        ],
        "podway.goal-definition-result/v1" => &[
            "schema",
            "goal_revision",
            "statement",
            "criteria",
            "recorded_at",
            "revision",
        ],
        "podway.goal-revision-result/v1" => &[
            "schema",
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
            &["schema", "detached", "admission", "job", "procedure_digest"]
        }
        "podway.session-start-result/v2" => &[
            "schema",
            "procedure_schema",
            "procedure_digest",
            "dry_run",
            "goal_tracking",
            "session_id",
            "revision",
            "entry_graph_node_id",
            "goal_required",
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
            "blockers",
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
            "blockers",
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
        "podway.stage-transition-result/v2" => &[
            "schema",
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
            "changed",
            "graph_node_id",
            "attempt_id",
            "attempt_number",
            "item_id",
            "revision",
            "value_digest",
        ],
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
        "podway.decision-result/v1" => &[
            "schema",
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
            "goal_revision",
            "statement",
            "criteria",
            "actor",
            "recorded_at",
            "revision",
        ],
        "podway.goal-revision-result/v1" => &[
            "schema",
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

/// Checks the v2 production bound imposed on the retained open v1 envelope.
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

/// Validates the retained v1 envelope boundary for a v2 result before framing.
pub fn validate_v2_output_envelope_value(output: &Value) -> bool {
    let Some(envelope) = output.as_object() else {
        return false;
    };
    if envelope.get("schema").and_then(Value::as_str) != Some(crate::OUTPUT_SCHEMA_V1) {
        return false;
    }
    let Some(request_id) = envelope.get("request_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(generated_at) = envelope.get("generated_at").and_then(Value::as_str) else {
        return false;
    };
    if crate::RequestIdV1::new(request_id).is_err()
        || crate::Rfc3339MillisV1::new(generated_at).is_err()
    {
        return false;
    }
    let Some(command) = envelope.get("command").and_then(Value::as_str) else {
        return false;
    };
    if crate::CommandNameV1::new(command).is_err() {
        return false;
    }
    let Some(result) = envelope.get("result").and_then(Value::as_object) else {
        return false;
    };
    let Some(contract) = decode_result_schema_contract_v2(result) else {
        return false;
    };
    if !contract.commands.contains(&command) {
        return false;
    }
    let Some(warnings) = envelope.get("warnings").and_then(Value::as_array) else {
        return false;
    };
    let warning_maps: Option<Vec<_>> = warnings
        .iter()
        .map(|warning| warning.as_object().cloned())
        .collect();
    if !warning_maps.is_some_and(|warnings| validate_v2_output_warnings(&warnings)) {
        return false;
    }
    let Some(length) = serde_json::to_vec(output)
        .ok()
        .and_then(|encoded| encoded.len().checked_add(1))
    else {
        return false;
    };
    if length > crate::MAX_FRAME_PAYLOAD_BYTES_V1 {
        return false;
    }
    if contract.schema == "podway.compact-status-result/v2" {
        let Some(queue) = result.get("queue").and_then(Value::as_object) else {
            return false;
        };
        return length <= crate::MAX_COMPACT_STATUS_ENVELOPE_BYTES_V1
            && queue.get("pending_mutations") == Some(&Value::Bool(false))
            && queue.get("queued_count").and_then(Value::as_u64) == Some(0)
            && queue.get("running_job_id") == Some(&Value::Null);
    }
    if contract.schema == "podway.status-result/v2" {
        if !result
            .get("item_values")
            .and_then(|values| serde_json::to_vec(values).ok())
            .is_some_and(|encoded| encoded.len() <= 262_144)
            || !result
                .get("blocker_window")
                .and_then(|values| serde_json::to_vec(values).ok())
                .is_some_and(|encoded| encoded.len() <= 49_152)
        {
            return false;
        }
        if result.get("tier").and_then(Value::as_str) != Some("verbose") {
            return true;
        }
        return [
            "current_trace_history",
            "stale_attempt_history",
            "stale_goal_revision_history",
            "stale_goal_assessment_history",
        ]
        .iter()
        .all(|field| {
            result
                .get(*field)
                .and_then(|window| serde_json::to_vec(window).ok())
                .is_some_and(|encoded| encoded.len() <= 65_536)
        });
    }
    true
}

macro_rules! define_result_schemas_v1 {
    ($($name:ident = $value:literal;)+) => {
        $(const $name: &str = $value;)+

        /// Closed result schema identifiers understood by the v1 runtime decoder.
        pub const SUPPORTED_RESULT_SCHEMAS_V1: &[&str] = &[$($name),+];
    };
}

define_result_schemas_v1! {
    VERSION_RESULT_SCHEMA_V1 = "podway.version-result/v1";
    DAEMON_STATUS_RESULT_SCHEMA_V1 = "podway.daemon-status-result/v1";
    PROCEDURE_VALIDATION_RESULT_SCHEMA_V1 = "podway.procedure-validation-result/v1";
    WORKSPACE_INIT_RESULT_SCHEMA_V1 = "podway.workspace-init-result/v1";
    DETACHED_ADMISSION_RESULT_SCHEMA_V1 = "podway.detached-admission-result/v1";
    SESSION_START_RESULT_SCHEMA_V1 = "podway.session-start-result/v1";
    STATUS_RESULT_SCHEMA_V1 = "podway.status-result/v1";
    COMPACT_STATUS_RESULT_SCHEMA_V1 = "podway.compact-status-result/v1";
    NEXT_RESULT_SCHEMA_V1 = "podway.next-result/v1";
    STAGE_TRANSITION_RESULT_SCHEMA_V1 = "podway.stage-transition-result/v1";
    ITEM_MUTATION_RESULT_SCHEMA_V1 = "podway.item-mutation-result/v1";
    JOB_LOOKUP_RESULT_SCHEMA_V1 = "podway.job-lookup-result/v1";
    JOB_RESULT_SCHEMA_V1 = "podway.job-result/v1";
}

/// Adds the schema identifier for a command-selected closed result family.
pub fn ensure_command_result_schema_v1(command: &str, result: &mut Map<String, Value>) {
    if let Some(schema) = command_result_schema_v1(command, result) {
        result
            .entry("schema".to_owned())
            .or_insert_with(|| Value::String(schema.to_owned()));
    }
}

/// Validates the closed result family selected by a public command name.
pub fn validate_command_result_v1(
    command: &str,
    result: &Map<String, Value>,
) -> Result<(), ProtocolError> {
    if result
        .get("schema")
        .and_then(Value::as_str)
        .is_some_and(|schema| {
            is_known_result_schema(schema) && !schema_allows_command(schema, command)
        })
    {
        return Err(ProtocolError::InvalidCommandResult {
            command: command.to_owned(),
        });
    }
    let Some(expected_schema) = command_result_schema_v1(command, result) else {
        return Ok(());
    };
    if result.get("schema").and_then(Value::as_str) != Some(expected_schema) {
        return Err(ProtocolError::InvalidCommandResult {
            command: command.to_owned(),
        });
    }
    let mut content = result.clone();
    content.remove("schema");
    let value = Value::Object(content);
    let valid = if result.get("detached") == Some(&Value::Bool(true))
        || result.get("schema").and_then(Value::as_str) == Some(DETACHED_ADMISSION_RESULT_SCHEMA_V1)
    {
        if matches!(command, "session.start" | "session.start_replace") {
            decode::<DetachedStartResultV1>(value)
        } else {
            decode::<DetachedMutationResultV1>(value)
        }
    } else {
        match command {
            "version" => validate_version_result(value),
            "daemon.status" => {
                validate_daemon_status_result(value.clone())
                    || validate_daemon_service_status_result(value)
            }
            "procedure.validate" => validate_procedure_validation_result(value),
            "workspace.init" => decode::<WorkspaceInitResultV1>(value),
            "session.status" => {
                if result.contains_key("procedure") {
                    CompactStatusResultV1::from_result_map(result).is_ok()
                } else {
                    StatusResultV1::from_result_map(result).is_ok()
                }
            }
            "session.next" => NextResultV1::from_result_map(result).is_ok(),
            "job.status" | "job.wait" => decode::<JobReadResultV1>(value),
            "job.lookup" => match result.get("found") {
                Some(Value::Bool(false)) => decode::<JobLookupMissingResultV1>(value),
                Some(Value::Bool(true)) => validate_job_lookup_found_result(value),
                _ => false,
            },
            "session.start" | "session.start_replace" => {
                if result.contains_key("dry_run") {
                    validate_start_dry_run_result(value)
                } else {
                    validate_start_terminal_result(value)
                }
            }
            command if is_item_mutation(command) => decode::<ItemMutationResultV1>(value),
            command if is_stage_transition(command) => {
                if result.get("preview") == Some(&Value::Bool(true)) {
                    decode::<StagePreviewResultV1>(value)
                } else if command == "session.reset" && result.contains_key("reset") {
                    validate_reset_result(value)
                } else {
                    decode::<StageTransitionResultV1>(value)
                }
            }
            _ => unreachable!("schema selection and result validation use the same command set"),
        }
    };
    if valid {
        Ok(())
    } else {
        Err(ProtocolError::InvalidCommandResult {
            command: command.to_owned(),
        })
    }
}

fn is_known_result_schema(schema: &str) -> bool {
    SUPPORTED_RESULT_SCHEMAS_V1.contains(&schema)
}

fn schema_allows_command(schema: &str, command: &str) -> bool {
    match schema {
        VERSION_RESULT_SCHEMA_V1 => command == "version",
        DAEMON_STATUS_RESULT_SCHEMA_V1 => command == "daemon.status",
        PROCEDURE_VALIDATION_RESULT_SCHEMA_V1 => command == "procedure.validate",
        WORKSPACE_INIT_RESULT_SCHEMA_V1 => command == "workspace.init",
        DETACHED_ADMISSION_RESULT_SCHEMA_V1 => supports_detached(command),
        SESSION_START_RESULT_SCHEMA_V1 => {
            matches!(command, "session.start" | "session.start_replace")
        }
        STATUS_RESULT_SCHEMA_V1 | COMPACT_STATUS_RESULT_SCHEMA_V1 => command == "session.status",
        NEXT_RESULT_SCHEMA_V1 => command == "session.next",
        STAGE_TRANSITION_RESULT_SCHEMA_V1 => is_stage_transition(command),
        ITEM_MUTATION_RESULT_SCHEMA_V1 => is_item_mutation(command),
        JOB_LOOKUP_RESULT_SCHEMA_V1 => command == "job.lookup",
        JOB_RESULT_SCHEMA_V1 => matches!(command, "job.status" | "job.wait"),
        _ => false,
    }
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
        "procedure.validate" => Some(PROCEDURE_VALIDATION_RESULT_SCHEMA_V1),
        "workspace.init" => Some(WORKSPACE_INIT_RESULT_SCHEMA_V1),
        "session.status" if result.contains_key("procedure") => {
            Some(COMPACT_STATUS_RESULT_SCHEMA_V1)
        }
        "session.status" => Some(STATUS_RESULT_SCHEMA_V1),
        "session.next" => Some(NEXT_RESULT_SCHEMA_V1),
        "job.status" | "job.wait" => Some(JOB_RESULT_SCHEMA_V1),
        "job.lookup" => Some(JOB_LOOKUP_RESULT_SCHEMA_V1),
        "session.start" | "session.start_replace" => Some(SESSION_START_RESULT_SCHEMA_V1),
        command if is_item_mutation(command) => Some(ITEM_MUTATION_RESULT_SCHEMA_V1),
        command if is_stage_transition(command) => Some(STAGE_TRANSITION_RESULT_SCHEMA_V1),
        _ => None,
    }
}

fn supports_detached(command: &str) -> bool {
    command == "workspace.init"
        || matches!(command, "session.start" | "session.start_replace")
        || is_item_mutation(command)
        || is_stage_transition(command)
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

fn validate_procedure_validation_result(value: Value) -> bool {
    serde_json::from_value::<ProcedureValidationResultV1>(value).is_ok_and(|result| {
        let Ok(canonical_json) = CanonicalProcedureJsonV1::new(result.canonical_json) else {
            return false;
        };
        non_empty(&result.file)
            && result.warnings.iter().all(|warning| {
                non_empty(&warning.code) && non_empty(&warning.path) && non_empty(&warning.message)
            })
            && verify_canonical_procedure_document_v1(&canonical_json, &result.digest).is_ok()
            && serde_json::from_str::<Value>(canonical_json.as_str())
                .is_ok_and(|canonical| canonical == Value::Object(result.procedure))
    })
}

fn validate_start_terminal_result(value: Value) -> bool {
    serde_json::from_value::<StartTerminalResultV1>(value)
        .is_ok_and(|result| result.revision_after != Revision::ZERO)
}

fn validate_start_dry_run_result(value: Value) -> bool {
    serde_json::from_value::<StartDryRunResultV1>(value).is_ok_and(|result| {
        non_empty(&result.task)
            && match result.source {
                StartSourceV1::Preset(source) => non_empty(&source.preset),
                StartSourceV1::Procedure(source) => non_empty(&source.procedure),
            }
            && non_empty(&result.first_stage.title)
    })
}

fn validate_reset_result(value: Value) -> bool {
    serde_json::from_value::<ResetResultV1>(value)
        .is_ok_and(|result| result.revision != Revision::ZERO)
}

fn validate_job_lookup_found_result(value: Value) -> bool {
    serde_json::from_value::<JobLookupFoundResultV1>(value)
        .is_ok_and(|result| non_empty(&result.job.command))
}

fn is_item_mutation(command: &str) -> bool {
    matches!(
        command,
        "item.check"
            | "item.uncheck"
            | "item.set"
            | "item.add"
            | "item.remove"
            | "item.attach"
            | "item.clear"
    )
}

fn is_stage_transition(command: &str) -> bool {
    matches!(
        command,
        "session.complete"
            | "session.skip"
            | "session.retry"
            | "session.return"
            | "session.block"
            | "session.unblock"
            | "session.cancel"
            | "session.reopen"
            | "session.reset"
    )
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
struct ProcedureValidationResultV1 {
    file: String,
    digest: Sha256Digest,
    procedure: Map<String, Value>,
    warnings: Vec<ProcedureWarningResultV1>,
    canonical_json: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcedureWarningResultV1 {
    code: String,
    path: String,
    message: String,
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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DetachedStartResultV1 {
    admission: AdmissionResultV1,
    #[serde(deserialize_with = "deserialize_true")]
    detached: bool,
    procedure_digest: Sha256Digest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeResultV1 {
    changed: bool,
    revision_before: Revision,
    revision_after: Revision,
    admission: AdmissionResultV1,
}

type StageTransitionResultV1 = ChangeResultV1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetResultV1 {
    #[serde(deserialize_with = "deserialize_true")]
    reset: bool,
    revision: Revision,
    admission: AdmissionResultV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartTerminalResultV1 {
    changed: bool,
    revision_before: Revision,
    revision_after: Revision,
    procedure_digest: Sha256Digest,
    admission: AdmissionResultV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemMutationResultV1 {
    changed: bool,
    item_id: ItemId,
    revision_before: Revision,
    revision_after: Revision,
    admission: AdmissionResultV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartDryRunResultV1 {
    #[serde(deserialize_with = "deserialize_true")]
    dry_run: bool,
    task: String,
    source: StartSourceV1,
    procedure_digest: Sha256Digest,
    first_stage: FirstStageV1,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StartSourceV1 {
    Preset(PresetSourceV1),
    Procedure(ProcedureSourceV1),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetSourceV1 {
    preset: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcedureSourceV1 {
    procedure: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FirstStageV1 {
    id: StageId,
    title: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActiveAttemptV1 {
    stage_id: StageId,
    attempt_id: AttemptId,
    #[serde(deserialize_with = "deserialize_positive_u32")]
    attempt_number: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StageAttemptV1 {
    attempt_id: AttemptId,
    #[serde(deserialize_with = "deserialize_positive_u32")]
    attempt_number: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AffectedStageV1 {
    stage_id: StageId,
    #[serde(deserialize_with = "deserialize_required_option")]
    before: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    after: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    before_attempt: Option<StageAttemptV1>,
    #[serde(deserialize_with = "deserialize_required_option")]
    after_attempt: Option<StageAttemptV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StagePreviewResultV1 {
    #[serde(deserialize_with = "deserialize_true")]
    preview: bool,
    changed: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    revision_before: Option<Revision>,
    #[serde(deserialize_with = "deserialize_required_option")]
    revision_after: Option<Revision>,
    #[serde(deserialize_with = "deserialize_required_option")]
    active_before: Option<ActiveAttemptV1>,
    #[serde(deserialize_with = "deserialize_required_option")]
    active_after: Option<ActiveAttemptV1>,
    #[serde(deserialize_with = "deserialize_required_option")]
    destination_attempt: Option<ActiveAttemptV1>,
    affected_stages: Vec<AffectedStageV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobReadResultV1 {
    #[serde(deserialize_with = "deserialize_required_option")]
    job: Option<ResponseEnvelopeOrCancellationV1>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ResponseEnvelopeOrCancellationV1 {
    Envelope(Box<ResponseEnvelopeV1>),
    Cancellation(CancelledTerminalV1),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelledTerminalV1 {
    kind: CancelledKindV1,
    payload: CancelledPayloadV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum CancelledKindV1 {
    Cancelled,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelledPayloadV1 {
    #[serde(deserialize_with = "deserialize_true")]
    cancelled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobLookupMissingResultV1 {
    found: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobLookupFoundResultV1 {
    found: bool,
    job: LookupJobV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LookupJobV1 {
    id: JobId,
    #[serde(deserialize_with = "deserialize_positive_u64")]
    sequence: u64,
    state: JobStateV1,
    submitted_at: Rfc3339MillisV1,
    claimed_at: Option<Rfc3339MillisV1>,
    #[serde(deserialize_with = "deserialize_required_option")]
    finished_at: Option<Rfc3339MillisV1>,
    command: String,
    request_digest: Sha256Digest,
    #[serde(deserialize_with = "deserialize_required_option")]
    terminal_response: Option<ResponseEnvelopeOrCancellationV1>,
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

fn deserialize_positive_u32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    match u32::deserialize(deserializer)? {
        value if value > 0 => Ok(value),
        _ => Err(D::Error::custom("field must be positive")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::validate_command_result_v1;

    const JOB_ID: &str = "00000000-0000-4000-8000-000000000001";
    const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

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
        assert!(validate_command_result_v1("item.set", &valid).is_ok());

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
            assert!(validate_command_result_v1("item.set", &object(malformed)).is_err());
        }
    }

    #[test]
    fn start_and_next_reject_missing_wrong_type_and_unknown_fields() {
        let start = object(json!({
            "schema": "podway.detached-admission-result/v1",
            "admission": {
                "admitted": true,
                "job_id": JOB_ID,
                "workspace_sequence": 1
            },
            "detached": true,
            "procedure_digest": DIGEST
        }));
        assert!(validate_command_result_v1("session.start", &start).is_ok());
        let mut missing = start.clone();
        missing.remove("procedure_digest");
        assert!(validate_command_result_v1("session.start", &missing).is_err());

        let mut next = object(json!({
            "schema": "podway.next-result/v1",
            "stage": null,
            "missing_required_items": [],
            "blockers": [],
            "allowed_actions": {
                "complete": false,
                "skip": false,
                "retry": false,
                "return_to": [],
                "cancel": true
            },
            "next_stage_after_completion": null,
            "suggestions": []
        }));
        assert!(validate_command_result_v1("session.next", &next).is_ok());
        next.get_mut("allowed_actions")
            .and_then(Value::as_object_mut)
            .expect("actions")
            .insert("future".to_owned(), Value::Bool(true));
        assert!(validate_command_result_v1("session.next", &next).is_err());
    }

    #[test]
    fn job_read_requires_nullable_field_and_lookup_variants_are_exclusive() {
        assert!(
            validate_command_result_v1(
                "job.status",
                &object(json!({"schema": "podway.job-result/v1", "job": null}))
            )
            .is_ok()
        );
        assert!(
            validate_command_result_v1(
                "job.status",
                &object(json!({"schema": "podway.job-result/v1"}))
            )
            .is_err()
        );
        assert!(
            validate_command_result_v1(
                "job.lookup",
                &object(json!({"schema": "podway.job-lookup-result/v1", "found": false}))
            )
            .is_ok()
        );
        assert!(
            validate_command_result_v1(
                "job.lookup",
                &object(
                    json!({"schema": "podway.job-lookup-result/v1", "found": false, "job": null})
                )
            )
            .is_err()
        );
        assert!(
            validate_command_result_v1(
                "job.lookup",
                &object(json!({"schema": "podway.job-lookup-result/v1", "found": true}))
            )
            .is_err()
        );
    }

    #[test]
    fn result_discriminators_are_required_and_command_specific() {
        let valid = object(json!({
            "schema": "podway.job-lookup-result/v1",
            "found": false
        }));
        assert!(validate_command_result_v1("job.lookup", &valid).is_ok());

        let mut missing = valid.clone();
        missing.remove("schema");
        assert!(validate_command_result_v1("job.lookup", &missing).is_err());

        let mut wrong = valid;
        wrong.insert("schema".to_owned(), json!("podway.job-result/v1"));
        assert!(validate_command_result_v1("job.lookup", &wrong).is_err());
    }
}
