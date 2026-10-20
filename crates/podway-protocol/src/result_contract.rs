use std::{
    collections::HashMap,
    error::Error,
    io,
    sync::{Arc, OnceLock},
};

use jsonschema::{Retrieve, Uri, Validator};
use serde_json::{Map, Value};

use crate::{
    ProtocolError, serialized_json_len_v1, validate_json_document_depth, validate_json_map_depth,
};

mod embedded_schemas {
    include!(concat!(env!("OUT_DIR"), "/embedded_json_schemas.rs"));
}

const EMBEDDED_SCHEMA_BASE_V2: &str = "https://podway.invalid/schemas/";
const MAX_DELEGATION_WORK_PACKAGE_BYTES_V1: usize =
    podway_core::MAX_DELEGATION_WORK_PACKAGE_BYTES_V1;
const MAX_DELEGATION_ACCEPT_INPUT_BYTES_V1: usize = 32_768;
const MAX_DELEGATION_RECEIPT_BYTES_V1: usize = 45_000;
const MAX_DELEGATION_PROJECTION_BYTES_V1: usize = 16_384;
const MAX_DELEGATION_PACKAGE_RESULT_BYTES_V1: usize = 1_000_000;
const MAX_DELEGATION_ACCEPT_RESULT_BYTES_V1: usize = 8192;

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

pub fn validate_delegation_document_v1(schema: &str, value: &Value) -> bool {
    let max_bytes = match schema {
        "podway.delegation-work-package/v1" => MAX_DELEGATION_WORK_PACKAGE_BYTES_V1,
        "podway.delegation-accept-input/v1" => MAX_DELEGATION_ACCEPT_INPUT_BYTES_V1,
        "podway.delegation-receipt/v1" => MAX_DELEGATION_RECEIPT_BYTES_V1,
        _ => return false,
    };
    if !validate_selected_direct_schema_v1(schema, "", value)
        || serialized_json_len_v1(value).map_or(true, |length| length > max_bytes)
    {
        return false;
    }
    let Some(document) = value.as_object() else {
        return false;
    };
    let bounded = |object: &Map<String, Value>, field: &str, limit: usize| {
        object
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| value.len() <= limit)
    };
    match schema {
        "podway.delegation-work-package/v1" => {
            let Some(declaration) = document.get("delegation").and_then(Value::as_object) else {
                return false;
            };
            if !bounded(declaration, "role_ref", 256) {
                return false;
            }
            document
                .get("evidence")
                .and_then(Value::as_array)
                .is_some_and(|entries| {
                    let mut selectors = Vec::new();
                    entries.iter().all(|entry| {
                        entry.get("selector").is_some_and(|selector| {
                            if selectors.contains(&selector) {
                                false
                            } else {
                                selectors.push(selector);
                                entry
                                    .get("items")
                                    .and_then(Value::as_array)
                                    .is_some_and(|items| {
                                        let mut ids = Vec::new();
                                        items.iter().all(|item| {
                                            item.get("item_id").is_some_and(|id| {
                                                if ids.contains(&id) {
                                                    false
                                                } else {
                                                    ids.push(id);
                                                    true
                                                }
                                            })
                                        })
                                    })
                            }
                        })
                    })
                })
        }
        "podway.delegation-accept-input/v1" | "podway.delegation-receipt/v1" => {
            let Some(execution) = document.get("execution").and_then(Value::as_object) else {
                return false;
            };
            if ![
                "scope_ref",
                "supervisor_ref",
                "worker_ref",
                "task_ref",
                "role_ref",
            ]
            .iter()
            .all(|field| bounded(execution, field, 256))
                || !bounded(document, "summary", 4096)
            {
                return false;
            }
            if schema == "podway.delegation-accept-input/v1" {
                bounded(document, "idempotency_key", 256)
                    && document
                        .get("session_revision")
                        .and_then(Value::as_u64)
                        .is_some()
                    && document
                        .get("rounds_used")
                        .and_then(Value::as_u64)
                        .is_some()
            } else {
                let Some(declaration) = document.get("delegation").and_then(Value::as_object)
                else {
                    return false;
                };
                bounded(declaration, "role_ref", 256)
                    && ["binding_ref", "role_ref", "role_snapshot_digest"]
                        .iter()
                        .all(|field| declaration.get(*field) == execution.get(*field))
                    && document
                        .get("rounds_used")
                        .and_then(Value::as_u64)
                        .zip(declaration.get("max_rounds").and_then(Value::as_u64))
                        .is_some_and(|(used, maximum)| used <= maximum)
            }
        }
        _ => false,
    }
}

/// A closed result family selected by the public contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResultSchemaContractV2 {
    pub schema: &'static str,
    pub schema_path: &'static str,
    pub commands: &'static [&'static str],
}

