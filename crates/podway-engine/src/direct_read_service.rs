//! Validated direct projections over a coherent Store snapshot.

use podway_core::{
    AttemptId, GraphNodeId, ItemId, SessionLifecycle, TerminalDispositionKindV2,
    TerminalDispositionV2, TraceSequenceV2,
};
use podway_protocol::{EvidencePageTokenV1, Rfc3339MillisV1, validate_daemon_free_document_v1};
use podway_store::direct_store::DirectWorkspaceViewV1;
use serde_json::{Map, Value};

use crate::v2_read_service::{
    EvidenceReadErrorV2, GraphStatusTierV2, GraphViewErrorV2, ReadScope,
    delegation_work_package_in, project_delegation_package_result_in,
    project_delegation_receipt_read_in, project_evidence_read_in, project_graph_archive_show_in,
    project_graph_next_in, project_graph_observation_in, project_graph_status_in,
};

pub(crate) fn archive_summary(summary: &podway_store::SessionArchiveSummaryV2) -> Value {
    serde_json::json!({"session_id":summary.session_id(), "task_title":summary.task_title(),
        "lifecycle": match summary.lifecycle() { SessionLifecycle::Prepared => "prepared", SessionLifecycle::Running => "running",
            SessionLifecycle::Completed => "completed", SessionLifecycle::Cancelled => "cancelled" },
        "session_revision":summary.session_revision(), "archive_slot":summary.archive_slot(),
        "archived_at_ms":summary.archived_at().get(), "archived_workspace_revision":summary.archived_workspace_revision()})
}

pub(crate) fn terminal_disposition(
    record: &TerminalDispositionV2,
) -> Result<Value, GraphViewErrorV2> {
    let timestamp = Rfc3339MillisV1::from_unix_millis(record.recorded_at().get())
        .map_err(|_| GraphViewErrorV2::InconsistentState("invalid disposition time"))?;
    let mut value = serde_json::json!({"session_revision":record.terminal_session_revision(), "recorded_at":timestamp});
    match record.kind() {
        TerminalDispositionKindV2::HandedOff => {
            value["kind"] = serde_json::json!("handed_off");
            value["summary"] = serde_json::json!(record.summary());
            value["reference"] = serde_json::json!(record.stable_reference());
        }
        TerminalDispositionKindV2::NotRequired => {
            value["kind"] = serde_json::json!("not_required");
            value["reason"] = serde_json::json!(record.reason());
        }
        TerminalDispositionKindV2::Superseded => {
            value["kind"] = serde_json::json!("superseded");
            value["reason"] = serde_json::json!(record.reason());
            value["successor_session_id"] = serde_json::json!(record.successor_session_id());
        }
    }
    if let Some(actor) = record.actor() {
        value["actor"] = serde_json::json!(actor.as_str());
    }
    Ok(value)
}

pub fn project_direct_archive_show_v1(
    view: &DirectWorkspaceViewV1,
    summary: &podway_store::SessionArchiveSummaryV2,
    tier: GraphStatusTierV2,
    history_before: Option<TraceSequenceV2>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let scope = ReadScope::from_direct(view);
    let status = direct_status(&scope, tier, history_before)?;
    let disposition =
        view.terminal_dispositions()
            .last()
            .ok_or(GraphViewErrorV2::InconsistentState(
                "archive has no disposition",
            ))?;
    let result = project_graph_archive_show_in(
        &scope,
        status,
        archive_summary(summary),
        terminal_disposition(disposition)?,
    )?;
    let mut result = result
        .as_object()
        .ok_or(GraphViewErrorV2::InconsistentState(
            "archive projection is not an object",
        ))?
        .clone();
    result.insert(
        "schema".into(),
        Value::String("podway.session-archive-show-result/v3".into()),
    );
    validate_projection("podway.session-archive-show-result/v3", result)
}

pub fn project_direct_receipt_v1(
    view: &DirectWorkspaceViewV1,
    attempt: &AttemptId,
    receipt: Option<&podway_store::DelegationReceiptV1>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    validate_projection(
        "podway.delegation-receipt-result/v1",
        project_delegation_receipt_read_in(&ReadScope::from_direct(view), attempt, receipt)?,
    )
}

fn validate_projection(
    schema: &str,
    result: Map<String, Value>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let value = Value::Object(result);
    if !validate_daemon_free_document_v1(schema, &value) {
        return Err(GraphViewErrorV2::InconsistentState(
            "direct projection violates its canonical schema",
        ));
    }
    match value {
        Value::Object(result) => Ok(result),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            Err(GraphViewErrorV2::InconsistentState(
                "direct projection is not an object",
            ))
        }
    }
}

pub fn project_direct_status_v1(
    view: &DirectWorkspaceViewV1,
    tier: GraphStatusTierV2,
    history_before: Option<TraceSequenceV2>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    direct_status(&ReadScope::from_direct(view), tier, history_before)
}

fn direct_status(
    scope: &ReadScope<'_>,
    tier: GraphStatusTierV2,
    history_before: Option<TraceSequenceV2>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let mut result = project_graph_status_in(scope, tier, history_before)?;
    let schema = match tier {
        GraphStatusTierV2::Compact => "podway.compact-status-result/v5",
        GraphStatusTierV2::Standard | GraphStatusTierV2::Verbose => "podway.status-result/v5",
    };
    result.insert("schema".into(), Value::String(schema.into()));
    validate_projection(schema, result)
}

fn direct_next_schema(view: &DirectWorkspaceViewV1) -> &'static str {
    if view
        .graph()
        .is_some_and(|graph| graph.trace().lifecycle() == SessionLifecycle::Prepared)
    {
        "podway.prepared-next-result/v3"
    } else {
        "podway.next-result/v5"
    }
}

pub fn project_direct_next_v1(
    view: &DirectWorkspaceViewV1,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let mut result = project_graph_next_in(&ReadScope::from_direct(view))?;
    let schema = direct_next_schema(view);
    result.insert("schema".into(), Value::String(schema.into()));
    validate_projection(schema, result)
}

pub fn project_direct_observation_v1(
    view: &DirectWorkspaceViewV1,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let mut result = project_graph_observation_in(&ReadScope::from_direct(view))?;
    result.insert(
        "schema".into(),
        Value::String("podway.observation-result/v5".into()),
    );
    result
        .get("status")
        .and_then(Value::as_object)
        .ok_or(GraphViewErrorV2::InconsistentState(
            "observation has no structured status",
        ))?;
    if let Some(guidance) = result.get_mut("guidance").and_then(Value::as_object_mut) {
        guidance.insert(
            "schema".into(),
            Value::String(direct_next_schema(view).into()),
        );
    }
    validate_projection("podway.observation-result/v5", result)
}

pub fn project_direct_evidence_v1(
    view: &DirectWorkspaceViewV1,
    source: &GraphNodeId,
    item_id: &ItemId,
    token: Option<&EvidencePageTokenV1>,
) -> Result<Map<String, Value>, EvidenceReadErrorV2> {
    let result = project_evidence_read_in(&ReadScope::from_direct(view), source, item_id, token)?;
    Ok(validate_projection(
        "podway.evidence-read-result/v1",
        result,
    )?)
}

pub fn project_direct_delegation_package_v1(
    view: &DirectWorkspaceViewV1,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let scope = ReadScope::from_direct(view);
    let package = delegation_work_package_in(&scope)?;
    validate_projection(
        "podway.delegation-package-result/v1",
        project_delegation_package_result_in(&scope, &package)?,
    )
}
