#![allow(dead_code)]

use podway_core::{AttemptId, ItemId, JobId, Revision, Sha256Digest, StageId};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use crate::{
    JobStateV1, NextResultV1, ProtocolError, ResponseEnvelopeV1, Rfc3339MillisV1, StatusResultV1,
};

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
    let valid = if result.get("detached") == Some(&Value::Bool(true)) {
        if matches!(command, "session.start" | "session.start_replace") {
            decode::<DetachedStartResultV1>(value)
        } else {
            decode::<DetachedMutationResultV1>(value)
        }
    } else {
        match command {
            "version" => decode::<VersionResultV1>(value),
            "daemon.status" => {
                decode::<DaemonStatusResultV1>(value.clone())
                    || decode::<DaemonServiceStatusResultV1>(value)
            }
            "procedure.validate" => decode::<ProcedureValidationResultV1>(value),
            "session.status" => decode::<StatusResultV1>(value),
            "session.next" => decode::<NextResultV1>(value),
            "job.status" | "job.wait" => decode::<JobReadResultV1>(value),
            "job.lookup" => match result.get("found") {
                Some(Value::Bool(false)) => decode::<JobLookupMissingResultV1>(value),
                Some(Value::Bool(true)) => decode::<JobLookupFoundResultV1>(value),
                _ => false,
            },
            "session.start" | "session.start_replace" => {
                if result.contains_key("dry_run") {
                    decode::<StartDryRunResultV1>(value)
                } else {
                    decode::<StartTerminalResultV1>(value)
                }
            }
            command if is_item_mutation(command) => decode::<ItemMutationResultV1>(value),
            command if is_stage_transition(command) => {
                if result.get("preview") == Some(&Value::Bool(true)) {
                    decode::<StagePreviewResultV1>(value)
                } else if command == "session.reset" && result.contains_key("reset") {
                    decode::<ResetResultV1>(value)
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

fn command_result_schema_v1(command: &str, result: &Map<String, Value>) -> Option<&'static str> {
    if result.get("detached") == Some(&Value::Bool(true)) {
        return Some("podway.detached-admission-result/v1");
    }
    match command {
        "version" => Some("podway.version-result/v1"),
        "daemon.status" => Some("podway.daemon-status-result/v1"),
        "procedure.validate" => Some("podway.procedure-validation-result/v1"),
        "session.status" => Some("podway.status-result/v1"),
        "session.next" => Some("podway.next-result/v1"),
        "job.status" | "job.wait" => Some("podway.job-result/v1"),
        "job.lookup" => Some("podway.job-lookup-result/v1"),
        "session.start" | "session.start_replace" => Some("podway.session-start-result/v1"),
        command if is_item_mutation(command) => Some("podway.item-mutation-result/v1"),
        command if is_stage_transition(command) => Some("podway.stage-transition-result/v1"),
        _ => None,
    }
}

fn decode<T: for<'de> Deserialize<'de>>(value: Value) -> bool {
    serde_json::from_value::<T>(value).is_ok()
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
    warnings: Vec<Value>,
    canonical_json: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionResultV1 {
    admitted: bool,
    job_id: JobId,
    workspace_sequence: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DetachedMutationResultV1 {
    admission: AdmissionResultV1,
    detached: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DetachedStartResultV1 {
    admission: AdmissionResultV1,
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
    attempt_number: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StageAttemptV1 {
    attempt_id: AttemptId,
    attempt_number: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AffectedStageV1 {
    stage_id: StageId,
    before: Option<String>,
    after: Option<String>,
    before_attempt: Option<StageAttemptV1>,
    after_attempt: Option<StageAttemptV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StagePreviewResultV1 {
    preview: bool,
    changed: bool,
    revision_before: Option<Revision>,
    revision_after: Option<Revision>,
    active_before: Option<ActiveAttemptV1>,
    active_after: Option<ActiveAttemptV1>,
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