pub const EXISTING_ROUTE_RESULT_SCHEMAS_V2: &[ResultSchemaContractV2] = &[result_schema_v2(
    "podway.procedure-validation-result/v2",
    "schemas/procedure-validation-result-v2.schema.json",
    &["procedure.validate"],
)];
pub const PROCEDURE_DIAGNOSTICS_RESULT_SCHEMA_V1: &str = "podway.procedure-diagnostics-result/v1";
pub const NEW_ROUTE_RESULT_SCHEMAS_V1: &[ResultSchemaContractV2] = &[
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
        "podway.delegation-package-result/v1",
        "schemas/delegation-package-result-v1.schema.json",
        &["delegation.package"],
    ),
    result_schema_v2(
        "podway.delegation-receipt-result/v1",
        "schemas/delegation-receipt-result-v1.schema.json",
        &["delegation.receipt"],
    ),
    result_schema_v2(
        "podway.procedure-graph-result/v2",
        "schemas/procedure-graph-result-v2.schema.json",
        &["procedure.graph"],
    ),
    result_schema_v2(
        "podway.procedure-preview-result/v2",
        "schemas/procedure-preview-result-v2.schema.json",
        &["procedure.preview"],
    ),
    result_schema_v2(
        "podway.evidence-read-result/v1",
        "schemas/evidence-read-result-v1.schema.json",
        &["evidence.read"],
    ),
    result_schema_v2(
        "podway.procedure-source-result/v1",
        "schemas/procedure-source-result-v1.schema.json",
        &["procedure.format", "procedure.scaffold"],
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
];
const DELEGATION_DOCUMENT_SCHEMAS_V1: &[ResultSchemaContractV2] = &[
    result_schema_v2(
        "podway.delegation-accept-input/v1",
        "schemas/delegation-accept-input-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.delegation-work-package/v1",
        "schemas/delegation-work-package-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.delegation-receipt/v1",
        "schemas/delegation-receipt-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.delegation-projection/v1",
        "schemas/delegation-projection-v1.schema.json",
        &[],
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

pub const DAEMON_FREE_DOCUMENT_SCHEMAS_V1: &[ResultSchemaContractV2] = &[
    result_schema_v2(
        "podway.session-archive-limit-details/v2",
        "schemas/session-archive-limit-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.attempt-conflict-details/v3",
        "schemas/attempt-conflict-details-v3.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.blocker-limit-details/v2",
        "schemas/blocker-limit-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.command-retired-details/v1",
        "schemas/command-retired-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.compact-status-result/v5",
        "schemas/compact-status-result-v5.schema.json",
        &["session.status"],
    ),
    result_schema_v2(
        "podway.contract-manifest/v2",
        "schemas/contract-manifest-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.criterion-assessment-result/v2",
        "schemas/criterion-assessment-result-v2.schema.json",
        &["goal.assess_criterion"],
    ),
    result_schema_v2(
        "podway.decision-result/v2",
        "schemas/decision-result-v2.schema.json",
        &["session.decide"],
    ),
    result_schema_v2(
        "podway.delegation-accept-result/v2",
        "schemas/delegation-accept-result-v2.schema.json",
        &["delegation.accept"],
    ),
    result_schema_v2(
        "podway.delegation-error-details/v2",
        "schemas/delegation-error-details-v2.schema.json",
        &[],
    ),
    result_schema_v2("podway.error/v2", "schemas/error-v2.schema.json", &[]),
    result_schema_v2(
        "podway.goal-definition-result/v2",
        "schemas/goal-definition-result-v2.schema.json",
        &["goal.define"],
    ),
    result_schema_v2(
        "podway.goal-revision-result/v2",
        "schemas/goal-revision-result-v2.schema.json",
        &["goal.revise"],
    ),
    result_schema_v2(
        "podway.idempotency-key-reused-details/v2",
        "schemas/idempotency-key-reused-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.internal-error-details/v2",
        "schemas/internal-error-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.item-mutation-result/v3",
        "schemas/item-mutation-result-v3.schema.json",
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
        "podway.item-record-many-result/v2",
        "schemas/item-record-many-result-v2.schema.json",
        &["item.record_many"],
    ),
    result_schema_v2(
        "podway.legacy-runtime-state-unsupported-details/v1",
        "schemas/legacy-runtime-state-unsupported-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.mutation-outcome-unknown-details/v3",
        "schemas/mutation-outcome-unknown-details-v3.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.next-result/v5",
        "schemas/next-result-v5.schema.json",
        &["session.next"],
    ),
    result_schema_v2(
        "podway.observation-result/v5",
        "schemas/observation-result-v5.schema.json",
        &["session.observe"],
    ),
    result_schema_v2(
        "podway.outcome-lookup-result/v1",
        "schemas/outcome-lookup-result-v1.schema.json",
        &["outcome.lookup"],
    ),
    result_schema_v2("podway.output/v4", "schemas/output-v4.schema.json", &[]),
    result_schema_v2(
        "podway.prepared-next-result/v3",
        "schemas/prepared-next-result-v3.schema.json",
        &["session.next"],
    ),
    result_schema_v2(
        "podway.procedure-digest-mismatch-details/v3",
        "schemas/procedure-digest-mismatch-details-v3.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.recoverable-v2-runtime-error-details/v2",
        "schemas/recoverable-v2-runtime-error-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.recovery-recipe/v2",
        "schemas/recovery-recipe-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.revision-conflict-details/v3",
        "schemas/revision-conflict-details-v3.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.rework-result/v2",
        "schemas/rework-result-v2.schema.json",
        &["session.rework"],
    ),
    result_schema_v2(
        "podway.session-archive-list-result/v2",
        "schemas/session-archive-list-result-v2.schema.json",
        &["session.archive_list"],
    ),
    result_schema_v2(
        "podway.session-archive-not-eligible-details/v2",
        "schemas/session-archive-not-eligible-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.session-archive-not-found-details/v2",
        "schemas/session-archive-not-found-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.session-archive-purge-result/v1",
        "schemas/session-archive-purge-result-v1.schema.json",
        &["session.archive_purge"],
    ),
    result_schema_v2(
        "podway.session-archive-result/v2",
        "schemas/session-archive-result-v2.schema.json",
        &["session.archive"],
    ),
    result_schema_v2(
        "podway.session-archive-show-result/v3",
        "schemas/session-archive-show-result-v3.schema.json",
        &["session.archive_show"],
    ),
    result_schema_v2(
        "podway.session-begin-result/v2",
        "schemas/session-begin-result-v2.schema.json",
        &["session.begin"],
    ),
    result_schema_v2(
        "podway.session-id-mismatch-details/v3",
        "schemas/session-id-mismatch-details-v3.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.session-reset-not-eligible-details/v2",
        "schemas/session-reset-not-eligible-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.session-reset-result/v2",
        "schemas/session-reset-result-v2.schema.json",
        &["session.reset"],
    ),
    result_schema_v2(
        "podway.session-start-decision-required-details/v2",
        "schemas/session-start-decision-required-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.session-start-result/v4",
        "schemas/session-start-result-v4.schema.json",
        &["session.start", "session.start_replace"],
    ),
    result_schema_v2(
        "podway.session-start-state-conflict-details/v2",
        "schemas/session-start-state-conflict-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.stage-transition-result/v3",
        "schemas/stage-transition-result-v3.schema.json",
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
        "podway.status-result/v5",
        "schemas/status-result-v5.schema.json",
        &["session.status"],
    ),
    result_schema_v2(
        "podway.terminal-disposition-result/v2",
        "schemas/terminal-disposition-result-v2.schema.json",
        &["session.terminal_disposition"],
    ),
    result_schema_v2(
        "podway.v2-runtime-error-details/v2",
        "schemas/v2-runtime-error-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.version-result/v2",
        "schemas/version-result-v2.schema.json",
        &["version"],
    ),
    result_schema_v2(
        "podway.workspace-busy-details/v1",
        "schemas/workspace-busy-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.workspace-contract-mismatch-details/v1",
        "schemas/workspace-contract-mismatch-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.workspace-init-result/v2",
        "schemas/workspace-init-result-v2.schema.json",
        &["workspace.init"],
    ),
    result_schema_v2(
        "podway.workspace-maintenance-details/v1",
        "schemas/workspace-maintenance-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.workspace-maintenance-intent-marker/v1",
        "schemas/workspace-maintenance-intent-marker-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.workspace-recovery-details/v2",
        "schemas/workspace-recovery-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.workspace-removal-marker/v2",
        "schemas/workspace-removal-marker-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.workspace-removal-result/v2",
        "schemas/workspace-removal-result-v2.schema.json",
        &["workspace.remove"],
    ),
    result_schema_v2(
        "podway.workspace-reset-marker/v1",
        "schemas/workspace-reset-marker-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.workspace-schema-unsupported-details/v1",
        "schemas/workspace-schema-unsupported-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.workspace-uuid-mismatch-details/v3",
        "schemas/workspace-uuid-mismatch-details-v3.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.writer-predecessor-declaration/v1",
        "schemas/writer-predecessor-declaration-v1.schema.json",
        &[],
    ),
];

pub fn decode_result_schema_contract_v2(result: &Value) -> Option<&'static ResultSchemaContractV2> {
    let object = result.as_object()?;
    let schema = object.get("schema")?.as_str()?;
    let contract = current_result_contracts_v2().find(|contract| contract.schema == schema)?;
    (validate_json_map_depth(object, 1).is_ok()
        && validate_selected_direct_schema_v1(schema, "", result)
        && validate_result_correlations_v2(schema, object))
    .then_some(contract)
}
fn current_result_contracts_v2() -> impl Iterator<Item = &'static ResultSchemaContractV2> {
    EXISTING_ROUTE_RESULT_SCHEMAS_V2
        .iter()
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
        .chain(
            DAEMON_FREE_DOCUMENT_SCHEMAS_V1
                .iter()
                .filter(|contract| !contract.commands.is_empty()),
        )
}
pub fn result_schema_top_level_fields_v2(
    schema: &str,
) -> Option<(&'static [&'static str], &'static [&'static str])> {
    match schema {
        "podway.procedure-diagnostics-result/v1" => Some((
            &[
                "schema",
                "operation",
                "procedure_schema",
                "file",
                "valid",
                "diagnostics",
                "diagnostics_truncated",
                "diagnostics_total",
            ],
            &[
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
        )),
        "podway.procedure-validation-result/v2" => Some((
            &["schema", "file", "procedure_schema", "digest", "valid"],
            &["schema", "file", "procedure_schema", "digest", "valid"],
        )),
        "podway.delegation-package-result/v1" => Some((
            &[
                "schema",
                "package",
                "package_digest",
                "workspace_uuid",
                "session_id",
                "session_revision",
                "attempt_id",
            ],
            &[
                "schema",
                "package",
                "package_digest",
                "workspace_uuid",
                "session_id",
                "session_revision",
                "attempt_id",
            ],
        )),
        "podway.delegation-receipt-result/v1" => Some((
            &[
                "schema",
                "session_id",
                "attempt_id",
                "receipt",
                "applicability",
                "reason",
            ],
            &[
                "schema",
                "session_id",
                "attempt_id",
                "receipt",
                "applicability",
                "reason",
            ],
        )),
        "podway.procedure-graph-result/v2" => Some((
            &[
                "schema",
                "procedure_schema",
                "procedure_digest",
                "format",
                "projection_digest",
                "projection",
                "delegated_placement_count",
                "delegated_placements",
            ],
            &[
                "schema",
                "procedure_schema",
                "procedure_digest",
                "format",
                "projection_digest",
                "projection",
                "delegated_placement_count",
                "delegated_placements",
            ],
        )),
        "podway.procedure-preview-result/v2" => Some((
            &[
                "schema",
                "file",
                "admissible",
                "checks",
                "diagnostics",
                "diagnostics_truncated",
                "diagnostics_total",
                "delegated_placement_count",
                "delegated_placements",
            ],
            &[
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
                "delegated_placement_count",
                "delegated_placements",
            ],
        )),
        "podway.evidence-read-result/v1" => Some((
            &[
                "schema",
                "procedure_schema",
                "consumer",
                "source",
                "item",
                "value_digest",
                "total_size",
                "size_unit",
                "page",
                "truncated",
                "next_page_token",
                "page_token_version",
            ],
            &[
                "schema",
                "procedure_schema",
                "consumer",
                "source",
                "item",
                "value_digest",
                "total_size",
                "size_unit",
                "page",
                "truncated",
                "next_page_token",
                "page_token_version",
            ],
        )),
        "podway.procedure-source-result/v1" => Some((
            &[
                "schema",
                "operation",
                "target_schema",
                "document",
                "target_digest",
            ],
            &[
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
        )),
        "podway.procedure-graph-result/v1" => Some((
            &[
                "schema",
                "procedure_schema",
                "procedure_digest",
                "format",
                "projection_digest",
                "projection",
            ],
            &[
                "schema",
                "procedure_schema",
                "procedure_digest",
                "format",
                "projection_digest",
                "projection",
            ],
        )),
        "podway.procedure-preview-result/v1" => Some((
            &[
                "schema",
                "file",
                "admissible",
                "checks",
                "diagnostics",
                "diagnostics_truncated",
                "diagnostics_total",
            ],
            &[
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
        )),
        "podway.compact-status-result/v5" => Some((
            &[
                "schema",
                "procedure",
                "session",
                "current",
                "goal_tracking",
                "goal_defined",
                "trace_length",
                "counters",
                "items",
            ],
            &[
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
                "delegation",
            ],
        )),
        "podway.criterion-assessment-result/v2" => Some((
            &[
                "schema",
                "graph_node_id",
                "attempt_id",
                "goal_revision",
                "mode",
                "result",
                "complete",
                "revision",
            ],
            &[
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
        )),
        "podway.decision-result/v2" => Some((
            &[
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
            &[
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
        )),
        "podway.delegation-accept-result/v2" => Some((
            &[
                "schema",
                "receipt_id",
                "workspace_uuid",
                "session_id",
                "session_revision",
                "attempt_id",
                "package_digest",
                "result_digest",
                "acceptance_state",
            ],
            &[
                "schema",
                "receipt_id",
                "workspace_uuid",
                "session_id",
                "session_revision",
                "attempt_id",
                "package_digest",
                "result_digest",
                "acceptance_state",
            ],
        )),
        "podway.goal-definition-result/v2" => Some((
            &[
                "schema",
                "goal_revision",
                "statement",
                "criteria",
                "recorded_at",
                "revision",
            ],
            &[
                "schema",
                "goal_revision",
                "statement",
                "criteria",
                "actor",
                "recorded_at",
                "revision",
            ],
        )),
        "podway.goal-revision-result/v2" => Some((
            &[
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
            &[
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
        )),
        "podway.item-mutation-result/v3" => Some((
            &[
                "schema",
                "changed",
                "graph_node_id",
                "attempt_id",
                "attempt_number",
                "item_id",
                "revision",
            ],
            &[
                "schema",
                "changed",
                "graph_node_id",
                "attempt_id",
                "attempt_number",
                "item_id",
                "revision",
                "value_digest",
            ],
        )),
        "podway.item-record-many-result/v2" => Some((
            &[
                "schema",
                "changed",
                "graph_node_id",
                "attempt_id",
                "attempt_number",
                "revision",
                "items",
            ],
            &[
                "schema",
                "changed",
                "graph_node_id",
                "attempt_id",
                "attempt_number",
                "revision",
                "items",
            ],
        )),
        "podway.next-result/v5" => Some((
            &[
                "schema",
                "procedure_schema",
                "procedure_digest",
                "goal_tracking",
                "goal_defined",
                "node",
                "attempt",
                "trace_length",
                "counters",
                "revision",
                "readiness",
                "missing_required_item_count",
                "blockers_total",
                "allowed_actions",
                "suggestions",
                "suggestions_total",
                "suggestions_truncated",
                "references",
                "readback",
                "readback_items_total",
                "readback_items_truncated",
                "allowed_manual_rework_targets",
            ],
            &[
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
                "missing_required_items_truncated",
                "options",
                "allowed_option_ids",
                "option_guard_statuses",
                "evidence_guidance",
                "next_graph_node_id",
                "terminal",
                "skip",
                "allowed_manual_rework_targets",
                "allowed_actions",
                "suggestions",
                "suggestions_total",
                "suggestions_truncated",
                "references",
                "readback",
                "readback_items_total",
                "readback_items_truncated",
                "blockers_total",
                "blockers",
                "blockers_truncated",
                "delegation",
            ],
        )),
        "podway.observation-result/v5" => Some((
            &[
                "schema",
                "status",
                "guidance",
                "active_items",
                "active_items_total",
                "active_items_truncated",
                "mutation_templates",
                "mutation_templates_total",
                "mutation_templates_truncated",
            ],
            &[
                "schema",
                "status",
                "guidance",
                "active_items",
                "active_items_total",
                "active_items_truncated",
                "mutation_templates",
                "mutation_templates_total",
                "mutation_templates_truncated",
            ],
        )),
        "podway.outcome-lookup-result/v1" => Some((
            &["schema", "idempotency_key", "found", "outcome"],
            &["schema", "idempotency_key", "found", "outcome"],
        )),
        "podway.prepared-next-result/v3" => Some((
            &[
                "schema",
                "procedure_schema",
                "procedure_digest",
                "session_id",
                "session_state",
                "revision",
                "goal_tracking",
                "goal_defined",
                "trace_length",
                "allowed_actions",
                "suggestions",
            ],
            &[
                "schema",
                "procedure_schema",
                "procedure_digest",
                "session_id",
                "session_state",
                "revision",
                "goal_tracking",
                "goal_defined",
                "trace_length",
                "allowed_actions",
                "suggestions",
                "delegated_placement_count",
            ],
        )),
        "podway.rework-result/v2" => Some((
            &[
                "schema",
                "from_graph_node_id",
                "to_graph_node_id",
                "target_attempt_id",
                "reason",
                "reactivated",
                "revision",
            ],
            &[
                "schema",
                "from_graph_node_id",
                "to_graph_node_id",
                "target_attempt_id",
                "reason",
                "reactivated",
                "revision",
            ],
        )),
        "podway.session-archive-result/v2" => Some((
            &[
                "schema",
                "session_id",
                "session_revision",
                "activity",
                "lifecycle",
            ],
            &[
                "schema",
                "session_id",
                "session_revision",
                "activity",
                "lifecycle",
            ],
        )),
        "podway.session-archive-show-result/v3" => Some((
            &[
                "schema",
                "activity",
                "archive",
                "terminal_disposition",
                "state",
            ],
            &[
                "schema",
                "activity",
                "archive",
                "terminal_disposition",
                "state",
                "receipt_summaries",
                "receipt_summaries_total",
                "receipt_summaries_truncated",
            ],
        )),
        "podway.session-begin-result/v2" => Some((
            &[
                "schema",
                "procedure_schema",
                "procedure_digest",
                "session_id",
                "revision",
                "session_state",
                "entry_graph_node_id",
                "attempt",
                "goal_tracking",
                "goal_defined",
                "goal_revision",
            ],
            &[
                "schema",
                "procedure_schema",
                "procedure_digest",
                "session_id",
                "revision",
                "session_state",
                "entry_graph_node_id",
                "attempt",
                "goal_tracking",
                "goal_defined",
                "goal_revision",
            ],
        )),
        "podway.session-reset-result/v2" => Some((
            &[
                "schema",
                "dry_run",
                "mode",
                "session_id",
                "session_revision",
                "lifecycle",
                "current_terminal_disposition",
                "eligible",
                "required_action",
                "reset",
            ],
            &[
                "schema",
                "dry_run",
                "mode",
                "session_id",
                "session_revision",
                "lifecycle",
                "current_terminal_disposition",
                "eligible",
                "required_action",
                "reset",
            ],
        )),
        "podway.session-start-result/v4" => Some((
            &[
                "schema",
                "procedure_schema",
                "procedure_digest",
                "dry_run",
                "session_state",
                "goal_tracking",
                "goal_defined",
                "active_attempt",
                "goal_revision",
            ],
            &[
                "schema",
                "procedure_schema",
                "procedure_digest",
                "dry_run",
                "session_state",
                "goal_tracking",
                "goal_defined",
                "active_attempt",
                "goal_revision",
                "entry_graph_node_id",
                "session_id",
                "revision",
                "existing_session",
            ],
        )),
        "podway.stage-transition-result/v3" => Some((
            &["schema", "transition", "revision"],
            &[
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
        )),
        "podway.status-result/v5" => Some((
            &[
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
            &[
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
                "delegation",
                "receipt_summaries",
                "receipt_summaries_total",
                "receipt_summaries_truncated",
            ],
        )),
        "podway.terminal-disposition-result/v2" => Some((
            &["schema", "session_id", "session_revision", "disposition"],
            &["schema", "session_id", "session_revision", "disposition"],
        )),
        "podway.version-result/v2" => Some((
            &[
                "schema",
                "product",
                "version",
                "target",
                "build_identity",
                "source_commit",
                "contract_manifest_schema",
                "contract_manifest_digest",
            ],
            &[
                "schema",
                "product",
                "version",
                "target",
                "build_identity",
                "source_commit",
                "contract_manifest_schema",
                "contract_manifest_digest",
            ],
        )),
        "podway.workspace-init-result/v2" => Some((
            &["schema", "initialized", "revision"],
            &["schema", "initialized", "revision"],
        )),
        "podway.workspace-removal-result/v2" => Some((
            &[
                "schema",
                "worktree_root",
                "workspace_uuid",
                "podway_directory_removed",
                "already_absent",
            ],
            &[
                "schema",
                "worktree_root",
                "workspace_uuid",
                "podway_directory_removed",
                "already_absent",
            ],
        )),
        _ => None,
    }
}
fn validate_result_correlations_v2(schema: &str, result: &Map<String, Value>) -> bool {
    match schema {
        "podway.terminal-disposition-result/v2" => result
            .get("session_revision")
            .zip(
                result
                    .get("disposition")
                    .and_then(Value::as_object)
                    .and_then(|value| value.get("session_revision")),
            )
            .is_some_and(|(session, disposition)| session == disposition),
        "podway.delegation-package-result/v1" => {
            let Some(package_value) = result.get("package") else {
                return false;
            };
            let Some(package) = package_value.as_object() else {
                return false;
            };
            result_json_within_bound_v2(result, MAX_DELEGATION_PACKAGE_RESULT_BYTES_V1)
                && validate_delegation_document_v1(
                    "podway.delegation-work-package/v1",
                    package_value,
                )
                && ["workspace_uuid", "session_id", "attempt_id"]
                    .iter()
                    .all(|field| result.get(*field) == package.get(*field))
        }
        "podway.delegation-receipt-result/v1" => result
            .get("receipt")
            .filter(|receipt| !receipt.is_null())
            .is_some_and(|receipt| {
                validate_delegation_document_v1("podway.delegation-receipt/v1", receipt)
                    && ["session_id", "attempt_id"]
                        .iter()
                        .all(|field| result.get(*field) == receipt.get(*field))
            }),
        "podway.delegation-accept-result/v2" => {
            result_json_within_bound_v2(result, MAX_DELEGATION_ACCEPT_RESULT_BYTES_V1)
        }
        "podway.status-result/v5" | "podway.compact-status-result/v5" => {
            delegation_projection_matches_current_attempt_v2(result)
        }
        "podway.next-result/v5" => {
            let attempt = result
                .get("attempt")
                .and_then(|value| value.get("attempt_id"));
            result.get("delegation").is_none_or(|projection| {
                delegation_projection_matches_attempt_v2(projection, attempt)
            })
        }
        "podway.observation-result/v5" => result
            .get("status")
            .and_then(Value::as_object)
            .is_some_and(delegation_projection_matches_current_attempt_v2),
        "podway.session-archive-show-result/v3" => result
            .get("state")
            .and_then(Value::as_object)
            .is_some_and(delegation_projection_matches_current_attempt_v2),
        "podway.decision-result/v2" => {
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
        _ => true,
    }
}

fn result_json_within_bound_v2(result: &Map<String, Value>, limit: usize) -> bool {
    serialized_json_len_v1(result).is_ok_and(|length| length <= limit)
}

fn delegation_projection_matches_current_attempt_v2(result: &Map<String, Value>) -> bool {
    let attempt = result
        .get("current")
        .and_then(|current| current.get("attempt"))
        .and_then(|attempt| attempt.get("attempt_id"));
    result
        .get("delegation")
        .is_none_or(|projection| delegation_projection_matches_attempt_v2(projection, attempt))
}

fn delegation_projection_matches_attempt_v2(projection: &Value, attempt: Option<&Value>) -> bool {
    let Some(state) = projection.get("state").and_then(Value::as_str) else {
        return false;
    };
    if !serialized_json_len_v1(projection)
        .is_ok_and(|length| length <= MAX_DELEGATION_PROJECTION_BYTES_V1)
    {
        return false;
    }
    match attempt {
        Some(attempt_id) => {
            state != "inactive"
                && projection
                    .get("receipt")
                    .filter(|receipt| !receipt.is_null())
                    .is_none_or(|receipt| receipt.get("attempt_id") == Some(attempt_id))
        }
        None => state == "inactive",
    }
}

pub const MAX_V2_OUTPUT_WARNINGS: usize = 4;
pub const MAX_V2_WARNING_CODE_CHARS: usize = 64;
pub const MAX_V2_WARNING_PATH_CHARS: usize = 256;
pub const MAX_V2_WARNING_MESSAGE_CHARS: usize = 512;

const PROCEDURE_INDEPENDENT_OUTPUT_COMMANDS_V1: &[&str] = &[
    "help",
    "version",
    "completions",
    "preset.list",
    "preset.show",
    "preset.explain",
    "procedure.show",
    "__complete",
];
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

pub fn validate_command_result_v2(command: &str, result: &Value) -> Result<(), ProtocolError> {
    if decode_result_schema_contract_v2(result)
        .is_some_and(|contract| contract.commands.contains(&command))
        || (PROCEDURE_INDEPENDENT_OUTPUT_COMMANDS_V1.contains(&command)
            && validate_procedure_independent_result_v1(command, result).is_ok())
    {
        Ok(())
    } else {
        Err(ProtocolError::InvalidCommandResult {
            command: command.to_owned(),
        })
    }
}
pub fn ensure_procedure_independent_result_schema_v1(
    command: &str,
    result: &mut Map<String, Value>,
) {
    if command == "version" {
        result
            .entry("schema".to_owned())
            .or_insert_with(|| Value::String("podway.version-result/v2".to_owned()));
    }
}
pub fn validate_procedure_independent_result_v1(
    command: &str,
    result: &Value,
) -> Result<(), ProtocolError> {
    let valid = if command == "version" {
        validate_daemon_free_document_v1("podway.version-result/v2", result)
    } else {
        PROCEDURE_INDEPENDENT_OUTPUT_COMMANDS_V1.contains(&command)
            && result
                .as_object()
                .is_some_and(|object| !object.contains_key("schema"))
    };
    if valid {
        Ok(())
    } else {
        Err(ProtocolError::InvalidCommandResult {
            command: command.to_owned(),
        })
    }
}
/// Preserved input and output budgets, independent of IPC framing.
pub const MAX_DIRECT_INPUT_BYTES_V1: usize = 1_048_576;
pub const MAX_DIRECT_OUTPUT_BYTES_V1: usize = 1_048_576;

type DirectValidatorCellV1 = OnceLock<Result<Validator, String>>;
type DirectValidatorRegistryV1 = HashMap<(String, String), DirectValidatorCellV1>;

static DAEMON_FREE_VALIDATORS_V1: OnceLock<Result<DirectValidatorRegistryV1, String>> =
    OnceLock::new();
static DIRECT_SCHEMA_RESOURCES_V1: OnceLock<Result<EmbeddedSchemaRetrieverV2, String>> =
    OnceLock::new();

fn direct_document_contracts_v1() -> impl Iterator<Item = &'static ResultSchemaContractV2> {
    DAEMON_FREE_DOCUMENT_SCHEMAS_V1
        .iter()
        .chain(EXISTING_ROUTE_RESULT_SCHEMAS_V2)
        .chain(NEW_ROUTE_RESULT_SCHEMAS_V1)
        .chain(DELEGATION_DOCUMENT_SCHEMAS_V1)
}
fn direct_schema_source_v1(contract: &ResultSchemaContractV2) -> Result<Value, String> {
    let filename = contract
        .schema_path
        .strip_prefix("schemas/")
        .ok_or_else(|| "invalid daemon-free schema path".to_owned())?;
    let source = embedded_schemas::EMBEDDED_JSON_SCHEMAS_V1
        .iter()
        .find(|(_, name, _)| *name == filename)
        .ok_or_else(|| format!("missing daemon-free schema {filename}"))?
        .2;
    serde_json::from_str(source).map_err(|error| error.to_string())
}

fn direct_command_matches_v1(branch: &Value, command: &str) -> bool {
    let constraint = &branch["properties"]["command"];
    constraint["const"].as_str() == Some(command)
        || constraint["enum"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(command)))
}

fn direct_validator_registry_v1() -> Result<DirectValidatorRegistryV1, String> {
    let mut registry = HashMap::new();
    for contract in direct_document_contracts_v1() {
        registry.insert((contract.schema.to_owned(), String::new()), OnceLock::new());
        // The finite keys come from the canonical schemas, never from a caller.
        // Each invocation compiles only the applicable command or error family.
        if contract.schema == "podway.output/v4" {
            let source = direct_schema_source_v1(contract)?;
            let definitions = source["$defs"]
                .as_object()
                .ok_or_else(|| "missing output definitions".to_owned())?;
            for branch in definitions.values() {
                let constraint = &branch["properties"]["command"];
                if let Some(command) = constraint["const"].as_str() {
                    registry.insert(
                        (contract.schema.to_owned(), command.to_owned()),
                        OnceLock::new(),
                    );
                }
                if let Some(commands) = constraint["enum"].as_array() {
                    for command in commands {
                        let command = command
                            .as_str()
                            .ok_or_else(|| "invalid output command".to_owned())?;
                        registry.insert(
                            (contract.schema.to_owned(), command.to_owned()),
                            OnceLock::new(),
                        );
                    }
                }
            }
        } else if contract.schema == "podway.error/v2" {
            let source = direct_schema_source_v1(contract)?;
            let codes = source["properties"]["code"]["enum"]
                .as_array()
                .ok_or_else(|| "missing direct error catalog".to_owned())?;
            for code in codes {
                let code = code
                    .as_str()
                    .ok_or_else(|| "invalid error code".to_owned())?;
                registry.insert(
                    (contract.schema.to_owned(), code.to_owned()),
                    OnceLock::new(),
                );
            }
        }
    }
    Ok(registry)
}

fn build_direct_validator_v1(schema_id: &str, discriminator: &str) -> Result<Validator, String> {
    let contract = direct_document_contracts_v1()
        .find(|contract| contract.schema == schema_id)
        .ok_or_else(|| "unknown direct schema".to_owned())?;
    let filename = contract
        .schema_path
        .strip_prefix("schemas/")
        .ok_or_else(|| "invalid daemon-free schema path".to_owned())?;
    let mut schema = direct_schema_source_v1(contract)?;
    if schema_id == "podway.output/v4" && !discriminator.is_empty() {
        let branches = schema["oneOf"]
            .as_array()
            .ok_or_else(|| "missing output branches".to_owned())?
            .iter()
            .filter(|branch| {
                branch["$ref"]
                    .as_str()
                    .and_then(|reference| reference.strip_prefix('#'))
                    .and_then(|pointer| schema.pointer(pointer))
                    .is_some_and(|branch| direct_command_matches_v1(branch, discriminator))
            })
            .cloned()
            .collect::<Vec<_>>();
        if branches.is_empty() {
            return Err("unknown direct output command".to_owned());
        }
        // Removed alternatives have a disjoint canonical command constraint.
        // Keep every matching alternative, including status and reset variants.
        schema["oneOf"] = Value::Array(branches);
    } else if schema_id == "podway.error/v2" && !discriminator.is_empty() {
        let rules = schema["allOf"]
            .as_array()
            .ok_or_else(|| "missing error rules".to_owned())?
            .iter()
            .filter(|rule| {
                rule["if"]["properties"]["code"]["const"].as_str() == Some(discriminator)
            })
            .cloned()
            .collect::<Vec<_>>();
        if rules.is_empty() {
            return Err("unknown direct error rule".to_owned());
        }
        schema["allOf"] = Value::Array(rules);
    }
    schema
        .as_object_mut()
        .ok_or_else(|| "invalid direct schema object".to_owned())?
        .remove("$id");
    let retriever = DIRECT_SCHEMA_RESOURCES_V1
        .get_or_init(embedded_schema_resources_v2)
        .as_ref()
        .map_err(Clone::clone)?;
    jsonschema::draft202012::options()
        .with_base_uri(format!("{EMBEDDED_SCHEMA_BASE_V2}{filename}"))
        .with_retriever(retriever.clone())
        .should_validate_formats(true)
        .build(&schema)
        .map_err(|error| format!("direct schema {filename} does not compile: {error}"))
}

#[cfg(test)]
fn build_daemon_free_validators_v1() -> Result<HashMap<&'static str, Validator>, String> {
    let retriever = embedded_schema_resources_v2()?;
    let mut validators = HashMap::new();
    for contract in direct_document_contracts_v1() {
        let filename = contract
            .schema_path
            .strip_prefix("schemas/")
            .ok_or_else(|| "invalid daemon-free schema path".to_owned())?;
        let source = embedded_schemas::EMBEDDED_JSON_SCHEMAS_V1
            .iter()
            .find(|(_, name, _)| *name == filename)
            .ok_or_else(|| format!("missing daemon-free schema {filename}"))?
            .2;
        let mut schema: Value = serde_json::from_str(source).map_err(|error| error.to_string())?;
        schema
            .as_object_mut()
            .ok_or_else(|| "invalid daemon-free schema object".to_owned())?
            .remove("$id");
        let validator = jsonschema::draft202012::options()
            .with_base_uri(format!("{EMBEDDED_SCHEMA_BASE_V2}{filename}"))
            .with_retriever(retriever.clone())
            .should_validate_formats(true)
            .build(&schema)
            .map_err(|error| format!("daemon-free schema {filename} does not compile: {error}"))?;
        validators.insert(contract.schema, validator);
    }
    Ok(validators)
}

/// Validate one current or explicitly reserved document against its canonical schema.
pub fn validate_daemon_free_document_v1(schema: &str, value: &Value) -> bool {
    let discriminator = match schema {
        "podway.output/v4" => value["command"].as_str(),
        "podway.error/v2" => value["code"].as_str(),
        _ => Some(""),
    };
    let Some(discriminator) = discriminator.filter(|value| value.len() <= 128) else {
        return false;
    };
    validate_json_document_depth(value).is_ok()
        && serialized_json_len_v1(value).is_ok_and(|length| length <= MAX_DIRECT_OUTPUT_BYTES_V1)
        && validate_selected_direct_schema_v1(schema, discriminator, value)
        && daemon_free_recovery_identity_v1(schema, value)
}

// Shared delegation callers retain their own budgets and correlations. Select
// only their canonical schema instead of initializing the legacy result family.
fn validate_selected_direct_schema_v1(schema: &str, discriminator: &str, value: &Value) -> bool {
    DAEMON_FREE_VALIDATORS_V1
        .get_or_init(direct_validator_registry_v1)
        .as_ref()
        .ok()
        .and_then(|validators| validators.get(&(schema.to_owned(), discriminator.to_owned())))
        .and_then(|cell| {
            cell.get_or_init(|| build_direct_validator_v1(schema, discriminator))
                .as_ref()
                .ok()
        })
        .is_some_and(|validator| validator.is_valid(value))
}

fn daemon_free_recovery_identity_v1(schema: &str, value: &Value) -> bool {
    let details = match schema {
        "podway.mutation-outcome-unknown-details/v3" => value,
        "podway.error/v2" if value["code"] == "MUTATION_OUTCOME_UNKNOWN" => &value["details"],
        _ => return true,
    };
    let Some(key) = details["idempotency_key"].as_str() else {
        return false;
    };
    details["reconcile"]["idempotency_key"].as_str() == Some(key)
        && details["recovery"]["argv"][5].as_str() == Some(key)
}

#[cfg(test)]
mod daemon_free_tests {
    const DELEGATION_COLD_SCHEMA: &str = "PODWAY_DELEGATION_COLD_SCHEMA";
    const DELEGATION_SCHEMAS: &[&str] = &[
        "podway.delegation-work-package/v1",
        "podway.delegation-accept-input/v1",
        "podway.delegation-receipt/v1",
    ];

    fn delegation_fixtures() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/v2/protocol/current-contracts.json"
        ))
        .unwrap()
    }

    #[test]
    fn delegation_cold_child() {
        let Ok(schema) = std::env::var(DELEGATION_COLD_SCHEMA) else {
            return;
        };
        assert!(DELEGATION_SCHEMAS.contains(&schema.as_str()));
        assert!(super::DIRECT_SCHEMA_RESOURCES_V1.get().is_none());
        assert!(super::DAEMON_FREE_VALIDATORS_V1.get().is_none());
        let value = delegation_fixtures()["fixtures"][&schema].clone();
        assert!(super::validate_delegation_document_v1(&schema, &value));
        let registry = super::DAEMON_FREE_VALIDATORS_V1
            .get()
            .unwrap()
            .as_ref()
            .unwrap();
        assert_eq!(
            registry
                .values()
                .filter(|cell| cell.get().is_some())
                .count(),
            1
        );
        assert!(
            registry[&(schema.clone(), String::new())]
                .get()
                .unwrap()
                .is_ok()
        );

        let mut invalid = value.clone();
        invalid["schema"] = serde_json::json!("unknown");
        assert!(!super::validate_delegation_document_v1(&schema, &invalid));
        assert!(super::validate_delegation_document_v1(&schema, &value));
        assert_eq!(
            registry
                .values()
                .filter(|cell| cell.get().is_some())
                .count(),
            1
        );
    }

    #[test]
    fn delegation_validation_compiles_only_its_selected_schema_in_a_cold_process() {
        use std::{
            process::Command,
            time::{Duration, Instant},
        };
        for schema in DELEGATION_SCHEMAS {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "result_contract::daemon_free_tests::delegation_cold_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(DELEGATION_COLD_SCHEMA, schema)
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success(), "cold schema validation failed: {schema}");
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("cold schema validation deadline expired: {schema}");
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    #[test]
    fn selected_delegation_validators_match_full_canonical_validation() {
        let full = super::build_daemon_free_validators_v1().unwrap();
        let fixtures = delegation_fixtures();
        for schema in DELEGATION_SCHEMAS {
            let value = fixtures["fixtures"][schema].clone();
            assert!(full[schema].is_valid(&value));
            let compare = |candidate: &serde_json::Value| {
                assert_eq!(
                    super::validate_selected_direct_schema_v1(schema, "", candidate),
                    full[schema].is_valid(candidate),
                    "{schema}: {candidate}"
                );
            };
            compare(&value);
            for field in value.as_object().unwrap().keys() {
                let mut candidate = value.clone();
                candidate.as_object_mut().unwrap().remove(field);
                compare(&candidate);
                let mut candidate = value.clone();
                candidate[field] = serde_json::Value::Null;
                compare(&candidate);
            }
            let mut unknown = value.clone();
            unknown["credential"] = serde_json::json!("unexpected");
            compare(&unknown);
            if let Some(execution) = value["execution"].as_object() {
                for field in execution.keys() {
                    let mut candidate = value.clone();
                    candidate["execution"]
                        .as_object_mut()
                        .unwrap()
                        .remove(field);
                    compare(&candidate);
                }
            }
            let mut invalid = value.clone();
            invalid["session_id"] = serde_json::json!("not-a-uuid");
            compare(&invalid);
        }
    }

    #[test]
    fn all_reserved_schemas_compile() {
        super::build_daemon_free_validators_v1().expect("reserved schema compilation");
    }

    #[test]
    fn selected_direct_validators_match_full_canonical_validation() {
        let validators = super::build_daemon_free_validators_v1().unwrap();
        let answers: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v2/protocol/current-contracts.json"
        ))
        .unwrap();
        let mut cases = answers["fixtures"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(schema, value)| (schema.clone(), value.clone()))
            .collect::<Vec<_>>();
        cases.extend(answers["cases"].as_array().unwrap().iter().map(|case| {
            (
                case["schema"].as_str().unwrap().to_owned(),
                case["value"].clone(),
            )
        }));
        for (schema, value) in cases {
            if !matches!(schema.as_str(), "podway.output/v4" | "podway.error/v2") {
                continue;
            }
            let validator = &validators[schema.as_str()];
            let compare = |candidate: &serde_json::Value| {
                assert_eq!(
                    super::validate_daemon_free_document_v1(&schema, candidate),
                    validator.is_valid(candidate)
                        && super::daemon_free_recovery_identity_v1(&schema, candidate),
                    "{schema}: {candidate}"
                );
            };
            compare(&value);
            for field in value.as_object().unwrap().keys() {
                let mut candidate = value.clone();
                candidate.as_object_mut().unwrap().remove(field);
                compare(&candidate);
                let mut candidate = value.clone();
                candidate[field] = serde_json::Value::Null;
                compare(&candidate);
            }
            for command in [
                "session.status",
                "session.reset",
                "procedure.validate",
                "workspace.doctor",
                "daemon.status",
                "unknown",
            ] {
                let mut candidate = value.clone();
                candidate["command"] = serde_json::json!(command);
                compare(&candidate);
            }
            for field in ["admission", "queue", "job"] {
                let mut candidate = value.clone();
                candidate[field] = serde_json::json!({});
                compare(&candidate);
            }
        }
    }
}
