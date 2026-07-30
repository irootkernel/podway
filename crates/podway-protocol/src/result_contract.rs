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
        ) && result.product.as_deref().is_none_or(non_empty)
            && result.daemon_version.as_deref().is_none_or(non_empty)
            && result.target.as_deref().is_none_or(non_empty)
            && result.source_commit.as_deref().is_none_or(non_empty)
            && result
                .contract_manifest_schema
                .as_deref()
                .is_none_or(non_empty)
            && unique_non_empty_strings(&result.protocol_versions)
            && result.pid.is_none_or(|pid| pid > 0)
            && optional_absolute_path(&result.executable_path)
            && optional_absolute_path(&result.socket_path)
            && optional_absolute_path(&result.configured_socket_path)
            && optional_absolute_path(&result.effective_socket_path)
            && if result.reachable {
                result.installed
                    && result.loaded
                    && result.pid.is_some()
                    && result.process_id.is_some()
                    && result.started_at.is_some()
                    && result.uptime_ms.is_some()
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
