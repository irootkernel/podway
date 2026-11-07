//! Deterministic, bounded Procedure v2 status and next projections.
//!
//! Projection consumes one coherent Store observation and the immutable canonical snapshot
//! captured at session start. It never resolves the Procedure source or mutates runtime state.

use std::{
    cell::{Ref, RefCell},
    error::Error,
    fmt,
};

use podway_config::{
    DelegationEvidenceItemV1, DelegationEvidenceResolutionV1, DelegationEvidenceV1,
    DelegationWorkPackageInputV1, DelegationWorkPackageV1, ParsedNodeDefinition, ParsedProcedure,
    ParsedProcedureV2, ProcedureDocumentFormat, goal_revision_safe_targets_v2,
    parse_procedure_document, project_delegation_work_package_v1, validate_procedure_v2,
};
use podway_core::{
    ArtifactLocationKindV1,
    AttemptId,
    AttemptLifecycle,
    AttemptValidityV2,
    BlockerState,
    ConditionStateV2,
    ConditionStatusV2,
    CriterionCitationV2,
    CriterionStatusV2,
    // Worst-case escaping and per-element structure are what make a page or preview bound provable,
    // and the authoring budget charges the same two factors.
    EVIDENCE_ENTRY_OVERHEAD_BYTES_V2 as EVIDENCE_ENTRY_OVERHEAD_BYTES,
    EVIDENCE_SCALAR_BYTES_V2 as EVIDENCE_SCALAR_BYTES,
    GoalOutcome,
    GraphNodeId,
    GraphPlacementV2,
    ItemId,
    ItemPredicateOperatorV2,
    ItemSpecV2,
    ItemTypeV1,
    MAX_EVIDENCE_PAGE_BYTES_V2,
    MAX_EVIDENCE_PREVIEW_BYTES_V2,
    MAX_EVIDENCE_PREVIEW_SCALARS_V2,
    MAX_EVIDENCE_PREVIEW_SLOTS_V2,
    MAX_EVIDENCE_RESPONSE_BYTES_V2,
    MAX_MISSING_ITEM_WINDOW_V2,
    MAX_OBSERVATION_CHOICE_WINDOW_V2,
    MAX_SUGGESTION_WINDOW_V2,
    PROCEDURE_SCHEMA_V2,
    PredicateActualV2,
    PredicateScalarV2,
    RecordedItemValueV2,
    ResolvedEvidenceReferenceV2,
    SessionAttemptV2,
    SessionLifecycle,
    TraceSequenceV2,
    UnixMillis,
    canonicalize_json_v1,
};
use podway_protocol::{
    ContractGenerationV1, EVIDENCE_PAGE_TOKEN_VERSION_V1, EvidencePageTokenV1,
    serialized_json_len_v1,
};
use podway_store::direct_store::DirectWorkspaceViewV1;
use podway_store::v2_delegation::{
    DelegationReadinessV1, DelegationReceiptSnapshotV1, DelegationReceiptV1,
};
use podway_store::{
    AttemptWorkflowMemoryV2, DurableWorktreeIdentityV1, EvidenceLookupErrorV2, EvidenceReadbackV2,
    EvidenceResolutionStateV2, GraphSessionStateV2, GraphWorkspaceViewV2, recorded_value_digest_v2,
    recorded_value_encoded_size_v2,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

mod observation;

pub(crate) use self::observation::{
    SuccessorObservationContextV1, finalize_direct_observation_in, project_successor_observation_v1,
};
use self::observation::{
    project_graph_observation_in, project_prepared_next_v1, serialized_bytes_v1,
    serialized_object_bytes_v1,
};

const ITEM_DISPLAY_CHARS_MAX: usize = 2_048;
const STATUS_VALUES_BYTES_MAX: usize = 262_144;
const BLOCKER_WINDOW_BYTES_MAX: usize = 49_152;
const HISTORY_WINDOW_BYTES_MAX: usize = 65_536;
const OBSERVATION_ITEM_VALUE_CHARS_MAX: usize = 128;
const OBSERVATION_ITEM_VALUE_BYTES_MAX: usize = 1_024;
// Result schemas bound these values independently of the aggregate direct output cap.
const MAX_DELEGATION_PACKAGE_RESULT_BYTES_V1: usize = 1_000_000;
const MAX_DELEGATION_RECEIPT_RESULT_BYTES_V1: usize = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphStatusTierV2 {
    Compact,
    Standard,
    Verbose,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphViewErrorV2 {
    MissingGraphSession,
    TerminalSessionHasNoNext,
    NoActiveAttempt,
    DelegationNotApplicable,
    DelegationReceiptMissing,
    AttemptNotFound,
    InvalidSnapshot,
    InconsistentState(&'static str),
    TimestampOutOfRange,
}

impl fmt::Display for GraphViewErrorV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingGraphSession => formatter.write_str("no Procedure v2 session exists"),
            Self::TerminalSessionHasNoNext => {
                formatter.write_str("a terminal Procedure v2 session has no next action")
            }
            Self::NoActiveAttempt => formatter.write_str("no active attempt exists"),
            Self::DelegationNotApplicable => formatter.write_str("no delegated action is current"),
            Self::DelegationReceiptMissing => {
                formatter.write_str("no accepted receipt exists for the attempt")
            }
            Self::AttemptNotFound => formatter.write_str("the selected attempt does not exist"),
            Self::InvalidSnapshot => {
                formatter.write_str("the stored Procedure v2 snapshot cannot be rehydrated")
            }
            Self::InconsistentState(reason) => formatter.write_str(reason),
            Self::TimestampOutOfRange => formatter.write_str("timestamp is outside RFC 3339 range"),
        }
    }
}

impl Error for GraphViewErrorV2 {}

fn delegated_placement_count(procedure: &ParsedProcedureV2) -> usize {
    procedure
        .graph()
        .placements()
        .iter()
        .filter(|placement| {
            matches!(placement, GraphPlacementV2::Action(action) if action.delegation().is_some())
        })
        .count()
}

fn delegation_projection(
    scope: &ReadScope<'_>,
    state: &GraphSessionStateV2,
    current: Option<&CurrentProjection<'_>>,
) -> Value {
    let Some(current) = current else {
        return if state.trace().lifecycle() == SessionLifecycle::Prepared {
            json!({"state":"inactive","reason":"no_active_attempt","safe_next_action":"begin_or_inspect"})
        } else {
            json!({"state":"inactive","reason":"historical_attempt","safe_next_action":"inspect_history"})
        };
    };
    if !current.delegated() {
        return json!({"state":"not_applicable","reason":"direct_placement","safe_next_action":"continue_direct"});
    }
    let receipts = scope.receipts();
    match receipts.readiness() {
        DelegationReadinessV1::ReceiptCurrent => json!({
            "state":"accepted", "reason":"current_receipt", "safe_next_action":"complete",
            "guidance":"Changing an accepted item makes the receipt stale; complete while its item state is current.",
            "receipt":receipts.current_receipt().map(|receipt| receipt.summary()),
        }),
        DelegationReadinessV1::PackageChanged | DelegationReadinessV1::ItemsChanged => json!({
            "state":"stale_acceptance",
            "reason": if receipts.readiness() == DelegationReadinessV1::PackageChanged {"package_changed"} else {"items_changed"},
            "safe_next_action":"retry_or_rework",
            "guidance":"The acceptance is stale. Retry or rework for a fresh attempt; do not redispatch automatically.",
            "receipt":receipts.current_receipt().map(|receipt| receipt.summary()),
        }),
        DelegationReadinessV1::Direct | DelegationReadinessV1::ReceiptMissing => {
            let (reason, action) = if !current.items_satisfied {
                ("items_incomplete", "record_items")
            } else if !current.open_blockers.is_empty() {
                ("blockers_open", "clear_blockers")
            } else {
                ("receipt_required", "accept")
            };
            json!({"state":"awaiting_acceptance","reason":reason,"safe_next_action":action,
                "guidance":"Finish all items and corrections before acceptance; later item edits require a fresh attempt."})
        }
    }
}

pub fn delegation_work_package_from_view_v1(
    view: &GraphWorkspaceViewV2,
) -> Result<DelegationWorkPackageV1, GraphViewErrorV2> {
    delegation_work_package_in(&ReadScope::from_graph(view))
}

pub(crate) fn delegation_work_package_in(
    scope: &ReadScope<'_>,
) -> Result<DelegationWorkPackageV1, GraphViewErrorV2> {
    let state = scope.graph()?;
    let validated = scope.validated()?;
    let procedure = validated.parsed();
    let current = CurrentProjection::derive(scope, state, procedure)?
        .ok_or(GraphViewErrorV2::NoActiveAttempt)?;
    let GraphPlacementV2::Action(action) = current.placement else {
        return Err(GraphViewErrorV2::DelegationNotApplicable);
    };
    if action.delegation().is_none() {
        return Err(GraphViewErrorV2::DelegationNotApplicable);
    }
    // `CurrentProjection::derive` already selected this attempt's evidence and fails closed when
    // that selection is invalid, so the package reuses the projection's admitted selection;
    // repeating the selection here could only fail on state the projection already rejected.
    let readback = &current.evidence_readback;
    let selectors = action
        .evidence_from()
        .map(|references| references.entries());
    if selectors.map_or(0, |entries| entries.len()) != readback.len() {
        return Err(GraphViewErrorV2::InconsistentState(
            "delegation evidence selector count is inconsistent",
        ));
    }
    let mut evidence = Vec::with_capacity(readback.len());
    for (selector, entry) in selectors.into_iter().flatten().zip(readback.iter()) {
        if entry.stale() || selector.source_node() != entry.reference().resolution().source_node() {
            return Err(GraphViewErrorV2::InconsistentState(
                "delegation evidence selector is stale or inconsistent",
            ));
        }
        let resolution = match entry.reference().resolution() {
            ResolvedEvidenceReferenceV2::Resolved(snapshot) => {
                DelegationEvidenceResolutionV1::Resolved {
                    source_attempt_id: snapshot.source_attempt_id().clone(),
                    source_attempt_number: snapshot.source_attempt_number(),
                    source_items_digest: snapshot.items_digest().clone(),
                }
            }
            ResolvedEvidenceReferenceV2::Skipped(snapshot) => {
                DelegationEvidenceResolutionV1::Skipped {
                    source_attempt_id: snapshot.source_attempt_id().clone(),
                    source_attempt_number: snapshot.source_attempt_number(),
                    source_items_digest: snapshot.items_digest().clone(),
                }
            }
            ResolvedEvidenceReferenceV2::Unresolved { .. } => {
                DelegationEvidenceResolutionV1::Unresolved
            }
        };
        let source = procedure
            .graph()
            .placements()
            .iter()
            .find(|placement| placement.id() == selector.source_node())
            .ok_or(GraphViewErrorV2::InvalidSnapshot)?;
        let definition_id = match source {
            GraphPlacementV2::Action(source) => source.definition(),
            GraphPlacementV2::Decision(source) => source.definition(),
        };
        let definition = procedure
            .node_definitions()
            .iter()
            .find(|definition| definition.id() == definition_id)
            .ok_or(GraphViewErrorV2::InvalidSnapshot)?;
        let specs = match definition {
            ParsedNodeDefinition::Action(source) => source.items(),
            ParsedNodeDefinition::Decision(source) => source.items(),
        };
        let item_ids: Vec<&ItemId> = selector.selected_items().map_or_else(
            || specs.iter().map(ItemSpecV2::id).collect(),
            |selected| selected.iter().collect(),
        );
        let source_memory = entry
            .reference()
            .resolution()
            .snapshot()
            .and_then(|snapshot| {
                state
                    .workflow_memory()
                    .attempts()
                    .iter()
                    .find(|memory| memory.attempt_id() == snapshot.source_attempt_id())
            });
        let items = item_ids
            .into_iter()
            .map(|item_id| {
                let Some(value) = entry
                    .items()
                    .items()
                    .iter()
                    .find(|item| item.id() == item_id)
                else {
                    return Ok(DelegationEvidenceItemV1::Missing {
                        item_id: item_id.clone(),
                    });
                };
                let revision = source_memory
                    .and_then(|memory| {
                        memory
                            .item_slots()
                            .iter()
                            .find(|slot| slot.item_id() == item_id)
                    })
                    .map(|slot| slot.revision())
                    .ok_or(GraphViewErrorV2::InconsistentState(
                        "delegation evidence item has no source slot",
                    ))?;
                Ok(DelegationEvidenceItemV1::Present {
                    item_id: item_id.clone(),
                    item_revision: revision,
                    value_digest: recorded_value_digest_v2(value.value())
                        .map_err(|_| GraphViewErrorV2::InvalidSnapshot)?,
                    value_type: value.value().item_type(),
                    size_bytes: recorded_value_encoded_size_v2(value.value())
                        .map_err(|_| GraphViewErrorV2::InvalidSnapshot)?,
                })
            })
            .collect::<Result<Vec<_>, GraphViewErrorV2>>()?;
        evidence.push(DelegationEvidenceV1 {
            selector: selector.clone(),
            resolution,
            items,
        });
    }
    let goal = current.attempt.goal_revision().and_then(|revision| {
        state
            .goal_state()
            .revisions()
            .iter()
            .find(|record| record.revision() == revision)
    });
    project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
        procedure: &validated,
        workspace_uuid: scope.identity().workspace_uuid(),
        session_id: state.trace().session_id(),
        placement_id: current.attempt.graph_node_id(),
        attempt_id: current.attempt.attempt_id(),
        goal,
        evidence: &evidence,
    })
    .map_err(|_| GraphViewErrorV2::InvalidSnapshot)
}

pub fn project_delegation_package_result_v1(
    view: &GraphWorkspaceViewV2,
    package: &DelegationWorkPackageV1,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    project_delegation_package_result_in(&ReadScope::from_graph(view), package)
}

pub(crate) fn project_delegation_package_result_in(
    scope: &ReadScope<'_>,
    package: &DelegationWorkPackageV1,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let state = scope.graph()?;
    let active = state
        .trace()
        .active_attempt()
        .ok_or(GraphViewErrorV2::InconsistentState(
            "delegation package requires an active attempt",
        ))?;
    let value = package.package();
    if value.get("workspace_uuid") != Some(&json!(scope.identity().workspace_uuid()))
        || value.get("session_id") != Some(&json!(state.trace().session_id()))
        || value.get("attempt_id") != Some(&json!(active.attempt_id()))
        || value.get("placement_id") != Some(&json!(active.graph_node_id()))
        || value.get("procedure_digest") != Some(&json!(state.snapshot().digest()))
    {
        return Err(GraphViewErrorV2::InconsistentState(
            "delegation package does not match the selected attempt",
        ));
    }
    let result = json!({
        "schema":"podway.delegation-package-result/v1",
        "package":value,
        "package_digest":package.digest().as_str(),
        "workspace_uuid":scope.identity().workspace_uuid().as_str(),
        "session_id":state.trace().session_id().as_str(),
        "session_revision":state.trace().revision().get(),
        "attempt_id":active.attempt_id().as_str(),
    });
    if serialized_bytes_v1(&result) > MAX_DELEGATION_PACKAGE_RESULT_BYTES_V1 {
        return Err(GraphViewErrorV2::InconsistentState(
            "delegation package result exceeds its byte allocation",
        ));
    }
    Ok(into_object(result))
}

pub fn project_delegation_receipt_result_v1(
    view: &GraphWorkspaceViewV2,
    receipt: &DelegationReceiptV1,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    project_delegation_receipt_result_in(&ReadScope::from_graph(view), receipt)
}

pub(crate) fn project_delegation_receipt_result_in(
    scope: &ReadScope<'_>,
    receipt: &DelegationReceiptV1,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let state = scope.graph()?;
    if receipt.workspace_uuid() != *scope.identity().workspace_uuid()
        || receipt.session_id() != *state.trace().session_id()
        || !state
            .trace()
            .attempts()
            .iter()
            .any(|attempt| attempt.attempt_id() == &receipt.attempt_id())
    {
        return Err(GraphViewErrorV2::InconsistentState(
            "delegation receipt does not belong to the selected session",
        ));
    }
    let active = state
        .trace()
        .active_attempt()
        .is_some_and(|attempt| attempt.attempt_id() == &receipt.attempt_id());
    if active && scope.receipts().current_receipt() != Some(receipt) {
        return Err(GraphViewErrorV2::InconsistentState(
            "active delegation receipt differs from the coherent workspace view",
        ));
    }
    let (applicability, reason) = if !active {
        ("historical", "historical_attempt")
    } else {
        match scope.receipts().readiness() {
            DelegationReadinessV1::ReceiptCurrent => ("accepted", "current_receipt"),
            DelegationReadinessV1::PackageChanged => ("stale_acceptance", "package_changed"),
            DelegationReadinessV1::ItemsChanged => ("stale_acceptance", "items_changed"),
            DelegationReadinessV1::Direct | DelegationReadinessV1::ReceiptMissing => {
                return Err(GraphViewErrorV2::InconsistentState(
                    "active delegation receipt has no applicable readiness",
                ));
            }
        }
    };
    let result = json!({
        "schema":"podway.delegation-receipt-result/v1",
        "session_id":receipt.session_id().as_str(),
        "attempt_id":receipt.attempt_id().as_str(),
        "receipt":receipt.value(),
        "applicability":applicability,
        "reason":reason,
    });
    if serialized_bytes_v1(&result) > MAX_DELEGATION_RECEIPT_RESULT_BYTES_V1 {
        return Err(GraphViewErrorV2::InconsistentState(
            "delegation receipt result exceeds its byte allocation",
        ));
    }
    Ok(into_object(result))
}

pub fn project_delegation_receipt_read_result_v1(
    view: &GraphWorkspaceViewV2,
    attempt_id: &AttemptId,
    receipt: Option<&DelegationReceiptV1>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    project_delegation_receipt_read_in(&ReadScope::from_graph(view), attempt_id, receipt)
}

pub(crate) fn project_delegation_receipt_read_in(
    scope: &ReadScope<'_>,
    attempt_id: &AttemptId,
    receipt: Option<&DelegationReceiptV1>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let state = scope.graph()?;
    let attempt = state
        .trace()
        .attempts()
        .iter()
        .find(|attempt| attempt.attempt_id() == attempt_id)
        .ok_or(GraphViewErrorV2::AttemptNotFound)?;
    let procedure = scope.procedure()?;
    let delegated = procedure.graph().placements().iter().any(|placement| {
        placement.id() == attempt.graph_node_id()
            && matches!(placement, GraphPlacementV2::Action(action) if action.delegation().is_some())
    });
    if !delegated {
        return Err(GraphViewErrorV2::DelegationNotApplicable);
    }
    project_delegation_receipt_result_in(
        scope,
        receipt.ok_or(GraphViewErrorV2::DelegationReceiptMissing)?,
    )
}

pub fn project_graph_status_v2(
    view: &GraphWorkspaceViewV2,
    tier: GraphStatusTierV2,
    history_before: Option<TraceSequenceV2>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    project_graph_status_in(&ReadScope::from_graph(view), tier, history_before)
}

pub(crate) fn project_graph_status_in(
    scope: &ReadScope<'_>,
    tier: GraphStatusTierV2,
    history_before: Option<TraceSequenceV2>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let state = scope.graph()?;
    if tier != GraphStatusTierV2::Verbose && history_before.is_some() {
        return Err(GraphViewErrorV2::InconsistentState(
            "a history cursor is valid only for verbose status",
        ));
    }
    let procedure = scope.procedure()?;
    let current = CurrentProjection::derive(scope, state, &procedure)?;
    let delegated = delegated_placement_count(&procedure) != 0;

    let mut result = Map::new();
    let schema = match tier {
        GraphStatusTierV2::Compact => "podway.compact-status-result/v5",
        GraphStatusTierV2::Standard | GraphStatusTierV2::Verbose => "podway.status-result/v5",
    };
    result.insert("schema".to_owned(), json!(schema));
    if tier != GraphStatusTierV2::Compact {
        result.insert(
            "tier".to_owned(),
            json!(if tier == GraphStatusTierV2::Standard {
                "standard"
            } else {
                "verbose"
            }),
        );
    }
    result.insert("procedure".to_owned(), procedure_identity(state));
    result.insert("session".to_owned(), session_identity(state));
    result.insert(
        "current".to_owned(),
        current
            .as_ref()
            .map(CurrentProjection::identity)
            .unwrap_or(Value::Null),
    );
    result.insert(
        "goal_tracking".to_owned(),
        json!(state.snapshot().goal_tracking()),
    );
    result.insert(
        "goal_defined".to_owned(),
        json!(state.goal_state().current_revision().is_some()),
    );
    add_goal_summary(&mut result, state);
    result.insert(
        "trace_length".to_owned(),
        json!(state.trace().attempts().len()),
    );
    result.insert("counters".to_owned(), counters(state));
    result.insert(
        "items".to_owned(),
        Value::Array(
            current
                .as_ref()
                .map(CurrentProjection::compact_items)
                .unwrap_or_default(),
        ),
    );
    if delegated {
        result.insert(
            "delegation".to_owned(),
            delegation_projection(scope, state, current.as_ref()),
        );
    }
    if tier == GraphStatusTierV2::Compact {
        return Ok(result);
    }
    if delegated {
        let receipts = scope.receipts();
        result.insert("receipt_summaries".to_owned(), json!(receipts.summaries()));
        result.insert(
            "receipt_summaries_total".to_owned(),
            json!(receipts.total()),
        );
        result.insert(
            "receipt_summaries_truncated".to_owned(),
            json!(receipts.truncated()),
        );
    }

    result.insert("purpose".to_owned(), json!(state.snapshot().purpose()));
    result.insert(
        "missing_required_item_ids".to_owned(),
        Value::Array(
            current
                .as_ref()
                .map(CurrentProjection::missing_ids)
                .unwrap_or_default(),
        ),
    );
    let (blockers, blockers_truncated) = current
        .as_ref()
        .map(CurrentProjection::blocker_window)
        .transpose()?
        .unwrap_or_default();
    result.insert("blocker_window".to_owned(), Value::Array(blockers));
    result.insert("blockers_truncated".to_owned(), json!(blockers_truncated));
    let (item_values, items_total, items_truncated) = current
        .as_ref()
        .map(CurrentProjection::item_values)
        .unwrap_or_default();
    result.insert("items_total".to_owned(), json!(items_total));
    result.insert("items_truncated".to_owned(), Value::Bool(items_truncated));
    result.insert("item_values".to_owned(), Value::Array(item_values));
    result.insert(
        "references".to_owned(),
        Value::Array(
            current
                .as_ref()
                .map(|current| current.reference_metadata(&procedure))
                .transpose()?
                .unwrap_or_default(),
        ),
    );
    result.insert(
        "allowed_option_ids".to_owned(),
        Value::Array(
            current
                .as_ref()
                .map(CurrentProjection::allowed_option_ids)
                .unwrap_or_default(),
        ),
    );
    result.insert(
        "allowed_manual_rework_targets".to_owned(),
        manual_rework_targets(state, &procedure),
    );
    add_static_disposition(&mut result, current.as_ref());
    if let Some(goal) = goal_display(state, current.as_ref()) {
        result.insert("goal".to_owned(), goal);
    }
    if tier == GraphStatusTierV2::Verbose {
        add_histories(&mut result, state, &procedure, history_before)?;
    }
    Ok(result)
}

pub fn project_graph_archive_show_result_v2(
    view: &GraphWorkspaceViewV2,
    status: Map<String, Value>,
    archive: Value,
    terminal_disposition: Value,
) -> Result<Value, GraphViewErrorV2> {
    Ok(Value::Object(project_graph_archive_show_in(
        &ReadScope::from_graph(view),
        status,
        archive,
        terminal_disposition,
    )?))
}

pub(crate) fn project_graph_archive_show_in(
    scope: &ReadScope<'_>,
    status: Map<String, Value>,
    archive: Value,
    terminal_disposition: Value,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let procedure = scope.procedure()?;
    let delegated = delegated_placement_count(&procedure) != 0;
    // The owned archive, disposition and status members move into the result: the `json!`
    // macro finalizes every expression through `to_value`, which would reserialize each of
    // them into a fresh copy before the result reached this function's callers.
    let mut result = Map::with_capacity(if delegated { 8 } else { 5 });
    result.insert(
        "schema".to_owned(),
        json!("podway.session-archive-show-result/v3"),
    );
    result.insert("activity".to_owned(), json!("inactive"));
    result.insert("archive".to_owned(), archive);
    result.insert("terminal_disposition".to_owned(), terminal_disposition);
    result.insert("state".to_owned(), Value::Object(status));
    if delegated {
        let receipts = scope.receipts();
        result.insert("receipt_summaries".to_owned(), json!(receipts.summaries()));
        result.insert(
            "receipt_summaries_total".to_owned(),
            json!(receipts.total()),
        );
        result.insert(
            "receipt_summaries_truncated".to_owned(),
            json!(receipts.truncated()),
        );
    }
    Ok(result)
}

/// Why one bounded evidence read could not be served.
///
/// Each variant maps to exactly one public code, so the daemon never has to guess which failure a
/// caller met. Nothing here changes state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvidenceReadErrorV2 {
    View(GraphViewErrorV2),
    /// The declared reference resolved to no readable value.
    NotAvailable {
        graph_node_id: String,
        source_graph_node_id: String,
        item_id: String,
        reference_state: &'static str,
    },
    /// A previously valid token no longer describes current state.
    TokenStale {
        graph_node_id: String,
        source_graph_node_id: String,
        item_id: String,
        expected_value_digest: String,
        current_value_digest: Option<String>,
    },
    /// The token offset is at or past the logical end of the value.
    TokenExhausted {
        source_graph_node_id: String,
        item_id: String,
        offset: u64,
        total_size: u64,
        unit: &'static str,
    },
    /// The token decoded but binds another session or another item.
    ///
    /// ADR-0024 refuses these before any state comparison, so an invented token can never reveal
    /// whether a session, attempt, or item exists.
    TokenNotBound,
    /// The declared reference resolved to a source attempt that is no longer valid.
    ///
    /// This is the reference itself going stale rather than a token losing its snapshot, so it
    /// keeps the pre-existing `EVIDENCE_REFERENCE_STALE` classification.
    ///
    /// Persistence validation refuses a valid consumer attempt that holds a stale evidence source,
    /// so this arm is fail-closed defense rather than a reachable outcome today. Classifying it
    /// correctly costs nothing and keeps the published code honest if that invariant ever moves.
    ReferenceStale {
        graph_node_id: String,
        source_graph_node_id: String,
        expected_source_attempt_id: String,
    },
}

impl From<GraphViewErrorV2> for EvidenceReadErrorV2 {
    fn from(error: GraphViewErrorV2) -> Self {
        Self::View(error)
    }
}

/// Reject cross-session or wrong-item tokens before caller and snapshot fences.
pub(crate) fn check_evidence_token_binding_v1(
    session_id: Option<&podway_core::SessionId>,
    item_id: &ItemId,
    token: Option<&EvidencePageTokenV1>,
) -> Result<(), EvidenceReadErrorV2> {
    if let Some(token) = token
        && (session_id.is_none_or(|session| token.session_id() != session.as_str())
            || token.item_id() != item_id.as_str())
    {
        return Err(EvidenceReadErrorV2::TokenNotBound);
    }
    Ok(())
}

/// Serves one bounded page of a selected evidence item under the current consumer attempt.
///
/// The read is fenced by the session, the consumer attempt, the declared reference, and, when a
/// token is supplied, the exact value digest the token was issued against. A token never widens
/// what may be read: it only names where a previous page stopped.
pub fn project_evidence_read_v1(
    view: &GraphWorkspaceViewV2,
    source: &GraphNodeId,
    item_id: &ItemId,
    token: Option<&EvidencePageTokenV1>,
) -> Result<Map<String, Value>, EvidenceReadErrorV2> {
    project_evidence_read_in(&ReadScope::from_graph(view), source, item_id, token)
}

pub(crate) fn project_evidence_read_in(
    scope: &ReadScope<'_>,
    source: &GraphNodeId,
    item_id: &ItemId,
    token: Option<&EvidencePageTokenV1>,
) -> Result<Map<String, Value>, EvidenceReadErrorV2> {
    check_evidence_token_binding_v1(
        scope.graph_state().map(|state| state.trace().session_id()),
        item_id,
        token,
    )?;
    let state = scope.graph()?;
    let active = state
        .trace()
        .active_attempt()
        .ok_or(GraphViewErrorV2::TerminalSessionHasNoNext)?;

    if let Some(token) = token {
        // A changed consumer attempt is a moved snapshot, not a forged token: ADR-0024 makes it
        // retryable so a caller re-reads state and continues rather than treating it as malformed.
        if token.consumer_attempt_id() != active.attempt_id().as_str() {
            return Err(EvidenceReadErrorV2::TokenStale {
                graph_node_id: active.graph_node_id().as_str().to_owned(),
                source_graph_node_id: source.as_str().to_owned(),
                item_id: item_id.as_str().to_owned(),
                expected_value_digest: token.value_digest(),
                current_value_digest: None,
            });
        }
    }

    let selected = state
        .selected_evidence_item(active.attempt_id(), source, item_id)
        .map_err(|_| GraphViewErrorV2::InconsistentState("evidence lookup failed"))?;
    let selected = match selected {
        Ok(selected) => selected,
        Err(EvidenceLookupErrorV2::NotAvailable { reference_state }) => {
            return Err(EvidenceReadErrorV2::NotAvailable {
                graph_node_id: active.graph_node_id().as_str().to_owned(),
                source_graph_node_id: source.as_str().to_owned(),
                item_id: item_id.as_str().to_owned(),
                reference_state,
            });
        }
        Err(EvidenceLookupErrorV2::Stale { source_attempt_id }) => {
            // Without a token there is no page-token snapshot to be stale, so this is the
            // reference itself and keeps the code the specification already publishes for it.
            return Err(match token {
                Some(token) => EvidenceReadErrorV2::TokenStale {
                    graph_node_id: active.graph_node_id().as_str().to_owned(),
                    source_graph_node_id: source.as_str().to_owned(),
                    item_id: item_id.as_str().to_owned(),
                    expected_value_digest: token.value_digest(),
                    current_value_digest: None,
                },
                None => EvidenceReadErrorV2::ReferenceStale {
                    graph_node_id: active.graph_node_id().as_str().to_owned(),
                    source_graph_node_id: source.as_str().to_owned(),
                    expected_source_attempt_id: source_attempt_id.as_str().to_owned(),
                },
            });
        }
        // A reference the consumer does not declare, an item its selector excludes, and a declared
        // item that holds no value are all "there is nothing here to read" for this attempt.
        Err(EvidenceLookupErrorV2::ReferenceNotDeclared)
        | Err(EvidenceLookupErrorV2::ItemNotSelected)
        | Err(EvidenceLookupErrorV2::ValueAbsent) => {
            return Err(EvidenceReadErrorV2::NotAvailable {
                graph_node_id: active.graph_node_id().as_str().to_owned(),
                source_graph_node_id: source.as_str().to_owned(),
                item_id: item_id.as_str().to_owned(),
                reference_state: "unresolved",
            });
        }
    };

    let mut offset = 0u64;
    if let Some(token) = token {
        if token.source_attempt_id() != selected.source_attempt_id().as_str()
            || token.value_digest() != selected.value_digest().as_str()
        {
            return Err(EvidenceReadErrorV2::TokenStale {
                graph_node_id: selected.consumer_graph_node_id().as_str().to_owned(),
                source_graph_node_id: selected.source_graph_node_id().as_str().to_owned(),
                item_id: selected.item_id().as_str().to_owned(),
                expected_value_digest: token.value_digest(),
                current_value_digest: Some(selected.value_digest().as_str().to_owned()),
            });
        }
        offset = token.offset();
    }

    // The logical size is measured once for the admitted value: the page helper reuses that total
    // instead of recomputing it, and both the published fields and the exhausted error report the
    // same count.
    let (total_size, size_unit) = evidence_logical_size_v1(selected.value());
    let page = evidence_page_v1(selected.value(), offset, total_size).ok_or_else(|| {
        EvidenceReadErrorV2::TokenExhausted {
            source_graph_node_id: selected.source_graph_node_id().as_str().to_owned(),
            item_id: selected.item_id().as_str().to_owned(),
            offset,
            total_size,
            unit: size_unit,
        }
    })?;

    let next_page_token = if page.truncated {
        Some(
            EvidencePageTokenV1::new(
                state.trace().session_id().as_str(),
                selected.consumer_attempt_id().as_str(),
                selected.source_attempt_id().as_str(),
                selected.item_id().as_str(),
                selected.value_digest().as_str(),
                offset + page.size,
            )
            .map_err(|_| GraphViewErrorV2::InconsistentState("page token is not constructible"))?
            .encode(),
        )
    } else {
        None
    };

    let mut result = Map::new();
    result.insert("schema".to_owned(), json!("podway.evidence-read-result/v1"));
    result.insert("procedure_schema".to_owned(), json!(PROCEDURE_SCHEMA_V2));
    result.insert(
        "consumer".to_owned(),
        json!({
            "graph_node_id": selected.consumer_graph_node_id().as_str(),
            "attempt_id": selected.consumer_attempt_id().as_str(),
            "attempt_number": selected.consumer_attempt_number().get(),
        }),
    );
    let source_procedure = scope.procedure()?;
    let source_title =
        definition_title_for_node(&source_procedure, selected.source_graph_node_id().as_str())
            .ok_or(GraphViewErrorV2::InconsistentState(
                "evidence source definition is absent",
            ))?;
    result.insert(
        "source".to_owned(),
        json!({
            "graph_node_id": selected.source_graph_node_id().as_str(),
            "title": source_title,
            "attempt_id": selected.source_attempt_id().as_str(),
            "attempt_number": selected.source_attempt_number().get(),
            "items_digest": selected.items_digest().as_str(),
        }),
    );
    result.insert(
        "item".to_owned(),
        json!({
            "item_id": selected.item_id().as_str(),
            "type": item_type(selected.value().item_type()),
            "revision": selected.item_revision().get(),
        }),
    );
    result.insert(
        "value_digest".to_owned(),
        json!(selected.value_digest().as_str()),
    );
    result.insert("total_size".to_owned(), json!(total_size));
    result.insert("size_unit".to_owned(), json!(size_unit));
    result.insert(
        "page".to_owned(),
        json!({"offset": offset, "size": page.size, "data": page.data}),
    );
    result.insert("truncated".to_owned(), json!(page.truncated));
    result.insert(
        "next_page_token".to_owned(),
        next_page_token.map_or(Value::Null, Value::String),
    );
    result.insert(
        "page_token_version".to_owned(),
        json!(EVIDENCE_PAGE_TOKEN_VERSION_V1),
    );

    // ADR-0024 and AUT-EVR-004 bound the complete response, not only its page data. That bound sits
    // far below the 1 MiB output cap, so the aggregate cap cannot prove it and it is measured here. What is
    // measured is the result member; the outer envelope adds a request id, command, workspace, and
    // warnings, which the 64 KiB between the page bound and this one covers many times over, and a
    // test asserts the encoded envelope on a maximal page. A page is built to fit, so exceeding
    // this is an inconsistency rather than something to trim: trimming would contradict the size
    // and offset the same response publishes.
    if serialized_object_bytes_v1(&result) > MAX_EVIDENCE_RESPONSE_BYTES_V2 {
        return Err(EvidenceReadErrorV2::View(
            GraphViewErrorV2::InconsistentState(
                "an evidence read response exceeded its published byte bound",
            ),
        ));
    }
    Ok(result)
}

struct EvidencePageV1 {
    size: u64,
    data: Value,
    truncated: bool,
}

/// The logical size of a recorded value and the unit that size counts.
fn evidence_logical_size_v1(value: &RecordedItemValueV2) -> (u64, &'static str) {
    if let Some(text) = value.as_text() {
        (text.chars().count() as u64, "scalars")
    } else if let Some(entries) = value.as_list() {
        (entries.len() as u64, "entries")
    } else if value.item_type() == ItemTypeV1::CheckResult {
        (check_result_encoded_bytes_v1(value) as u64, "bytes")
    } else {
        (1, "value")
    }
}

/// Slices one recorded value into the page starting at `offset`, or `None` when the offset is at or
/// past the logical end.
///
/// Text pages end only at a Unicode scalar boundary and list pages carry whole entries, so a page
/// can always be concatenated with its successors to reproduce the value exactly. `total` is the
/// caller's logical size of the same admitted value, so the text arm allocates only the returned
/// page text rather than materializing the whole value as scalars.
fn evidence_page_v1(
    value: &RecordedItemValueV2,
    offset: u64,
    total: u64,
) -> Option<EvidencePageV1> {
    if let Some(text) = value.as_text() {
        let start = usize::try_from(offset).ok()?;
        // An empty value is complete at offset zero. Refusing it would make a legally recorded
        // value unreadable and would report a token failure to a caller that supplied no token.
        if total == 0 && start == 0 {
            return Some(EvidencePageV1 {
                size: 0,
                data: Value::String(String::new()),
                truncated: false,
            });
        }
        if start as u64 >= total {
            return None;
        }
        // At least one scalar always fits, so a page can never be empty and stall a caller.
        let capacity = (MAX_EVIDENCE_PAGE_BYTES_V2 / EVIDENCE_SCALAR_BYTES).max(1);
        let end = total.min(start as u64 + capacity as u64) as usize;
        let byte_start = text
            .char_indices()
            .nth(start)
            .expect("a scalar index below the logical count is present")
            .0;
        let page: String = text[byte_start..].chars().take(end - start).collect();
        return Some(EvidencePageV1 {
            size: (end - start) as u64,
            data: Value::String(page),
            truncated: (end as u64) < total,
        });
    }
    if let Some(entries) = value.as_list() {
        let start = usize::try_from(offset).ok()?;
        if entries.is_empty() && start == 0 {
            return Some(EvidencePageV1 {
                size: 0,
                data: Value::Array(Vec::new()),
                truncated: false,
            });
        }
        if start >= entries.len() {
            return None;
        }
        let mut charged = 0usize;
        let mut end = start;
        while end < entries.len() {
            let cost = EVIDENCE_ENTRY_OVERHEAD_BYTES
                + entries[end].chars().count() * EVIDENCE_SCALAR_BYTES;
            // The first entry is always admitted: one maximal entry is bounded well inside a page.
            if end > start && charged + cost > MAX_EVIDENCE_PAGE_BYTES_V2 {
                break;
            }
            charged += cost;
            end += 1;
        }
        return Some(EvidencePageV1 {
            size: (end - start) as u64,
            data: Value::Array(
                entries[start..end]
                    .iter()
                    .map(|entry| json!(entry))
                    .collect(),
            ),
            truncated: end < entries.len(),
        });
    }
    if value.item_type() == ItemTypeV1::CheckResult {
        if offset != 0 {
            return None;
        }
        // The logical size of a check result is its encoded byte count, already measured once
        // for this value; the page reports that same total as its size.
        return Some(EvidencePageV1 {
            size: total,
            data: typed_item_value(value),
            truncated: false,
        });
    }
    // Confirm, choice, integer, and artifact values are indivisible single-page reads.
    if offset != 0 {
        return None;
    }
    Some(EvidencePageV1 {
        size: 1,
        data: typed_item_value(value),
        truncated: false,
    })
}

pub fn project_graph_next_v2(
    view: &GraphWorkspaceViewV2,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    project_graph_next_in(&ReadScope::from_graph(view))
}

pub(crate) fn project_graph_next_in(
    scope: &ReadScope<'_>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let state = scope.graph()?;
    if state.trace().lifecycle() == SessionLifecycle::Prepared {
        let procedure = scope.procedure()?;
        return project_prepared_next_v1(state, &procedure);
    }
    let procedure = scope.procedure()?;
    let current = CurrentProjection::derive(scope, state, &procedure)?
        .ok_or(GraphViewErrorV2::TerminalSessionHasNoNext)?;
    project_running_next(
        scope,
        state,
        &procedure,
        &current,
        EvidencePreviewDisplay::Publish,
    )
}

/// Builds the running-session `next` projection.
///
/// `display` decides what happens to a preview the slot gate admits: `Publish` materializes its
/// payload for `session.next`, while `Suppress` evaluates the same admission and its gated
/// checks for observation guidance without constructing the preview text it must not carry.
fn project_running_next(
    scope: &ReadScope<'_>,
    state: &GraphSessionStateV2,
    procedure: &ParsedProcedureV2,
    current: &CurrentProjection<'_>,
    display: EvidencePreviewDisplay,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let delegated = delegated_placement_count(procedure) != 0;
    let mut result = Map::new();
    result.insert("schema".to_owned(), json!("podway.next-result/v5"));
    if delegated {
        result.insert(
            "delegation".to_owned(),
            delegation_projection(scope, state, Some(current)),
        );
    }
    result.insert("procedure_schema".to_owned(), json!("podway.procedure/v2"));
    result.insert(
        "procedure_digest".to_owned(),
        json!(state.snapshot().digest().as_str()),
    );
    result.insert(
        "goal_tracking".to_owned(),
        json!(state.snapshot().goal_tracking()),
    );
    result.insert(
        "goal_defined".to_owned(),
        json!(state.goal_state().current_revision().is_some()),
    );
    add_goal_summary(&mut result, state);
    if let Some(goal) = goal_display(state, Some(current)) {
        result.insert("goal".to_owned(), goal);
    }
    result.insert("node".to_owned(), current.node_identity());
    result.insert("attempt".to_owned(), current.attempt_identity());
    result.insert(
        "trace_length".to_owned(),
        json!(state.trace().attempts().len()),
    );
    result.insert("counters".to_owned(), counters(state));
    result.insert("revision".to_owned(), json!(state.trace().revision().get()));
    result.insert("readiness".to_owned(), current.readiness_value());
    result.insert(
        "missing_required_item_count".to_owned(),
        json!(current.missing_required.len()),
    );
    let missing_items = current.missing_items();
    let missing_total = current.missing_required.len();
    result.insert(
        "missing_required_items_truncated".to_owned(),
        json!(missing_items.len() < missing_total),
    );
    result.insert(
        "missing_required_items".to_owned(),
        Value::Array(missing_items),
    );
    result.insert(
        "blockers_total".to_owned(),
        json!(current.open_blockers.len()),
    );
    let (blockers, blockers_truncated) = current.blocker_window()?;
    result.insert("blockers".to_owned(), Value::Array(blockers));
    result.insert("blockers_truncated".to_owned(), json!(blockers_truncated));
    result.insert(
        "references".to_owned(),
        Value::Array(current.reference_metadata(procedure)?),
    );
    let (readback_values, readback_items_total) = readback(state, current, procedure, display)?;
    result.insert("readback".to_owned(), Value::Array(readback_values));
    result.insert(
        "readback_items_total".to_owned(),
        json!(readback_items_total),
    );
    // Vetting refuses a declaration whose selection exceeds the published record ceiling, so an
    // admitted Procedure always fits and this flag is false by construction. It stays in the
    // contract because a window is only honest when it can say it was cut.
    result.insert("readback_items_truncated".to_owned(), json!(false));
    result.insert(
        "allowed_manual_rework_targets".to_owned(),
        manual_rework_targets(state, procedure),
    );

    match current.definition {
        ParsedNodeDefinition::Action(definition) => {
            result.insert("title".to_owned(), json!(definition.title()));
            result.insert("intent".to_owned(), json!(definition.intent()));
            if let Some(description) = definition.description() {
                result.insert("description".to_owned(), json!(description));
            }
            result.insert("instructions".to_owned(), json!(definition.instructions()));
            add_static_disposition(&mut result, Some(current));
        }
        ParsedNodeDefinition::Decision(definition) => {
            result.insert("title".to_owned(), json!(definition.title()));
            result.insert("objective".to_owned(), json!(definition.objective()));
            result.insert("prompt".to_owned(), json!(definition.prompt()));
            if let Some(description) = definition.description() {
                result.insert("description".to_owned(), json!(description));
            }
            let allowed_options = current.allowed_option_ids();
            result.insert(
                "options".to_owned(),
                Value::Array(
                    definition
                        .options()
                        .iter()
                        .map(|option| {
                            let mut value = Map::new();
                            value.insert("option_id".to_owned(), json!(option.id().as_str()));
                            value.insert("label".to_owned(), json!(option.label()));
                            if let Some(criteria) = option.criteria() {
                                value.insert("criteria".to_owned(), json!(criteria));
                            }
                            if let Some(guards) = option.guards() {
                                value
                                    .insert("guards".to_owned(), guards_authoring_value_v2(guards));
                            }
                            Value::Object(value)
                        })
                        .collect(),
                ),
            );
            result.insert(
                "allowed_option_ids".to_owned(),
                Value::Array(allowed_options),
            );
            if let Some(statuses) = current.option_guard_statuses() {
                result.insert("option_guard_statuses".to_owned(), Value::Array(statuses));
            }
            let mut policy = Map::new();
            policy.insert("required".to_owned(), Value::Bool(true));
            if let Some(prompt) = definition.reason().prompt() {
                policy.insert("prompt".to_owned(), json!(prompt));
            }
            result.insert("reason_policy".to_owned(), Value::Object(policy));
            if !definition.evidence_guidance().is_empty() {
                result.insert(
                    "evidence_guidance".to_owned(),
                    json!(definition.evidence_guidance()),
                );
            }
        }
    }
    let actions = current.allowed_actions(state, procedure);
    result.insert("allowed_actions".to_owned(), json!(actions));
    let suggestions = current.suggestions(state, &actions);
    // The window and its total are equal today because the suggestion builder truncates to the
    // same cap; both are emitted so a later widening cannot truncate silently.
    result.insert("suggestions_total".to_owned(), json!(suggestions.len()));
    result.insert("suggestions_truncated".to_owned(), json!(false));
    result.insert("suggestions".to_owned(), Value::Array(suggestions));
    Ok(result)
}

/// Project one self-contained automation observation from a single coherent Store view.
pub fn project_graph_observation_v1(
    view: &GraphWorkspaceViewV2,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    project_graph_observation_in(
        &ReadScope::from_graph(view),
        ContractGenerationV1::Generation1,
    )
}

/// Extracts the object map from an owned projection `Value` without copying it.
///
/// The `json!` macro finalizes every expression through `to_value`, and the extraction sites
/// this replaces cloned the resulting map back out of the value. Moving the map out removes
/// that copy while keeping the unreachable non-object miss explicit, exactly as the previous
/// `expect` messages did.
fn into_object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            panic!("projection result is not an object")
        }
    }
}

fn rehydrate_validated(
    state: &GraphSessionStateV2,
) -> Result<podway_config::ValidatedProcedureV2, GraphViewErrorV2> {
    let parsed = parse_procedure_document(
        state.snapshot().canonical_json().as_str().as_bytes(),
        ProcedureDocumentFormat::Json,
    )
    .map_err(|_| GraphViewErrorV2::InvalidSnapshot)?;
    let ParsedProcedure::V2(parsed) = parsed;
    validate_procedure_v2(parsed).map_err(|_| GraphViewErrorV2::InvalidSnapshot)
}

/// Request-scoped admission over one coherent view the caller already owns.
///
/// The graph and receipts are borrowed. This context does not clone
/// `GraphSessionStateV2` or build another workspace view. The first declaration
/// use parses and semantically validates once. Nested consumers of the same
/// request reuse that wrapper. Read projections keep `state.snapshot().digest()`;
/// package construction keeps `ValidatedProcedureV2::digest()`. Those identities
/// are not rewritten to match.
pub(crate) struct ReadScope<'a> {
    identity: &'a DurableWorktreeIdentityV1,
    graph: Option<&'a GraphSessionStateV2>,
    receipts: &'a DelegationReceiptSnapshotV1,
    current_terminal_disposition: bool,
    validated: RefCell<Option<podway_config::ValidatedProcedureV2>>,
}

impl<'a> ReadScope<'a> {
    pub(crate) fn from_graph(view: &'a GraphWorkspaceViewV2) -> Self {
        Self {
            identity: view.identity(),
            graph: view.graph_state(),
            receipts: view.delegation_receipts(),
            current_terminal_disposition: view.current_terminal_disposition(),
            validated: RefCell::new(None),
        }
    }

    pub(crate) fn from_direct(view: &'a DirectWorkspaceViewV1) -> Self {
        let current_terminal_disposition = view.graph().is_some_and(|graph| {
            view.terminal_dispositions().iter().any(|record| {
                record.session_id() == graph.trace().session_id()
                    && record.terminal_session_revision() == graph.trace().revision()
            })
        });
        Self {
            identity: view.binding().identity(),
            graph: view.graph(),
            receipts: view.delegation_receipts(),
            current_terminal_disposition,
            validated: RefCell::new(None),
        }
    }

    pub(crate) fn identity(&self) -> &'a DurableWorktreeIdentityV1 {
        self.identity
    }

    pub(crate) fn graph_state(&self) -> Option<&'a GraphSessionStateV2> {
        self.graph
    }

    fn graph(&self) -> Result<&'a GraphSessionStateV2, GraphViewErrorV2> {
        self.graph.ok_or(GraphViewErrorV2::MissingGraphSession)
    }

    pub(crate) fn receipts(&self) -> &'a DelegationReceiptSnapshotV1 {
        self.receipts
    }

    fn current_terminal_disposition(&self) -> bool {
        self.current_terminal_disposition
    }

    fn admit(&self) -> Result<(), GraphViewErrorV2> {
        if self.validated.borrow().is_some() {
            return Ok(());
        }
        let validated = rehydrate_validated(self.graph()?)?;
        *self.validated.borrow_mut() = Some(validated);
        Ok(())
    }

    fn validated(&self) -> Result<Ref<'_, podway_config::ValidatedProcedureV2>, GraphViewErrorV2> {
        self.admit()?;
        Ok(Ref::map(self.validated.borrow(), |slot| {
            slot.as_ref().expect("admitted procedure")
        }))
    }

    fn procedure(&self) -> Result<Ref<'_, ParsedProcedureV2>, GraphViewErrorV2> {
        self.admit()?;
        Ok(Ref::map(self.validated.borrow(), |slot| {
            slot.as_ref().expect("admitted procedure").parsed()
        }))
    }
}

struct CurrentProjection<'a> {
    state: &'a GraphSessionStateV2,
    attempt: &'a SessionAttemptV2,
    memory: &'a AttemptWorkflowMemoryV2,
    placement: &'a GraphPlacementV2,
    definition: &'a ParsedNodeDefinition,
    item_specs: &'a [ItemSpecV2],
    missing_required: Vec<&'a ItemSpecV2>,
    open_blockers: Vec<&'a podway_store::BlockerStateV2>,
    items_satisfied: bool,
    goal_ready: bool,
    evidence_ready: bool,
    evidence_readback: Vec<EvidenceReadbackV2>,
    delegation_readiness: DelegationReadinessV1,
}

impl<'a> CurrentProjection<'a> {
    fn derive(
        scope: &ReadScope<'_>,
        state: &'a GraphSessionStateV2,
        procedure: &'a ParsedProcedureV2,
    ) -> Result<Option<Self>, GraphViewErrorV2> {
        if state.trace().lifecycle() != SessionLifecycle::Running {
            return Ok(None);
        }
        let attempt = state
            .trace()
            .active_attempt()
            .ok_or(GraphViewErrorV2::InconsistentState(
                "running graph session has no active attempt",
            ))?;
        let memory = state
            .workflow_memory()
            .attempts()
            .iter()
            .find(|memory| memory.attempt_id() == attempt.attempt_id())
            .ok_or(GraphViewErrorV2::InconsistentState(
                "active attempt workflow memory is absent",
            ))?;
        let placement = procedure
            .graph()
            .placement(attempt.graph_node_id())
            .ok_or(GraphViewErrorV2::InvalidSnapshot)?;
        let definition = procedure
            .node_definitions()
            .iter()
            .find(|definition| match (definition, placement) {
                (ParsedNodeDefinition::Action(definition), GraphPlacementV2::Action(placement)) => {
                    definition.id() == placement.definition()
                }
                (
                    ParsedNodeDefinition::Decision(definition),
                    GraphPlacementV2::Decision(placement),
                ) => definition.id() == placement.definition(),
                _ => false,
            })
            .ok_or(GraphViewErrorV2::InvalidSnapshot)?;
        let item_specs = match definition {
            ParsedNodeDefinition::Action(definition) => definition.items(),
            ParsedNodeDefinition::Decision(definition) => definition.items(),
        };
        if item_specs.len() != memory.item_slots().len()
            || item_specs
                .iter()
                .zip(memory.item_slots())
                .any(|(spec, slot)| {
                    spec.id() != slot.item_id() || spec.item_type() != slot.item_type()
                })
        {
            return Err(GraphViewErrorV2::InconsistentState(
                "active item slots disagree with the immutable Procedure snapshot",
            ));
        }
        let values = memory
            .item_slots()
            .iter()
            .map(|slot| slot.value())
            .collect::<Vec<_>>();
        let missing_required = item_specs
            .iter()
            .zip(memory.item_slots())
            .filter_map(|(spec, slot)| {
                (spec.required_now(item_specs, &values)
                    && !slot
                        .value()
                        .is_some_and(|value| spec.is_satisfied_by(value)))
                .then_some(spec)
            })
            .collect::<Vec<_>>();
        let items_satisfied = item_specs
            .iter()
            .zip(memory.item_slots())
            .all(|(spec, slot)| {
                !spec.required_now(item_specs, &values)
                    || slot
                        .value()
                        .is_some_and(|value| spec.is_satisfied_by(value))
            });
        let open_blockers = memory
            .blockers()
            .iter()
            .filter(|blocker| blocker.state() == BlockerState::Open)
            .collect::<Vec<_>>();
        let terminal_action =
            matches!(placement, GraphPlacementV2::Action(action) if action.outcome().is_terminal());
        let goal_ready = match definition {
            ParsedNodeDefinition::Decision(definition) if definition.assessment().is_some() => {
                determined_goal_outcome(state, attempt).is_some()
            }
            _ => {
                !terminal_action
                    || !state.snapshot().goal_tracking()
                    || state
                        .goal_state()
                        .latest_fresh_assessment(state.trace())
                        .is_some()
            }
        };
        let evidence_readback = state
            .selected_evidence_readback(attempt.attempt_id())
            .map_err(|_| {
                GraphViewErrorV2::InconsistentState("selected evidence readback is invalid")
            })?;
        let evidence_ready = match placement {
            GraphPlacementV2::Action(_) => true,
            GraphPlacementV2::Decision(_) => evidence_readback.iter().all(|readback| {
                !readback.stale()
                    && (!readback.reference().required()
                        || matches!(
                            readback.reference().resolution(),
                            ResolvedEvidenceReferenceV2::Resolved(_)
                        ))
            }),
        };
        Ok(Some(Self {
            state,
            attempt,
            memory,
            placement,
            definition,
            item_specs,
            missing_required,
            open_blockers,
            items_satisfied,
            goal_ready,
            evidence_ready,
            evidence_readback,
            delegation_readiness: scope.receipts().readiness(),
        }))
    }

    fn identity(&self) -> Value {
        json!({
            "node": self.node_identity(),
            "attempt": self.attempt_identity(),
            "readiness": self.readiness_value(),
            "missing_required_item_count": self.missing_required.len(),
            "blockers_total": self.open_blockers.len(),
        })
    }

    fn node_identity(&self) -> Value {
        let definition = match self.definition {
            ParsedNodeDefinition::Action(definition) => definition.id(),
            ParsedNodeDefinition::Decision(definition) => definition.id(),
        };
        json!({
            "node_definition_id": definition.as_str(),
            "graph_node_id": self.attempt.graph_node_id().as_str(),
            "node_type": self.placement.node_kind().as_str(),
        })
    }

    fn attempt_identity(&self) -> Value {
        json!({
            "attempt_id": self.attempt.attempt_id().as_str(),
            "attempt_number": self.attempt.number().get(),
        })
    }

    fn can_advance(&self) -> bool {
        self.items_satisfied
            && self.open_blockers.is_empty()
            && self.goal_ready
            && self.evidence_ready
            && (!self.delegated()
                || self.delegation_readiness == DelegationReadinessV1::ReceiptCurrent)
    }

    fn delegated(&self) -> bool {
        matches!(self.placement, GraphPlacementV2::Action(action) if action.delegation().is_some())
    }

    fn readiness_value(&self) -> Value {
        json!({
            "items_satisfied": self.items_satisfied,
            "unblocked": self.open_blockers.is_empty(),
            "goal_ready": self.goal_ready,
            "can_advance": self.can_advance(),
        })
    }

    fn compact_items(&self) -> Vec<Value> {
        self.item_specs
            .iter()
            .zip(self.memory.item_slots())
            .map(|(spec, slot)| {
                json!({
                    "item_id": spec.id().as_str(),
                    "type": item_type(spec.item_type()),
                    "required": spec.common().required(),
                    "satisfied": slot.value().is_some_and(|value| spec.is_satisfied_by(value)),
                    "revision": slot.revision().get(),
                })
            })
            .collect()
    }

    /// The complete missing-item identifier set.
    ///
    /// Raising items per definition to 128 made this array able to exceed the 64 its own schema
    /// published. `status-result/v3` is a released, served family, so the schema was corrected to
    /// hold what a definition can declare rather than the array being windowed: windowing it would
    /// have needed a new required member, and a released family never grows one. Identifiers are
    /// small, so carrying all 128 costs less than the count and flag that would describe a cut.
    fn missing_ids(&self) -> Vec<Value> {
        self.missing_required
            .iter()
            .map(|item| json!(item.id().as_str()))
            .collect()
    }

    /// The bounded missing-item detail window.
    ///
    /// ADR-0024 keeps `missing_required_item_count` exact while this window stops at 64 entries, so
    /// raising items per node to 128 cannot make the detail array unbounded. The caller learns of
    /// the cut from `missing_required_items_truncated`, never silently.
    fn missing_items(&self) -> Vec<Value> {
        self.missing_required
            .iter()
            .take(MAX_MISSING_ITEM_WINDOW_V2)
            .map(|item| {
                json!({
                    "item_id": item.id().as_str(),
                    "prompt": item.common().prompt(),
                })
            })
            .collect()
    }

    /// The exact total counts every recorded value, while display construction happens only for
    /// candidates the byte budget can still consider: the first candidate that does not fit ends
    /// construction, and later values stay counted but unformatted.
    fn item_values(&self) -> (Vec<Value>, usize, bool) {
        let total = self
            .memory
            .item_slots()
            .iter()
            .filter(|slot| slot.value().is_some())
            .count();
        let mut used = 2_usize;
        let mut values = Vec::new();
        for slot in self.memory.item_slots() {
            let Some(value) = slot.value() else {
                continue;
            };
            let candidate = if value.item_type() == ItemTypeV1::CheckResult {
                json!({
                    "item_id": slot.item_id().as_str(),
                    "value": check_result_preview_v1(value),
                    "value_truncated": false,
                })
            } else {
                let display = display_item_value(value);
                let (display, value_truncated) = truncate_chars(&display, ITEM_DISPLAY_CHARS_MAX);
                json!({
                    "item_id": slot.item_id().as_str(),
                    "value": display,
                    "value_truncated": value_truncated,
                })
            };
            let bytes = serialized_json_len_v1(&candidate).unwrap_or(STATUS_VALUES_BYTES_MAX)
                + usize::from(!values.is_empty());
            if used.saturating_add(bytes) > STATUS_VALUES_BYTES_MAX {
                break;
            }
            used += bytes;
            values.push(candidate);
        }
        let truncated = values.len() < total;
        (values, total, truncated)
    }

    fn active_item_descriptors(&self) -> Vec<Value> {
        let values = self
            .memory
            .item_slots()
            .iter()
            .map(|slot| slot.value())
            .collect::<Vec<_>>();
        self.item_specs
            .iter()
            .zip(self.memory.item_slots())
            .map(|(spec, slot)| {
                let mut descriptor = Map::new();
                descriptor.insert("item_id".to_owned(), json!(spec.id().as_str()));
                descriptor.insert("type".to_owned(), json!(item_type(spec.item_type())));
                descriptor.insert("prompt".to_owned(), json!(spec.common().prompt()));
                if let Some(help) = spec.common().help() {
                    descriptor.insert("help".to_owned(), json!(help));
                }
                descriptor.insert("required".to_owned(), json!(spec.common().required()));
                descriptor.insert(
                    "required_now".to_owned(),
                    json!(spec.required_now(self.item_specs, &values)),
                );
                if let Some(status) = spec.condition_status(self.item_specs, &values) {
                    descriptor.insert(
                        "condition_status".to_owned(),
                        condition_status_value_v2(&status),
                    );
                }
                descriptor.insert(
                    "satisfied".to_owned(),
                    json!(
                        slot.value()
                            .is_some_and(|value| spec.is_satisfied_by(value))
                    ),
                );
                if let Some(reason) = slot
                    .value()
                    .and_then(|value| spec.check_result_unsatisfied_reason(value))
                {
                    descriptor.insert("unsatisfied_reason".to_owned(), json!(reason.as_str()));
                }
                descriptor.insert("revision".to_owned(), json!(slot.revision().get()));
                descriptor.insert("constraints".to_owned(), item_constraints(spec));
                let (value, value_truncated) = slot
                    .value()
                    .map(project_observation_item_value)
                    .unwrap_or((Value::Null, false));
                descriptor.insert("value".to_owned(), value);
                descriptor.insert("value_truncated".to_owned(), json!(value_truncated));
                Value::Object(descriptor)
            })
            .collect()
    }

    fn mutation_templates(
        &self,
        scope: &ReadScope<'_>,
        state: &GraphSessionStateV2,
        procedure: &ParsedProcedureV2,
    ) -> Vec<Value> {
        let actions = self.allowed_actions(state, procedure);
        let mut recipes = self.suggestions(state, &actions);
        for (spec, slot) in self.item_specs.iter().zip(self.memory.item_slots()) {
            let item_id = spec.id().as_str();
            let set_placeholder = match spec.item_type() {
                ItemTypeV1::Text => Some(("item.set", "set", "<text>")),
                ItemTypeV1::Choice => Some(("item.set", "set", "<choice>")),
                ItemTypeV1::Integer => Some(("item.set", "set", "<integer>")),
                ItemTypeV1::Artifact => Some(("item.attach", "attach", "<path>")),
                ItemTypeV1::Confirm | ItemTypeV1::List | ItemTypeV1::CheckResult => None,
            };
            if let Some((command, verb, placeholder)) = set_placeholder {
                push_recipe_unique(
                    &mut recipes,
                    json!({
                        "command": command,
                        "argv": ["podway", verb, item_id, placeholder],
                        "item_id": item_id,
                    }),
                );
            }
            if spec.item_type() == ItemTypeV1::Confirm {
                push_recipe_unique(
                    &mut recipes,
                    json!({"command":"item.check","argv":["podway","check",item_id],"item_id":item_id}),
                );
            }
            if let ItemSpecV2::List(list) = spec {
                let count = slot
                    .value()
                    .and_then(RecordedItemValueV2::as_list)
                    .map_or(0, <[String]>::len);
                if count < usize::from(list.max_items()) {
                    push_recipe_unique(
                        &mut recipes,
                        json!({"command":"item.add","argv":["podway","add",item_id,"<value>"],"item_id":item_id}),
                    );
                }
                if count != 0 {
                    push_recipe_unique(
                        &mut recipes,
                        json!({"command":"item.remove","argv":["podway","remove",item_id,"<value>"],"item_id":item_id}),
                    );
                }
            }
            if slot.value().is_some() {
                if spec.item_type() == ItemTypeV1::Confirm {
                    push_recipe_unique(
                        &mut recipes,
                        json!({"command":"item.uncheck","argv":["podway","uncheck",item_id],"item_id":item_id}),
                    );
                }
                push_recipe_unique(
                    &mut recipes,
                    json!({"command":"item.clear","argv":["podway","clear",item_id],"item_id":item_id}),
                );
            }
        }
        if actions.contains(&"session.block") {
            push_recipe_unique(
                &mut recipes,
                json!({"command":"session.block","argv":["podway","block","--reason","<reason>"]}),
            );
        }
        if actions.contains(&"session.unblock") {
            push_recipe_unique(
                &mut recipes,
                json!({"command":"session.unblock","argv":["podway","unblock","<blocker-id>"]}),
            );
        }
        push_recipe_unique(
            &mut recipes,
            json!({"command":"session.cancel","argv":["podway","cancel","--reason","<reason>"]}),
        );
        push_recipe_unique(
            &mut recipes,
            json!({
                "command":"session.reset",
                "argv":["podway","reset","--progress-summary","<progress-summary>","--yes"]
            }),
        );
        if actions.contains(&"session.rework") {
            push_recipe_unique(
                &mut recipes,
                json!({"command":"session.rework","argv":["podway","rework","--to","<graph-node-id>","--reason","<reason>"]}),
            );
        }
        if actions.contains(&"goal.revise") {
            push_recipe_unique(
                &mut recipes,
                json!({
                    "command":"goal.revise",
                    "argv":["podway","goal","revise","--goal","<goal>","--criterion","<criterion>","--rework-to","<graph-node-id>","--reason","<reason>"]
                }),
            );
        }
        recipes
            .into_iter()
            .filter_map(|suggestion| {
                let object = suggestion.as_object()?;
                let command = object.get("command")?.as_str()?;
                let mut argv = object.get("argv")?.as_array()?.clone();
                append_flag(
                    &mut argv,
                    "--if-workspace-uuid",
                    scope.identity().workspace_uuid().as_str(),
                );
                append_flag(
                    &mut argv,
                    "--if-session-id",
                    state.trace().session_id().as_str(),
                );

                let mut fences = Map::new();
                fences.insert(
                    "workspace_uuid".to_owned(),
                    json!(scope.identity().workspace_uuid().as_str()),
                );
                fences.insert(
                    "session_id".to_owned(),
                    json!(state.trace().session_id().as_str()),
                );
                if command.starts_with("item.") {
                    let item_id = object.get("item_id")?.as_str()?;
                    let slot = self
                        .memory
                        .item_slots()
                        .iter()
                        .find(|slot| slot.item_id().as_str() == item_id)?;
                    append_flag(
                        &mut argv,
                        "--if-attempt",
                        self.attempt.attempt_id().as_str(),
                    );
                    append_flag(
                        &mut argv,
                        "--if-item-revision",
                        &slot.revision().get().to_string(),
                    );
                    fences.insert(
                        "attempt_id".to_owned(),
                        json!(self.attempt.attempt_id().as_str()),
                    );
                    fences.insert("item_revision".to_owned(), json!(slot.revision().get()));
                    if command == "item.record_many" {
                        fences.insert(
                            "session_revision".to_owned(),
                            json!(state.trace().revision().get()),
                        );
                        let ItemSpecV2::CheckResult(specification) = self
                            .item_specs
                            .iter()
                            .find(|specification| specification.id().as_str() == item_id)?
                        else {
                            return None;
                        };
                        let stdin_template = json!({
                            "schema": "podway.item-record-many-input/v1",
                            "workspace_uuid": scope.identity().workspace_uuid().as_str(),
                            "session_id": state.trace().session_id().as_str(),
                            "session_revision": state.trace().revision().get(),
                            "attempt_id": self.attempt.attempt_id().as_str(),
                            "idempotency_key": "<idempotency-key>",
                            "operations": [{
                                "item_id": item_id,
                                "expected_item_revision": slot.revision().get(),
                                "record": {
                                    "type": "check_result",
                                    "operation_id": specification.operation_id().as_str(),
                                    "operation_digest": specification.operation_digest().as_str(),
                                    "input_basis": {
                                        "descriptor": "<input-basis-descriptor>",
                                        "digest": "<input-basis-digest>",
                                    },
                                    "executor": {
                                        "name": "<executor-name>",
                                        "version": "<executor-version>",
                                    },
                                    "outcome": "<outcome>",
                                    "summary": "<summary>",
                                    "output_digest": "<output-digest>",
                                },
                            }],
                        });
                        return Some(json!({
                            "command": command,
                            "argv": ["podway", "record", "--stdin"],
                            "preconditions": fences,
                            "authority": "optimistic_concurrency_only",
                            "idempotency_key_required": true,
                            "requires_explicit_authorization": false,
                            "stdin_template": serde_json::to_string_pretty(&stdin_template).ok()?,
                        }));
                    }
                } else {
                    append_flag(
                        &mut argv,
                        "--if-session-revision",
                        &state.trace().revision().get().to_string(),
                    );
                    fences.insert(
                        "session_revision".to_owned(),
                        json!(state.trace().revision().get()),
                    );
                    if matches!(
                        command,
                        "session.complete"
                            | "session.decide"
                            | "session.retry"
                            | "session.skip"
                            | "session.rework"
                            | "session.block"
                            | "session.unblock"
                            | "session.cancel"
                            | "goal.revise"
                            | "goal.assess_criterion"
                    ) {
                        append_flag(
                            &mut argv,
                            "--if-attempt",
                            self.attempt.attempt_id().as_str(),
                        );
                        fences.insert(
                            "attempt_id".to_owned(),
                            json!(self.attempt.attempt_id().as_str()),
                        );
                    }
                    if matches!(
                        command,
                        "session.decide" | "goal.revise" | "goal.assess_criterion"
                    ) && let Some(revision) = state.goal_state().current_revision()
                    {
                        append_flag(&mut argv, "--if-goal-revision", &revision.get().to_string());
                        fences.insert("goal_revision".to_owned(), json!(revision.get()));
                    }
                }
                append_flag(&mut argv, "--idempotency-key", "<idempotency-key>");
                Some(json!({
                    "command": command,
                    "argv": argv,
                    "preconditions": fences,
                    "authority": "optimistic_concurrency_only",
                    "idempotency_key_required": true,
                    "requires_explicit_authorization": matches!(
                        command,
                        "goal.define" | "goal.revise" | "session.cancel" | "session.reset"
                    ),
                }))
            })
            .collect()
    }

    fn blocker_window(&self) -> Result<(Vec<Value>, bool), GraphViewErrorV2> {
        let mut blockers = self.open_blockers.clone();
        blockers.sort_by_key(|blocker| std::cmp::Reverse(blocker.created_at()));
        let mut used = 2_usize;
        let mut projected = Vec::new();
        for blocker in blockers {
            let value = json!({
                "blocker_id": blocker.blocker_id().as_str(),
                "reason": blocker.reason(),
                "created_at": timestamp(blocker.created_at())?,
            });
            let bytes = serialized_json_len_v1(&value)
                .map_err(|_| GraphViewErrorV2::InconsistentState("blocker view is not JSON"))?
                + usize::from(!projected.is_empty());
            if used.saturating_add(bytes) > BLOCKER_WINDOW_BYTES_MAX {
                break;
            }
            used += bytes;
            projected.push(value);
        }
        let truncated = projected.len() < self.open_blockers.len();
        Ok((projected, truncated))
    }

    fn reference_metadata(
        &self,
        procedure: &ParsedProcedureV2,
    ) -> Result<Vec<Value>, GraphViewErrorV2> {
        self.memory
            .evidence()
            .iter()
            .map(|reference| reference_metadata(reference, procedure, false))
            .collect()
    }

    fn allowed_option_ids(&self) -> Vec<Value> {
        if !self.can_advance() {
            return Vec::new();
        }
        match self.definition {
            ParsedNodeDefinition::Decision(definition) => {
                if let Some(assessment) = definition.assessment() {
                    let Some(outcome) = determined_goal_outcome_for_current(self) else {
                        return Vec::new();
                    };
                    definition
                        .options()
                        .iter()
                        .filter(|option| {
                            assessment.outcomes().iter().any(|mapping| {
                                mapping.option_id() == option.id() && mapping.outcome() == outcome
                            }) && self.option_guard_met(option)
                        })
                        .map(|option| json!(option.id().as_str()))
                        .collect()
                } else {
                    definition
                        .options()
                        .iter()
                        .filter(|option| self.option_guard_met(option))
                        .map(|option| json!(option.id().as_str()))
                        .collect()
                }
            }
            ParsedNodeDefinition::Action(_) => Vec::new(),
        }
    }

    fn option_guard_met(&self, option: &podway_core::DecisionOptionV2) -> bool {
        option.guards().is_none_or(|guards| {
            guards
                .evaluate(|source, item| self.evidence_value(source, item))
                .state()
                == ConditionStateV2::Met
        })
    }

    fn evidence_value(&self, source: &GraphNodeId, item: &ItemId) -> Option<&RecordedItemValueV2> {
        self.evidence_readback
            .iter()
            .filter(|readback| {
                !readback.stale()
                    && readback.reference().resolution().source_node() == source
                    && matches!(
                        readback.reference().resolution(),
                        ResolvedEvidenceReferenceV2::Resolved(_)
                    )
            })
            .find_map(|readback| {
                readback
                    .items()
                    .items()
                    .iter()
                    .find(|recorded| recorded.id() == item)
            })
            .map(podway_core::RecordedItemV2::value)
    }

    fn option_guard_statuses(&self) -> Option<Vec<Value>> {
        let ParsedNodeDefinition::Decision(definition) = self.definition else {
            return None;
        };
        definition
            .options()
            .iter()
            .any(|option| option.guards().is_some())
            .then(|| {
                definition
                    .options()
                    .iter()
                    .map(|option| {
                        let (state, predicates) = option.guards().map_or_else(
                            || (ConditionStateV2::Met, Vec::new()),
                            |guards| {
                                let status = guards
                                    .evaluate(|source, item| self.evidence_value(source, item));
                                let predicates = guards
                                    .predicates()
                                    .iter()
                                    .zip(status.predicates())
                                    .map(|(predicate, status)| {
                                        evidence_predicate_status_value_v2(predicate, status)
                                    })
                                    .collect();
                                (status.state(), predicates)
                            },
                        );
                        json!({
                            "option_id": option.id().as_str(),
                            "state": state.as_str(),
                            "predicates": predicates,
                        })
                    })
                    .collect()
            })
    }

    fn allowed_actions(
        &self,
        state: &GraphSessionStateV2,
        procedure: &ParsedProcedureV2,
    ) -> Vec<&'static str> {
        let mut actions = Vec::new();
        for (spec, slot) in self.item_specs.iter().zip(self.memory.item_slots()) {
            match spec {
                ItemSpecV2::Confirm(_) => push_unique(&mut actions, "item.check"),
                ItemSpecV2::Text(_) | ItemSpecV2::Choice(_) | ItemSpecV2::Integer(_) => {
                    push_unique(&mut actions, "item.set");
                }
                ItemSpecV2::List(list) => {
                    let item_count = slot
                        .value()
                        .and_then(RecordedItemValueV2::as_list)
                        .map_or(0, <[String]>::len);
                    if item_count < usize::from(list.max_items()) {
                        push_unique(&mut actions, "item.add");
                    }
                    if item_count != 0 {
                        push_unique(&mut actions, "item.remove");
                    }
                }
                ItemSpecV2::Artifact(_) => push_unique(&mut actions, "item.attach"),
                ItemSpecV2::CheckResult(_) => {
                    if !slot
                        .value()
                        .is_some_and(|value| spec.is_satisfied_by(value))
                    {
                        push_unique(&mut actions, "item.record_many");
                    }
                }
            }
            if slot.value().is_some() {
                if spec.item_type() == ItemTypeV1::Confirm {
                    push_unique(&mut actions, "item.uncheck");
                }
                push_unique(&mut actions, "item.clear");
            }
        }
        if self.can_advance() {
            match self.placement {
                GraphPlacementV2::Action(_) => push_unique(&mut actions, "session.complete"),
                GraphPlacementV2::Decision(_) if !self.allowed_option_ids().is_empty() => {
                    push_unique(&mut actions, "session.decide");
                }
                GraphPlacementV2::Decision(_) => {}
            }
        }
        if matches!(self.placement, GraphPlacementV2::Action(action) if action.skip().is_some() && (!action.outcome().is_terminal() || self.goal_ready))
        {
            push_unique(&mut actions, "session.skip");
        }
        push_unique(&mut actions, "session.retry");
        if self.open_blockers.len() < 64 {
            push_unique(&mut actions, "session.block");
        }
        if !self.open_blockers.is_empty() {
            push_unique(&mut actions, "session.unblock");
        }
        push_unique(&mut actions, "session.cancel");
        push_unique(&mut actions, "session.reset");
        if manual_rework_targets(state, procedure)
            .as_array()
            .is_some_and(|targets| !targets.is_empty())
        {
            push_unique(&mut actions, "session.rework");
        }
        if state.snapshot().goal_tracking() {
            if state.goal_state().current_revision().is_some() {
                if goal_revision_rework_targets(state, procedure)
                    .as_array()
                    .is_some_and(|targets| !targets.is_empty())
                {
                    push_unique(&mut actions, "goal.revise");
                }
            } else {
                push_unique(&mut actions, "goal.define");
            }
            if matches!(self.definition, ParsedNodeDefinition::Decision(definition) if definition.assessment().is_some())
                && unassessed_goal_criteria(state, self.attempt)
                    .is_some_and(|criteria| !criteria.is_empty())
            {
                push_unique(&mut actions, "goal.assess_criterion");
            }
        }
        actions
    }

    fn check_result_target_index(&self) -> Option<usize> {
        let values = self
            .memory
            .item_slots()
            .iter()
            .map(|slot| slot.value())
            .collect::<Vec<_>>();
        [true, false].into_iter().find_map(|required| {
            self.item_specs
                .iter()
                .zip(self.memory.item_slots())
                .enumerate()
                .find_map(|(index, (specification, slot))| {
                    (specification.item_type() == ItemTypeV1::CheckResult
                        && specification.required_now(self.item_specs, &values) == required
                        && !slot
                            .value()
                            .is_some_and(|value| specification.is_satisfied_by(value)))
                    .then_some(index)
                })
        })
    }

    fn suggestions(&self, state: &GraphSessionStateV2, actions: &[&str]) -> Vec<Value> {
        let mut suggestions = Vec::new();
        if let Some(index) = self.check_result_target_index() {
            suggestions.push(json!({
                "command": "item.record_many",
                "argv": ["podway", "record", "--stdin"],
                "item_id": self.item_specs[index].id().as_str(),
            }));
        }
        for item in &self.missing_required {
            if item.item_type() == ItemTypeV1::CheckResult {
                continue;
            }
            let (command, verb, placeholder) = match item.item_type() {
                ItemTypeV1::Confirm => ("item.check", "check", None),
                ItemTypeV1::Text => ("item.set", "set", Some("<text>")),
                ItemTypeV1::Choice => ("item.set", "set", Some("<choice>")),
                ItemTypeV1::Integer => ("item.set", "set", Some("<integer>")),
                ItemTypeV1::List => ("item.add", "add", Some("<value>")),
                ItemTypeV1::Artifact => ("item.attach", "attach", Some("<path>")),
                ItemTypeV1::CheckResult => unreachable!("handled above"),
            };
            let mut argv = vec![json!("podway"), json!(verb), json!(item.id().as_str())];
            if let Some(placeholder) = placeholder {
                argv.push(json!(placeholder));
            }
            suggestions.push(json!({
                "command": command,
                "argv": argv,
                "item_id": item.id().as_str(),
            }));
        }
        if actions.contains(&"session.complete") {
            suggestions
                .push(json!({"command": "session.complete", "argv": ["podway", "complete"]}));
        }
        if actions.contains(&"session.decide")
            && let ParsedNodeDefinition::Decision(definition) = self.definition
        {
            let allowed_options = self.allowed_option_ids();
            for option in definition
                .options()
                .iter()
                .filter(|option| allowed_options.contains(&json!(option.id().as_str())))
            {
                suggestions.push(json!({
                    "command": "session.decide",
                    "argv": ["podway", "decide", "--option", option.id().as_str(), "--reason", "<reason>"],
                }));
            }
        }
        suggestions.push(json!({
            "command": "session.retry",
            "argv": ["podway", "retry", "--reason", "<reason>"],
        }));
        if actions.contains(&"session.skip") {
            let reason_required = matches!(self.placement, GraphPlacementV2::Action(action) if action.skip().is_some_and(|skip| skip.reason_required()));
            suggestions.push(if reason_required {
                json!({"command": "session.skip", "argv": ["podway", "skip", "--reason", "<text>"]})
            } else {
                json!({"command": "session.skip", "argv": ["podway", "skip"]})
            });
        }
        if actions.contains(&"goal.define") {
            suggestions.push(json!({
                "command": "goal.define",
                "argv": ["podway", "goal", "define", "--goal", "<goal>", "--criterion", "<criterion>"],
            }));
        }
        if actions.contains(&"goal.assess_criterion")
            && let Some(criteria) = unassessed_goal_criteria(state, self.attempt)
        {
            for criterion in criteria {
                suggestions.push(json!({
                    "command": "goal.assess_criterion",
                    "argv": ["podway", "goal", "assess-criterion", criterion, "--status", "<status>", "--reason", "<reason>"],
                }));
            }
        }
        suggestions.truncate(MAX_SUGGESTION_WINDOW_V2);
        suggestions
    }
}

fn condition_status_value_v2(status: &ConditionStatusV2) -> Value {
    json!({
        "state": status.state().as_str(),
        "predicates": status.predicates().iter().map(|status| {
            let predicate = status.predicate();
            let mut source = Map::new();
            source.insert("kind".to_owned(), json!("item"));
            source.insert("item_id".to_owned(), json!(predicate.item().as_str()));
            if predicate.field_outcome() {
                source.insert("field".to_owned(), json!("outcome"));
            }
            let mut value = Map::new();
            value.insert("source".to_owned(), Value::Object(source));
            value.insert("operator".to_owned(), json!(predicate.operator().as_str()));
            match predicate.operator() {
                ItemPredicateOperatorV2::Equals(expected)
                | ItemPredicateOperatorV2::NotEquals(expected) => {
                    value.insert("expected".to_owned(), predicate_scalar_value_v2(expected));
                }
                ItemPredicateOperatorV2::AtLeast(expected)
                | ItemPredicateOperatorV2::AtMost(expected) => {
                    value.insert("expected".to_owned(), json!(expected));
                }
                ItemPredicateOperatorV2::Empty | ItemPredicateOperatorV2::NonEmpty => {}
            }
            match status.actual() {
                PredicateActualV2::Scalar(actual) => {
                    value.insert("actual".to_owned(), predicate_scalar_value_v2(actual));
                }
                PredicateActualV2::Count(actual) => {
                    value.insert("actual_count".to_owned(), json!(actual));
                }
                PredicateActualV2::Unavailable => {}
            }
            value.insert("state".to_owned(), json!(status.state().as_str()));
            Value::Object(value)
        }).collect::<Vec<_>>(),
    })
}

fn guards_authoring_value_v2(guards: &podway_core::OptionGuardV2) -> Value {
    Value::Array(
        guards
            .predicates()
            .iter()
            .map(|predicate| {
                let mut value = Map::new();
                value.insert(
                    "evidence".to_owned(),
                    json!({
                        "node": predicate.source_node().as_str(),
                        "item": predicate.item().as_str(),
                    }),
                );
                if predicate.field_outcome() {
                    value.insert("field".to_owned(), json!("outcome"));
                }
                insert_predicate_operator_v2(&mut value, predicate.operator());
                Value::Object(value)
            })
            .collect(),
    )
}

fn evidence_predicate_status_value_v2(
    predicate: &podway_core::EvidencePredicateV2,
    status: &podway_core::PredicateStatusV2,
) -> Value {
    let mut source = Map::new();
    source.insert("kind".to_owned(), json!("evidence"));
    source.insert(
        "graph_node_id".to_owned(),
        json!(predicate.source_node().as_str()),
    );
    source.insert("item_id".to_owned(), json!(predicate.item().as_str()));
    if predicate.field_outcome() {
        source.insert("field".to_owned(), json!("outcome"));
    }
    let mut value = Map::new();
    value.insert("source".to_owned(), Value::Object(source));
    value.insert("operator".to_owned(), json!(predicate.operator().as_str()));
    match predicate.operator() {
        ItemPredicateOperatorV2::Equals(expected)
        | ItemPredicateOperatorV2::NotEquals(expected) => {
            value.insert("expected".to_owned(), predicate_scalar_value_v2(expected));
        }
        ItemPredicateOperatorV2::AtLeast(expected) | ItemPredicateOperatorV2::AtMost(expected) => {
            value.insert("expected".to_owned(), json!(expected));
        }
        ItemPredicateOperatorV2::Empty | ItemPredicateOperatorV2::NonEmpty => {}
    }
    match status.actual() {
        PredicateActualV2::Scalar(actual) => {
            value.insert("actual".to_owned(), predicate_scalar_value_v2(actual));
        }
        PredicateActualV2::Count(actual) => {
            value.insert("actual_count".to_owned(), json!(actual));
        }
        PredicateActualV2::Unavailable => {}
    }
    value.insert("state".to_owned(), json!(status.state().as_str()));
    Value::Object(value)
}

fn insert_predicate_operator_v2(
    value: &mut Map<String, Value>,
    operator: &ItemPredicateOperatorV2,
) {
    match operator {
        ItemPredicateOperatorV2::Equals(expected) => {
            value.insert("equals".to_owned(), predicate_scalar_value_v2(expected));
        }
        ItemPredicateOperatorV2::NotEquals(expected) => {
            value.insert("not_equals".to_owned(), predicate_scalar_value_v2(expected));
        }
        ItemPredicateOperatorV2::Empty => {
            value.insert("empty".to_owned(), json!(true));
        }
        ItemPredicateOperatorV2::NonEmpty => {
            value.insert("non_empty".to_owned(), json!(true));
        }
        ItemPredicateOperatorV2::AtLeast(expected) => {
            value.insert("at_least".to_owned(), json!(expected));
        }
        ItemPredicateOperatorV2::AtMost(expected) => {
            value.insert("at_most".to_owned(), json!(expected));
        }
    }
}

fn predicate_scalar_value_v2(value: &PredicateScalarV2) -> Value {
    match value {
        PredicateScalarV2::Boolean(value) => json!(value),
        PredicateScalarV2::Integer(value) => json!(value),
        PredicateScalarV2::Text(value) => json!(value),
    }
}

fn append_flag(argv: &mut Vec<Value>, flag: &str, value: &str) {
    argv.push(json!(flag));
    argv.push(json!(value));
}

fn push_recipe_unique(recipes: &mut Vec<Value>, recipe: Value) {
    if !recipes.contains(&recipe) {
        recipes.push(recipe);
    }
}

/// The bounded constraint projection carried by one observation active item.
///
/// ADR-0024 caps the exposed choices at eight while keeping the exact count and a truncation flag,
/// so a wide choice set never inflates an observation. The complete declaration stays available
/// from the Procedure inspection routes.
fn item_constraints(spec: &ItemSpecV2) -> Value {
    let mut constraints = Map::new();
    let mut choices_total = 0usize;
    match spec {
        ItemSpecV2::Confirm(_) => {}
        ItemSpecV2::Text(spec) => {
            constraints.insert("min_length".to_owned(), json!(spec.min_length()));
            constraints.insert("max_length".to_owned(), json!(spec.max_length()));
            constraints.insert("multiline".to_owned(), json!(spec.multiline()));
        }
        ItemSpecV2::Choice(spec) => {
            choices_total = spec.choices().len();
            if choices_total > 0 {
                constraints.insert(
                    "choices".to_owned(),
                    json!(
                        spec.choices()
                            .iter()
                            .take(MAX_OBSERVATION_CHOICE_WINDOW_V2)
                            .collect::<Vec<_>>()
                    ),
                );
            }
        }
        ItemSpecV2::Integer(spec) => {
            if let Some(minimum) = spec.minimum() {
                constraints.insert("minimum".to_owned(), json!(minimum));
            }
            if let Some(maximum) = spec.maximum() {
                constraints.insert("maximum".to_owned(), json!(maximum));
            }
        }
        ItemSpecV2::List(spec) => {
            constraints.insert("min_items".to_owned(), json!(spec.min_items()));
            constraints.insert("max_items".to_owned(), json!(spec.max_items()));
            constraints.insert("max_item_length".to_owned(), json!(spec.max_item_length()));
            constraints.insert(
                "max_total_length".to_owned(),
                json!(spec.max_total_length()),
            );
            constraints.insert("unique".to_owned(), json!(spec.unique()));
        }
        ItemSpecV2::Artifact(spec) => {
            constraints.insert(
                "allowed_media_types".to_owned(),
                json!(spec.allowed_media_types()),
            );
        }
        ItemSpecV2::CheckResult(spec) => {
            constraints.insert(
                "operation_id".to_owned(),
                json!(spec.operation_id().as_str()),
            );
            constraints.insert(
                "operation_digest".to_owned(),
                json!(spec.operation_digest().as_str()),
            );
            constraints.insert(
                "accepted_outcomes".to_owned(),
                json!(
                    spec.accepted_outcomes()
                        .iter()
                        .map(|outcome| outcome.as_str())
                        .collect::<Vec<_>>()
                ),
            );
        }
    }
    constraints.insert("choices_total".to_owned(), json!(choices_total));
    constraints.insert(
        "choices_truncated".to_owned(),
        json!(choices_total > MAX_OBSERVATION_CHOICE_WINDOW_V2),
    );
    Value::Object(constraints)
}

fn project_observation_item_value(value: &RecordedItemValueV2) -> (Value, bool) {
    if value.item_type() == ItemTypeV1::Confirm {
        return (Value::Bool(true), false);
    }
    if let Some(value) = value.as_text() {
        let (value, truncated) = truncate_chars(value, OBSERVATION_ITEM_VALUE_CHARS_MAX);
        return (json!(value), truncated);
    }
    if let Some(value) = value.as_choice() {
        return (json!(value), false);
    }
    if let Some(value) = value.as_integer() {
        return (json!(value), false);
    }
    if let Some(value) = value.as_list() {
        let mut projected: Vec<String> = Vec::new();
        let mut truncated = false;
        // Each candidate's JSON bytes are measured once and the admitted prefix is charged
        // incrementally: the array brackets cost two bytes, every further entry one
        // separator byte, so no admitted entry is ever serialized again.
        let mut admitted_bytes = 0usize;
        for entry in value {
            let (entry, entry_truncated) = truncate_chars(entry, OBSERVATION_ITEM_VALUE_CHARS_MAX);
            let entry_bytes = match serialized_json_len_v1(&entry) {
                Ok(bytes) => bytes,
                // A measurement failure keeps the former whole-prefix fail-closed shape:
                // the candidate is refused and the window is cut.
                Err(_) => {
                    truncated = true;
                    break;
                }
            };
            let over_budget = 2usize
                .checked_add(admitted_bytes)
                .and_then(|bytes| bytes.checked_add(entry_bytes))
                .and_then(|bytes| bytes.checked_add(projected.len()))
                .is_none_or(|total| total > OBSERVATION_ITEM_VALUE_BYTES_MAX);
            if over_budget {
                truncated = true;
                break;
            }
            admitted_bytes += entry_bytes;
            projected.push(entry);
            truncated |= entry_truncated;
        }
        truncated |= projected.len() < value.len();
        return (Value::from(projected), truncated);
    }
    if value.item_type() == ItemTypeV1::CheckResult {
        return (check_result_preview_v1(value), false);
    }
    let artifact = value
        .as_artifact()
        .expect("the item type guarantees an artifact value");
    let (location, location_truncated) =
        truncate_chars(artifact.location(), OBSERVATION_ITEM_VALUE_CHARS_MAX);
    (
        json!({
            "location_type": match artifact.location_kind() {
                ArtifactLocationKindV1::LocalPath => "path",
                ArtifactLocationKindV1::ExternalReference => "reference",
            },
            "location": location,
            "sha256_digest": artifact.digest().as_str(),
            "size_bytes": artifact.size_bytes(),
            "media_type": artifact.media_type(),
        }),
        location_truncated,
    )
}

fn procedure_identity(state: &GraphSessionStateV2) -> Value {
    json!({
        "schema": "podway.procedure/v2",
        "id": state.snapshot().procedure_id(),
        "version": state.snapshot().procedure_version(),
        "digest": state.snapshot().digest().as_str(),
    })
}

fn session_identity(state: &GraphSessionStateV2) -> Value {
    json!({
        "id": state.trace().session_id().as_str(),
        "lifecycle": lifecycle(state.trace().lifecycle()),
        "revision": state.trace().revision().get(),
    })
}

fn counters(state: &GraphSessionStateV2) -> Value {
    if state.trace().lifecycle() == SessionLifecycle::Prepared {
        return Value::Array(Vec::new());
    }
    encode_graph_counters_v2(state.counters().iter().map(|counter| {
        (
            counter.graph_node_id().as_str(),
            counter.attempt_count(),
            counter.rework_traversal_count(),
        )
    }))
}

fn encode_graph_counters_v2<'a>(counters: impl IntoIterator<Item = (&'a str, u64, u64)>) -> Value {
    Value::Array(
        counters
            .into_iter()
            .map(|(graph_node_id, attempt_count, rework_traversal_count)| {
                json!({
                    "graph_node_id": graph_node_id,
                    "attempt_count": attempt_count,
                    "rework_traversal_count": rework_traversal_count,
                })
            })
            .collect(),
    )
}

fn add_static_disposition(
    result: &mut Map<String, Value>,
    current: Option<&CurrentProjection<'_>>,
) {
    let Some(current) = current else {
        return;
    };
    if let GraphPlacementV2::Action(action) = current.placement {
        if let Some(target) = action.outcome().next_target() {
            result.insert("next_graph_node_id".to_owned(), json!(target.as_str()));
        } else {
            result.insert("terminal".to_owned(), Value::Bool(true));
        }
        if let Some(skip) = action.skip() {
            result.insert(
                "skip".to_owned(),
                json!({"allowed": true, "reason_required": skip.reason_required()}),
            );
        }
    }
}

fn manual_rework_targets(state: &GraphSessionStateV2, procedure: &ParsedProcedureV2) -> Value {
    let valid_nodes = state
        .trace()
        .attempts()
        .iter()
        .filter(|attempt| attempt.validity() == AttemptValidityV2::Valid)
        .map(|attempt| attempt.graph_node_id())
        .collect::<Vec<_>>();
    Value::Array(
        procedure
            .graph()
            .manual_rework()
            .map(|manual| {
                manual
                    .targets()
                    .iter()
                    .filter(|target| valid_nodes.contains(target))
                    .map(|target| json!(target.as_str()))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn goal_revision_rework_targets(
    state: &GraphSessionStateV2,
    procedure: &ParsedProcedureV2,
) -> Value {
    let safe = goal_revision_safe_targets_v2(procedure);
    let valid_nodes = state
        .trace()
        .attempts()
        .iter()
        .filter(|attempt| attempt.validity() == AttemptValidityV2::Valid)
        .map(|attempt| attempt.graph_node_id())
        .collect::<Vec<_>>();
    Value::Array(
        procedure
            .graph()
            .manual_rework()
            .map(|manual| {
                manual
                    .targets()
                    .iter()
                    .filter(|target| valid_nodes.contains(target) && safe.contains(target))
                    .map(|target| json!(target.as_str()))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn add_goal_summary(result: &mut Map<String, Value>, state: &GraphSessionStateV2) {
    if let Some(revision) = state.goal_state().current_revision() {
        result.insert("goal_revision".to_owned(), json!(revision.get()));
        if let Some(assessment) = state.goal_state().latest_fresh_assessment(state.trace()) {
            result.insert(
                "latest_goal_outcome".to_owned(),
                json!(assessment.outcome().as_str()),
            );
        }
    }
}

fn goal_display(
    state: &GraphSessionStateV2,
    current: Option<&CurrentProjection<'_>>,
) -> Option<Value> {
    let revision_number = state.goal_state().current_revision()?;
    let revision = state
        .goal_state()
        .revisions()
        .iter()
        .find(|record| record.revision() == revision_number)?;
    let attempt_results = current.and_then(|current| {
        state
            .goal_state()
            .attempt_assessments()
            .iter()
            .find(|assessment| {
                assessment.attempt_id() == current.attempt.attempt_id()
                    && assessment.goal_revision() == revision_number
            })
    });
    let latest_assessment = state
        .goal_state()
        .latest_fresh_assessment(state.trace())
        .filter(|assessment| assessment.goal_revision() == revision_number);
    let current_is_goal_assessment = current.is_some_and(|current| {
        matches!(
            current.definition,
            ParsedNodeDefinition::Decision(definition) if definition.assessment().is_some()
        )
    });
    let displayed_historical_assessment = (!current_is_goal_assessment)
        .then_some(latest_assessment)
        .flatten();
    let criteria = revision
        .criteria()
        .criteria()
        .iter()
        .map(|criterion| {
            let status = attempt_results
                .and_then(|assessment| {
                    assessment
                        .results()
                        .iter()
                        .find(|result| result.result().criterion_id() == criterion.id())
                })
                .map(|result| result.result().status().as_str())
                .or_else(|| {
                    displayed_historical_assessment.and_then(|assessment| {
                        assessment
                            .criterion_results()
                            .iter()
                            .find(|result| result.criterion_id() == criterion.id())
                            .map(|result| result.status().as_str())
                    })
                })
                .unwrap_or("unassessed");
            json!({
                "criterion_id": criterion.id().as_str(),
                "statement": criterion.statement(),
                "status": status,
            })
        })
        .collect::<Vec<_>>();
    let mut goal = Map::new();
    goal.insert("revision".to_owned(), json!(revision_number.get()));
    goal.insert("statement".to_owned(), json!(revision.statement().as_str()));
    goal.insert("criteria".to_owned(), Value::Array(criteria));
    if let Some(results) = attempt_results
        && let Some(first) = results.results().first()
    {
        goal.insert(
            "assessment_mode".to_owned(),
            json!(first.result().mode().as_str()),
        );
    } else if let Some(assessment) = displayed_historical_assessment {
        goal.insert(
            "assessment_mode".to_owned(),
            json!(assessment.mode().as_str()),
        );
    }
    let determined = current
        .and_then(|current| determined_goal_outcome(state, current.attempt))
        .or_else(|| {
            (!current_is_goal_assessment).then_some(())?;
            state
                .goal_state()
                .latest_fresh_assessment(state.trace())
                .map(|assessment| assessment.outcome())
        });
    if let Some(outcome) = determined {
        goal.insert("determined_outcome".to_owned(), json!(outcome.as_str()));
    }
    Some(Value::Object(goal))
}

fn determined_goal_outcome_for_current(current: &CurrentProjection<'_>) -> Option<GoalOutcome> {
    determined_goal_outcome(current.state, current.attempt)
}

fn determined_goal_outcome(
    state: &GraphSessionStateV2,
    attempt: &SessionAttemptV2,
) -> Option<GoalOutcome> {
    let revision = state.goal_state().current_revision()?;
    if attempt.goal_revision() != Some(revision) {
        return None;
    }
    let goal = state
        .goal_state()
        .revisions()
        .iter()
        .find(|record| record.revision() == revision)?;
    let assessment = state
        .goal_state()
        .attempt_assessments()
        .iter()
        .find(|assessment| {
            assessment.attempt_id() == attempt.attempt_id()
                && assessment.goal_revision() == revision
        })?;
    if assessment.results().len() != goal.criteria().criteria().len()
        || goal.criteria().criteria().iter().any(|criterion| {
            !assessment
                .results()
                .iter()
                .any(|state| state.result().criterion_id() == criterion.id())
        })
    {
        return None;
    }
    if assessment
        .results()
        .iter()
        .all(|state| state.result().status() == CriterionStatusV2::NotApplicable)
    {
        Some(GoalOutcome::Superseded)
    } else if assessment
        .results()
        .iter()
        .all(|state| state.result().status() == CriterionStatusV2::Satisfied)
    {
        Some(GoalOutcome::Achieved)
    } else if assessment
        .results()
        .iter()
        .any(|state| state.result().status() == CriterionStatusV2::Unsatisfied)
    {
        Some(GoalOutcome::NotAchieved)
    } else {
        None
    }
}

fn unassessed_goal_criteria<'a>(
    state: &'a GraphSessionStateV2,
    attempt: &SessionAttemptV2,
) -> Option<Vec<&'a str>> {
    let revision = state.goal_state().current_revision()?;
    if attempt.goal_revision() != Some(revision) {
        return None;
    }
    let goal = state
        .goal_state()
        .revisions()
        .iter()
        .find(|record| record.revision() == revision)?;
    let results = state
        .goal_state()
        .attempt_assessments()
        .iter()
        .find(|assessment| {
            assessment.attempt_id() == attempt.attempt_id()
                && assessment.goal_revision() == revision
        });
    Some(
        goal.criteria()
            .criteria()
            .iter()
            .filter(|criterion| {
                results.is_none_or(|assessment| {
                    !assessment
                        .results()
                        .iter()
                        .any(|state| state.result().criterion_id() == criterion.id())
                })
            })
            .map(|criterion| criterion.id().as_str())
            .collect(),
    )
}

fn reference_metadata(
    reference: &EvidenceResolutionStateV2,
    procedure: &ParsedProcedureV2,
    stale: bool,
) -> Result<Value, GraphViewErrorV2> {
    let resolution = reference.resolution();
    let source = resolution.source_node();
    let mut value = Map::new();
    value.insert("source_graph_node_id".to_owned(), json!(source.as_str()));
    if let Some(title) = definition_title_for_node(procedure, source.as_str()) {
        value.insert("source_title".to_owned(), json!(title));
    }
    let state = if stale {
        "stale"
    } else {
        match resolution {
            ResolvedEvidenceReferenceV2::Resolved(_) => "resolved",
            ResolvedEvidenceReferenceV2::Skipped(_) => "skipped",
            ResolvedEvidenceReferenceV2::Unresolved { .. } => "unresolved",
        }
    };
    value.insert("state".to_owned(), json!(state));
    if let Some(snapshot) = resolution.snapshot() {
        value.insert(
            "source_attempt_id".to_owned(),
            json!(snapshot.source_attempt_id().as_str()),
        );
        value.insert(
            "source_attempt_number".to_owned(),
            json!(snapshot.source_attempt_number().get()),
        );
        value.insert(
            "items_digest".to_owned(),
            json!(snapshot.items_digest().as_str()),
        );
    } else if stale {
        return Err(GraphViewErrorV2::InconsistentState(
            "a stale reference must retain resolved snapshot metadata",
        ));
    }
    Ok(Value::Object(value))
}

/// The evidence surface of `next-result/v3`: identity, digest, and size for every selected item,
/// with a bounded preview only where the Procedure named specific items.
///
/// ADR-0024 moved complete values out of progression guidance. A caller that needs content pages it
/// through `evidence.read` using the continuation token a preview carries.
fn readback(
    state: &GraphSessionStateV2,
    current: &CurrentProjection<'_>,
    procedure: &ParsedProcedureV2,
    display: EvidencePreviewDisplay,
) -> Result<(Vec<Value>, u64), GraphViewErrorV2> {
    // `CurrentProjection::derive` already selected this attempt's evidence and fails closed when
    // that selection is invalid, so guidance reuses the projection's admitted selection instead
    // of repeating it; the former recompute's error mapping was unreachable after derive.
    let mut previews_remaining = MAX_EVIDENCE_PREVIEW_SLOTS_V2;
    let mut items_total = 0u64;
    let mut values = Vec::with_capacity(current.evidence_readback.len());
    for entry in &current.evidence_readback {
        let (value, count) =
            readback_value(state, entry, procedure, &mut previews_remaining, display)?;
        items_total += count;
        values.push(value);
    }
    Ok((values, items_total))
}

/// One evidence reference projected as metadata, with bounded previews for explicitly selected
/// items while preview slots remain.
///
/// Returns the projection and the number of item metadata records it contributed, so the caller can
/// report the exact response-wide total. `display` only decides whether an admitted preview's
/// payload is materialized: admission, slot consumption, and the truncation-bound snapshot,
/// consumer, and token-constructibility checks run identically in both modes, at this same stage,
/// for exactly these items.
fn readback_value(
    state: &GraphSessionStateV2,
    readback: &EvidenceReadbackV2,
    procedure: &ParsedProcedureV2,
    previews_remaining: &mut usize,
    display: EvidencePreviewDisplay,
) -> Result<(Value, u64), GraphViewErrorV2> {
    let resolution = readback.reference().resolution();
    let source = resolution.source_node();
    let source_title = definition_title_for_node(procedure, source.as_str())
        .ok_or(GraphViewErrorV2::InvalidSnapshot)?;
    // A stale read-back must never be published as resolved. `next-result/v3` deliberately admits
    // only the three live states, because progression guidance projects the current attempt and a
    // current attempt is valid; a stale entry reaching here means the trace disagrees with itself,
    // so this fails closed rather than mislabelling evidence the caller would then trust.
    // Reachable only if that invariant is ever relaxed, which is why no test drives this arm;
    // `v2scl004_evidence_read_separates_a_stale_reference_from_a_stale_token` proves the state
    // cannot be constructed today.
    if readback.stale() {
        return Err(GraphViewErrorV2::InconsistentState(
            "progression guidance cannot read back stale evidence",
        ));
    }
    let reference_state = match resolution {
        ResolvedEvidenceReferenceV2::Resolved(_) => "resolved",
        ResolvedEvidenceReferenceV2::Skipped(_) => "skipped",
        ResolvedEvidenceReferenceV2::Unresolved { .. } => "unresolved",
    };
    let mut value = Map::new();
    value.insert("source_graph_node_id".to_owned(), json!(source.as_str()));
    value.insert("source_title".to_owned(), json!(source_title));
    value.insert("state".to_owned(), json!(reference_state));

    // An omitted selector means metadata-only: the Procedure did not name which items matter, so
    // guidance does not guess by previewing all of them.
    let explicit_selector = !readback.reference().selected_item_ids().is_empty();
    let source_slots = resolution.snapshot().and_then(|snapshot| {
        state
            .workflow_memory()
            .attempts()
            .iter()
            .find(|memory| memory.attempt_id() == snapshot.source_attempt_id())
    });

    let mut items = Vec::with_capacity(readback.items().items().len());
    for item in readback.items().items() {
        let revision = source_slots
            .and_then(|memory| {
                memory
                    .item_slots()
                    .iter()
                    .find(|slot| slot.item_id() == item.id())
            })
            .map(|slot| slot.revision())
            .ok_or(GraphViewErrorV2::InconsistentState(
                "an evidence item has no source slot",
            ))?;
        let digest = recorded_value_digest_v2(item.value()).map_err(|_| {
            GraphViewErrorV2::InconsistentState("an evidence value is not digestible")
        })?;
        let (total_size, size_unit) = evidence_logical_size_v1(item.value());
        let mut metadata = Map::new();
        metadata.insert("item_id".to_owned(), json!(item.id().as_str()));
        metadata.insert(
            "type".to_owned(),
            json!(item_type(item.value().item_type())),
        );
        metadata.insert("revision".to_owned(), json!(revision.get()));
        metadata.insert("value_digest".to_owned(), json!(digest.as_str()));
        metadata.insert("total_size".to_owned(), json!(total_size));
        metadata.insert("size_unit".to_owned(), json!(size_unit));

        if explicit_selector
            && *previews_remaining > 0
            && let Some(plan) = evidence_preview_plan_v1(item.value(), total_size)
        {
            *previews_remaining -= 1;
            if display == EvidencePreviewDisplay::Publish {
                metadata.insert(
                    "preview".to_owned(),
                    evidence_preview_payload_v1(item.value(), &plan),
                );
                metadata.insert("preview_truncated".to_owned(), json!(plan.truncated));
            }
            if plan.truncated {
                let snapshot = resolution
                    .snapshot()
                    .ok_or(GraphViewErrorV2::InconsistentState(
                        "a resolved reference must carry its snapshot",
                    ))?;
                // The consumer attempt binds the token, not the source node: a continuation is
                // only valid for the attempt that may currently read this evidence. The token is
                // still constructed under suppression so an unconstructible one fails here for
                // exactly the same items; only its encoding is bound to publication.
                let consumer = state
                    .trace()
                    .active_attempt()
                    .ok_or(GraphViewErrorV2::TerminalSessionHasNoNext)?;
                let token = EvidencePageTokenV1::new(
                    state.trace().session_id().as_str(),
                    consumer.attempt_id().as_str(),
                    snapshot.source_attempt_id().as_str(),
                    item.id().as_str(),
                    digest.as_str(),
                    plan.size,
                )
                .map_err(|_| {
                    GraphViewErrorV2::InconsistentState("a page token is not constructible")
                })?;
                if display == EvidencePreviewDisplay::Publish {
                    metadata.insert("next_page_token".to_owned(), json!(token.encode()));
                }
            }
        }
        items.push(Value::Object(metadata));
    }

    let items_total = items.len() as u64;
    value.insert("items".to_owned(), Value::Array(items));
    value.insert("items_total".to_owned(), json!(items_total));
    value.insert("items_truncated".to_owned(), json!(false));

    if let Some(snapshot) = resolution.snapshot() {
        value.insert(
            "source_attempt_id".to_owned(),
            json!(snapshot.source_attempt_id().as_str()),
        );
        value.insert(
            "source_attempt_number".to_owned(),
            json!(snapshot.source_attempt_number().get()),
        );
        value.insert(
            "items_digest".to_owned(),
            json!(snapshot.items_digest().as_str()),
        );
    }
    if let Some(decision) = readback.decision() {
        value.insert(
            "decision_record".to_owned(),
            decision_readback_projection(state, decision)?,
        );
    }
    Ok((Value::Object(value), items_total))
}

/// How `readback_value` treats a preview its gates admitted.
///
/// `Publish` is `session.next`: the admitted preview's payload is materialized. `Suppress` is
/// observation guidance: the same admission consumes the same slot and runs the same
/// truncation-bound checks, but the payload text ADR-0024 removes is never constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvidencePreviewDisplay {
    Publish,
    Suppress,
}

/// The admission decision for one preview: how much it would carry and whether it truncates.
///
/// This is the single owner of preview presence, size, and truncation. Publishing materializes
/// exactly this plan; suppression evaluates it without building the payload it would drop.
struct EvidencePreviewPlanV1 {
    size: u64,
    truncated: bool,
}

/// Decides whether one recorded value earns a preview slot, and what that preview would carry.
///
/// The preview is deliberately small: it exists so a caller can recognise a value, not consume it.
/// `total` is the caller's logical size of the same admitted value, so the text arm decides
/// without touching the value's scalars. A preview that would carry nothing — an empty value or a
/// list whose first entry alone exceeds the slot — is no preview at all and spends no slot.
fn evidence_preview_plan_v1(
    value: &RecordedItemValueV2,
    total: u64,
) -> Option<EvidencePreviewPlanV1> {
    if value.as_text().is_some() {
        if total == 0 {
            return None;
        }
        let end = total.min(MAX_EVIDENCE_PREVIEW_SCALARS_V2 as u64);
        return Some(EvidencePreviewPlanV1 {
            size: end,
            truncated: end < total,
        });
    }
    if let Some(entries) = value.as_list() {
        // A list preview carries whole entries only, and charges the bytes each one encodes to
        // rather than its scalars alone: many one-scalar entries cost mostly structure. Every
        // entry is budget-tested, the first included — unlike a page, a preview never admits an
        // entry that alone overflows its slot — so admitting nothing is a real outcome, and an
        // empty preview is worth no slot, so it is published as no preview at all.
        let mut charged = 0usize;
        let mut end = 0usize;
        while end < entries.len() {
            let cost = EVIDENCE_ENTRY_OVERHEAD_BYTES
                + entries[end].chars().count() * EVIDENCE_SCALAR_BYTES;
            if charged + cost > MAX_EVIDENCE_PREVIEW_BYTES_V2 {
                break;
            }
            charged += cost;
            end += 1;
        }
        if end == 0 {
            return None;
        }
        return Some(EvidencePreviewPlanV1 {
            size: end as u64,
            truncated: end < entries.len(),
        });
    }
    if value.item_type() == ItemTypeV1::CheckResult {
        // The logical size of a check result is its encoded byte count, already measured once
        // for this value; the preview reports that same total as its size.
        return Some(EvidencePreviewPlanV1 {
            size: total,
            truncated: false,
        });
    }
    // Confirm, choice, integer, and artifact values are indivisible, so a preview of one is either
    // complete or absent. An artifact projection can exceed a preview slot on its own, and its
    // metadata already describes it, so it is left to `evidence.read` rather than truncated.
    if indivisible_preview_charge_v1(value) > MAX_EVIDENCE_PREVIEW_BYTES_V2 {
        return None;
    }
    Some(EvidencePreviewPlanV1 {
        size: 1,
        truncated: false,
    })
}

/// Materializes the payload of an already admitted preview plan.
///
/// This performs no admission arithmetic: it renders exactly the scalars, entries, or typed
/// projection the plan admitted, so publishing and suppressing cannot disagree about presence,
/// size, or truncation.
fn evidence_preview_payload_v1(value: &RecordedItemValueV2, plan: &EvidencePreviewPlanV1) -> Value {
    if let Some(text) = value.as_text() {
        return Value::String(text.chars().take(plan.size as usize).collect());
    }
    if let Some(entries) = value.as_list() {
        return Value::Array(
            entries[..plan.size as usize]
                .iter()
                .map(|entry| json!(entry))
                .collect(),
        );
    }
    if value.item_type() == ItemTypeV1::CheckResult {
        return check_result_preview_v1(value);
    }
    typed_item_value(value)
}

/// The encoded bytes an indivisible projected value would contribute to a preview, charged from
/// the typed value without materializing its JSON projection.
///
/// Keys, quoting, and the widest decimal a number can render to are all charged: a preview slot
/// bounds what the response actually serializes, not what its content alone would measure. This
/// is the same charge the retained test-only recursive charger applies to `typed_item_value`,
/// which the unit oracles keep equal to this typed charge.
fn indivisible_preview_charge_v1(value: &RecordedItemValueV2) -> usize {
    /// The bytes the widest `i64` renders to, sign included.
    const NUMBER_BYTES: usize = 20;
    /// Quotes, a separator, and slack for one member or element.
    const MEMBER_OVERHEAD_BYTES: usize = EVIDENCE_ENTRY_OVERHEAD_BYTES;
    fn charged_scalars(text: &str) -> usize {
        text.chars().count() * EVIDENCE_SCALAR_BYTES
    }
    match value.item_type() {
        ItemTypeV1::Confirm | ItemTypeV1::Integer => NUMBER_BYTES,
        ItemTypeV1::Choice => charged_scalars(value.as_choice().unwrap_or_default()),
        ItemTypeV1::Artifact => {
            let artifact = value
                .as_artifact()
                .expect("artifact type has an artifact value");
            let location_type = match artifact.location_kind() {
                podway_core::ArtifactLocationKindV1::LocalPath => "path",
                podway_core::ArtifactLocationKindV1::ExternalReference => "reference",
            };
            MEMBER_OVERHEAD_BYTES * 5
                + charged_scalars("location_type")
                + charged_scalars(location_type)
                + charged_scalars("location")
                + charged_scalars(artifact.location())
                + charged_scalars("sha256_digest")
                + charged_scalars(artifact.digest().as_str())
                + charged_scalars("size_bytes")
                + NUMBER_BYTES
                + charged_scalars("media_type")
                + charged_scalars(artifact.media_type())
        }
        // Text, list, and check-result values admit previews through their own arms above.
        ItemTypeV1::Text | ItemTypeV1::List | ItemTypeV1::CheckResult => {
            unreachable!("divisible values never charge as indivisible")
        }
    }
}

/// The retained pre-VIEWS-004 construction of one bounded preview, test oracle only.
///
/// Publishing must equal this legacy output for every value; the unit oracles compare the
/// plan-and-payload path against it so the new admission owner cannot drift from the published
/// behavior this file shipped before preview suppression existed.
#[cfg(test)]
fn legacy_evidence_preview_v1(value: &RecordedItemValueV2, total: u64) -> Option<EvidencePageV1> {
    if let Some(text) = value.as_text() {
        if total == 0 {
            return None;
        }
        let end = total.min(MAX_EVIDENCE_PREVIEW_SCALARS_V2 as u64);
        let preview: String = text.chars().take(end as usize).collect();
        return Some(EvidencePageV1 {
            size: end,
            data: Value::String(preview),
            truncated: end < total,
        });
    }
    if let Some(entries) = value.as_list() {
        let mut charged = 0usize;
        let mut end = 0usize;
        while end < entries.len() {
            let cost = EVIDENCE_ENTRY_OVERHEAD_BYTES
                + entries[end].chars().count() * EVIDENCE_SCALAR_BYTES;
            if charged + cost > MAX_EVIDENCE_PREVIEW_BYTES_V2 {
                break;
            }
            charged += cost;
            end += 1;
        }
        if end == 0 {
            return None;
        }
        return Some(EvidencePageV1 {
            size: end as u64,
            data: Value::Array(entries[..end].iter().map(|entry| json!(entry)).collect()),
            truncated: end < entries.len(),
        });
    }
    if value.item_type() == ItemTypeV1::CheckResult {
        return Some(EvidencePageV1 {
            size: total,
            data: check_result_preview_v1(value),
            truncated: false,
        });
    }
    let projected = typed_item_value(value);
    if legacy_projected_preview_bytes_v1(&projected) > MAX_EVIDENCE_PREVIEW_BYTES_V2 {
        return None;
    }
    Some(EvidencePageV1 {
        size: 1,
        data: projected,
        truncated: false,
    })
}

/// The retained pre-VIEWS-004 recursive charge over a materialized projection, test oracle only.
#[cfg(test)]
fn legacy_projected_preview_bytes_v1(value: &Value) -> usize {
    /// The bytes the widest `i64` renders to, sign included.
    const NUMBER_BYTES: usize = 20;
    /// Quotes, a separator, and slack for one member or element.
    const MEMBER_OVERHEAD_BYTES: usize = EVIDENCE_ENTRY_OVERHEAD_BYTES;
    match value {
        Value::String(text) => text.chars().count() * EVIDENCE_SCALAR_BYTES,
        Value::Object(fields) => fields
            .iter()
            .map(|(key, member)| {
                MEMBER_OVERHEAD_BYTES
                    + key.chars().count() * EVIDENCE_SCALAR_BYTES
                    + legacy_projected_preview_bytes_v1(member)
            })
            .sum::<usize>(),
        Value::Array(members) => members
            .iter()
            .map(|member| MEMBER_OVERHEAD_BYTES + legacy_projected_preview_bytes_v1(member))
            .sum::<usize>(),
        _ => NUMBER_BYTES,
    }
}

fn definition_title_for_node<'a>(
    procedure: &'a ParsedProcedureV2,
    node_id: &str,
) -> Option<&'a str> {
    let placement = procedure
        .graph()
        .placements()
        .iter()
        .find(|placement| placement.id().as_str() == node_id)?;
    let definition_id = match placement {
        GraphPlacementV2::Action(placement) => placement.definition(),
        GraphPlacementV2::Decision(placement) => placement.definition(),
    };
    procedure
        .node_definitions()
        .iter()
        .find_map(|definition| match definition {
            ParsedNodeDefinition::Action(value) if value.id() == definition_id => {
                Some(value.title())
            }
            ParsedNodeDefinition::Decision(value) if value.id() == definition_id => {
                Some(value.title())
            }
            _ => None,
        })
}

fn add_histories(
    result: &mut Map<String, Value>,
    state: &GraphSessionStateV2,
    procedure: &ParsedProcedureV2,
    cursor: Option<TraceSequenceV2>,
) -> Result<(), GraphViewErrorV2> {
    let trace = state.trace().attempts();
    let entries = trace
        .iter()
        .rev()
        .filter(|attempt| attempt.validity() == AttemptValidityV2::Valid)
        .filter(|attempt| before_cursor(attempt.trace(), cursor));
    result.insert(
        "current_trace_history".to_owned(),
        history_window(
            entries,
            |attempt| trace_entry(state, procedure, attempt),
            32,
        )?,
    );

    let entries = trace
        .iter()
        .rev()
        .filter(|attempt| attempt.validity() == AttemptValidityV2::Stale)
        .filter(|attempt| before_cursor(attempt.trace(), cursor));
    result.insert(
        "stale_attempt_history".to_owned(),
        history_window(
            entries,
            |attempt| stale_attempt_value(state, procedure, attempt),
            1,
        )?,
    );

    let entries = state
        .workflow_memory()
        .decisions()
        .iter()
        .rev()
        .filter(|record| before_cursor(record.trace(), cursor));
    result.insert(
        "decision_history".to_owned(),
        history_window(entries, decision_value, 1)?,
    );

    let entries = state
        .workflow_memory()
        .reworks()
        .iter()
        .rev()
        .filter(|record| before_cursor(record.trace(), cursor));
    result.insert(
        "rework_history".to_owned(),
        history_window(
            entries,
            |record| {
                let mut value = json!({
                    "trace_sequence": record.trace().get(),
                    "kind": record.kind().as_str(),
                    "from_graph_node_id": record.from_node().as_str(),
                    "to_graph_node_id": record.to_node().as_str(),
                    "target_attempt_id": record.target_attempt_id().as_str(),
                    "reason": record.reason().as_str(),
                    "reactivated": record.reactivated(),
                    "recorded_at": timestamp(record.recorded_at())?,
                });
                if let Some(actor) = record.actor()
                    && let Some(object) = value.as_object_mut()
                {
                    object.insert("actor".to_owned(), json!(actor.as_str()));
                }
                Ok(value)
            },
            6,
        )?,
    );

    let current_revision = state.goal_state().current_revision();
    let entries = state
        .goal_state()
        .revisions()
        .iter()
        .rev()
        .filter(|record| Some(record.revision()) != current_revision)
        .filter(|record| before_cursor(record.binding_trace(), cursor));
    result.insert(
        "stale_goal_revision_history".to_owned(),
        history_window(
            entries,
            |record| stale_goal_revision_value(state, record),
            1,
        )?,
    );

    let entries = state
        .goal_state()
        .assessments()
        .iter()
        .rev()
        .filter(|assessment| {
            current_revision != Some(assessment.goal_revision())
                || !state.trace().attempts().iter().any(|attempt| {
                    attempt.attempt_id() == assessment.decision_attempt_id()
                        && attempt.validity() == AttemptValidityV2::Valid
                })
        })
        .filter(|assessment| before_cursor(assessment.decision_trace(), cursor));
    result.insert(
        "stale_goal_assessment_history".to_owned(),
        history_window(
            entries,
            |assessment| stale_goal_assessment_value(state, assessment),
            1,
        )?,
    );
    Ok(())
}

fn before_cursor(sequence: TraceSequenceV2, cursor: Option<TraceSequenceV2>) -> bool {
    cursor.is_none_or(|cursor| sequence < cursor)
}

/// The min/max `trace_sequence` bounds of a history window, or `None` when no entry carries one.
fn trace_bounds(entries: &[Value]) -> Option<(u64, u64)> {
    entries.iter().fold(None, |bounds, entry| {
        fold_trace_bounds(bounds, entry.get("trace_sequence"))
    })
}

/// Folds one candidate's `trace_sequence` into the running min/max bounds.
///
/// Entries without a `trace_sequence` member, including an explicit `null`, never
/// contribute: the bounds are the extremes among participating entries, not assumed
/// first and last positions.
fn fold_trace_bounds(bounds: Option<(u64, u64)>, sequence: Option<&Value>) -> Option<(u64, u64)> {
    let Some(sequence) = sequence.and_then(Value::as_u64) else {
        return bounds;
    };
    Some(match bounds {
        None => (sequence, sequence),
        Some((first, last)) => (first.min(sequence), last.max(sequence)),
    })
}

/// A borrowed trace window whose serialized shape matches the owned projection's member.
#[derive(serde::Serialize)]
struct HistoryTraceWindowMeasure {
    first_sequence: u64,
    last_sequence: u64,
}

/// The fixed wrapper shape for history-window byte budgeting.
///
/// Field order matches the owned construction's serialized key order. The empty `entries`
/// slice keeps the real array's bracket pair charged, and the marker is fixed to `false`,
/// keeping the longer spelling budgeted exactly as before so the final marker cannot add a
/// byte when a candidate happens to consume the iterator exactly. Production budgeting
/// serializes this shape once per window as its fixed skeleton and measures each entry
/// separately; the test-only `history_window_measure_bytes` oracle serializes whole
/// windows through it.
#[derive(serde::Serialize)]
struct HistoryWindowMeasure<'a> {
    entries: &'a [Value],
    trace_truncated: bool,
    trace_window: Option<HistoryTraceWindowMeasure>,
}

/// Maps any serialization failure to the history view's existing inconsistent-state error.
fn history_measurement_error(_: serde_json::Error) -> GraphViewErrorV2 {
    GraphViewErrorV2::InconsistentState("history view is not JSON")
}

/// Fixed wrapper bytes for incremental history-window budgeting, plus the null window length.
///
/// The wrapper is measured once per window through the same serializer that produces the
/// final bytes: the `entries` bracket pair and the longer `false` marker stay charged, and
/// only the `null` trace-window placeholder is removed because every candidate replaces it
/// with its own measured window bytes.
fn history_window_wrapper_bytes() -> Result<(usize, usize), GraphViewErrorV2> {
    let wrapper = serialized_json_len_v1(&HistoryWindowMeasure {
        entries: &[],
        trace_truncated: false,
        trace_window: None,
    })
    .map_err(history_measurement_error)?;
    let null_window = serialized_json_len_v1(&Value::Null).map_err(history_measurement_error)?;
    let skeleton = wrapper
        .checked_sub(null_window)
        .ok_or(GraphViewErrorV2::InconsistentState(
            "history view is not JSON",
        ))?;
    Ok((skeleton, null_window))
}

fn history_window<T>(
    entries: impl Iterator<Item = T>,
    mut render: impl FnMut(T) -> Result<Value, GraphViewErrorV2>,
    maximum: usize,
) -> Result<Value, GraphViewErrorV2> {
    let mut entries = entries;
    let mut bounded: Vec<Value> = Vec::new();
    let mut admitted_bytes = 0usize;
    let mut admitted_bounds: Option<(u64, u64)> = None;
    let mut truncated = false;
    let (skeleton_bytes, null_window_bytes) = history_window_wrapper_bytes()?;
    while bounded.len() < maximum {
        let Some(item) = entries.next() else {
            break;
        };
        let entry = render(item)?;
        let entry_bytes = serialized_json_len_v1(&entry).map_err(history_measurement_error)?;
        // Trial bounds fold the candidate in before it is measured, so the trace-window
        // charge already reflects a shrinking minimum or widening maximum, including one
        // further digit such as 9999 -> 10000.
        let trial_bounds = fold_trace_bounds(admitted_bounds, entry.get("trace_sequence"));
        let window_bytes = match trial_bounds {
            None => null_window_bytes,
            Some((first_sequence, last_sequence)) => {
                serialized_json_len_v1(&HistoryTraceWindowMeasure {
                    first_sequence,
                    last_sequence,
                })
                .map_err(history_measurement_error)?
            }
        };
        // The admitted prefix is charged incrementally: skeleton (which budgets the longer
        // `false` spelling), each entry's own bytes, one separator per further entry, and
        // the trial trace window. Nothing admitted is ever reserialized. A checked-size
        // failure is the history view's existing inconsistent-state error, kept distinct
        // from an ordinary over-budget refusal.
        let total = skeleton_bytes
            .checked_add(admitted_bytes)
            .and_then(|bytes| bytes.checked_add(entry_bytes))
            .and_then(|bytes| bytes.checked_add(bounded.len()))
            .and_then(|bytes| bytes.checked_add(window_bytes))
            .ok_or(GraphViewErrorV2::InconsistentState(
                "history view is not JSON",
            ))?;
        if total > HISTORY_WINDOW_BYTES_MAX {
            // The refused candidate leaves the admitted accounting unchanged, and no
            // further source entry is pulled after a byte-overflow cut.
            truncated = true;
            break;
        }
        admitted_bytes += entry_bytes;
        admitted_bounds = trial_bounds;
        bounded.push(entry);
    }
    if !truncated && entries.next().is_some() {
        // A full window only asks whether another filtered source entry exists; that
        // lookahead payload is never rendered, so even a failing render there is only a
        // truncation fact.
        truncated = true;
    }
    Ok(history_window_value(bounded, truncated))
}

/// Whole-window serialization oracle for tests: measures an admitted window in one serde
/// pass, the way the budget was computed before incremental accounting.
#[cfg(test)]
fn history_window_measure_bytes(entries: &[Value]) -> Result<usize, GraphViewErrorV2> {
    let window = HistoryWindowMeasure {
        entries,
        trace_truncated: false,
        trace_window: trace_bounds(entries).map(|(first_sequence, last_sequence)| {
            HistoryTraceWindowMeasure {
                first_sequence,
                last_sequence,
            }
        }),
    };
    serde_json::to_vec(&window)
        .map(|encoded| encoded.len())
        .map_err(|_| GraphViewErrorV2::InconsistentState("history view is not JSON"))
}

fn history_window_value(entries: Vec<Value>, trace_truncated: bool) -> Value {
    let trace_window = trace_bounds(&entries).map(|(first_sequence, last_sequence)| {
        let mut bounds = Map::with_capacity(2);
        bounds.insert("first_sequence".to_owned(), json!(first_sequence));
        bounds.insert("last_sequence".to_owned(), json!(last_sequence));
        Value::Object(bounds)
    });
    // The admitted entries move straight into the result: the `json!` macro finalizes owned
    // expressions through `to_value`, which would reserialize every entry once more.
    let mut result = Map::with_capacity(3);
    result.insert("entries".to_owned(), Value::Array(entries));
    result.insert("trace_truncated".to_owned(), Value::Bool(trace_truncated));
    result.insert(
        "trace_window".to_owned(),
        trace_window.unwrap_or(Value::Null),
    );
    Value::Object(result)
}

fn trace_entry(
    state: &GraphSessionStateV2,
    procedure: &ParsedProcedureV2,
    attempt: &SessionAttemptV2,
) -> Result<Value, GraphViewErrorV2> {
    let metadata = attempt_metadata(state, attempt)?;
    let definition_id = definition_id_for_node(procedure, attempt.graph_node_id().as_str())?;
    let mut value = Map::new();
    value.insert("trace_sequence".to_owned(), json!(attempt.trace().get()));
    value.insert(
        "graph_node_id".to_owned(),
        json!(attempt.graph_node_id().as_str()),
    );
    value.insert("node_definition_id".to_owned(), json!(definition_id));
    value.insert(
        "attempt_id".to_owned(),
        json!(attempt.attempt_id().as_str()),
    );
    value.insert("attempt_number".to_owned(), json!(attempt.number().get()));
    value.insert(
        "goal_revision".to_owned(),
        attempt
            .goal_revision()
            .map_or(Value::Null, |revision| json!(revision.get())),
    );
    value.insert(
        "lifecycle".to_owned(),
        json!(attempt_lifecycle(attempt.lifecycle())),
    );
    value.insert(
        "validity".to_owned(),
        json!(attempt_validity(attempt.validity())),
    );
    value.insert(
        "started_at".to_owned(),
        json!(timestamp(metadata.started_at())?),
    );
    if let Some(finished) = metadata.ended_at() {
        value.insert("finished_at".to_owned(), json!(timestamp(finished)?));
    }
    Ok(Value::Object(value))
}

fn stale_attempt_value(
    state: &GraphSessionStateV2,
    procedure: &ParsedProcedureV2,
    attempt: &SessionAttemptV2,
) -> Result<Value, GraphViewErrorV2> {
    let metadata = attempt_metadata(state, attempt)?;
    let memory = state
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == attempt.attempt_id())
        .ok_or(GraphViewErrorV2::InconsistentState(
            "stale attempt memory is absent",
        ))?;
    let definition_id = definition_id_for_node(procedure, attempt.graph_node_id().as_str())?;
    // The summary counts every slot with a recorded value but formats only the first three of
    // those valued slots: empty slots contribute neither an item nor a count, and valued slots
    // beyond the displayed window stay counted but unformatted.
    let mut items = Vec::new();
    let mut items_total = 0usize;
    for slot in memory.item_slots() {
        let Some(value) = slot.value() else {
            continue;
        };
        items_total += 1;
        if items.len() >= 3 {
            continue;
        }
        let display = display_item_value(value);
        let (display, truncated) = truncate_chars(&display, ITEM_DISPLAY_CHARS_MAX);
        items.push(json!({"item_id": slot.item_id().as_str(), "value": display, "value_truncated": truncated}));
    }
    Ok(json!({
        "trace_sequence": attempt.trace().get(),
        "graph_node_id": attempt.graph_node_id().as_str(),
        "node_definition_id": definition_id,
        "attempt_id": attempt.attempt_id().as_str(),
        "attempt_number": attempt.number().get(),
        "goal_revision": attempt.goal_revision().map(|revision| revision.get()),
        "lifecycle": attempt_lifecycle(attempt.lifecycle()),
        "validity": "stale",
        "started_at": timestamp(metadata.started_at())?,
        "finished_at": timestamp(metadata.ended_at().ok_or(GraphViewErrorV2::InconsistentState("stale attempt has no finish timestamp"))?)?,
        "terminal_reason": metadata.terminal_reason(),
        "items": items,
        "items_total": items_total,
        "items_truncated": items_total > 3,
        "references": memory.evidence().iter().map(|reference| {
            reference_metadata(reference, procedure, !reference.resolution().is_unresolved())
        }).collect::<Result<Vec<_>, _>>()?,
    }))
}

fn decision_value(record: &podway_core::DecisionRecordV2) -> Result<Value, GraphViewErrorV2> {
    let mut value = json!({
        "trace_sequence": record.trace().get(),
        "session_id": record.session_id().as_str(),
        "session_revision": record.session_revision().get(),
        "procedure_schema": "podway.procedure/v2",
        "procedure_snapshot_id": record.procedure_snapshot_id().as_str(),
        "procedure_digest": record.procedure_digest().as_str(),
        "graph_node_id": record.graph_node_id().as_str(),
        "node_definition_id": record.node_definition_id().as_str(),
        "attempt_id": record.attempt_id().as_str(),
        "attempt_number": record.attempt_number().get(),
        "goal_revision": record.goal_revision().map(|revision| revision.get()),
        "option_id": record.selected_option().as_str(),
        "effect": record.route_effect().as_str(),
        "target_graph_node_id": record.route_target().as_str(),
        "reason": record.reason().as_str(),
        "recorded_at": timestamp(record.recorded_at())?,
        "references": record.evidence().references().iter().map(reference_snapshot_value).collect::<Vec<_>>(),
    });
    if let Some(actor) = record.actor()
        && let Some(object) = value.as_object_mut()
    {
        object.insert("actor".to_owned(), json!(actor.as_str()));
    }
    Ok(value)
}

pub(crate) fn decision_readback_projection(
    state: &GraphSessionStateV2,
    decision: &podway_core::DecisionRecordV2,
) -> Result<Value, GraphViewErrorV2> {
    let mut value = decision_value(decision)?;
    let Some(assessment) = state.goal_state().assessments().iter().find(|assessment| {
        assessment.decision_attempt_id() == decision.attempt_id()
            && assessment.decision_graph_node_id() == decision.graph_node_id()
            && assessment.decision_trace() == decision.trace()
            && decision.goal_revision() == Some(assessment.goal_revision())
    }) else {
        return Ok(value);
    };
    let object = value
        .as_object_mut()
        .ok_or(GraphViewErrorV2::InconsistentState(
            "decision projection is not an object",
        ))?;
    object.insert("assessment".to_owned(), json!("session_goal"));
    object.insert(
        "assessment_mode".to_owned(),
        json!(assessment.mode().as_str()),
    );
    object.insert(
        "goal_outcome".to_owned(),
        json!(assessment.outcome().as_str()),
    );
    object.insert(
        "criterion_results".to_owned(),
        Value::Array(
            assessment
                .criterion_results()
                .iter()
                .map(|result| {
                    json!({
                        "criterion_id": result.criterion_id().as_str(),
                        "status": result.status().as_str(),
                        "reason": result.reason().as_str(),
                        "citations": result.citations().iter().map(citation_value).collect::<Vec<_>>(),
                    })
                })
                .collect(),
        ),
    );
    Ok(value)
}

fn reference_snapshot_value(reference: &ResolvedEvidenceReferenceV2) -> Value {
    let mut value = Map::new();
    value.insert(
        "source_graph_node_id".to_owned(),
        json!(reference.source_node().as_str()),
    );
    match reference {
        ResolvedEvidenceReferenceV2::Resolved(snapshot) => {
            value.insert("state".to_owned(), json!("resolved"));
            add_reference_snapshot_fields(&mut value, snapshot);
        }
        ResolvedEvidenceReferenceV2::Skipped(snapshot) => {
            value.insert("state".to_owned(), json!("skipped"));
            add_reference_snapshot_fields(&mut value, snapshot);
        }
        ResolvedEvidenceReferenceV2::Unresolved { .. } => {
            value.insert("state".to_owned(), json!("unresolved"));
        }
    }
    Value::Object(value)
}

fn add_reference_snapshot_fields(
    value: &mut Map<String, Value>,
    snapshot: &podway_core::EvidenceReferenceSnapshotV2,
) {
    value.insert(
        "source_attempt_id".to_owned(),
        json!(snapshot.source_attempt_id().as_str()),
    );
    value.insert(
        "source_attempt_number".to_owned(),
        json!(snapshot.source_attempt_number().get()),
    );
    value.insert(
        "items_digest".to_owned(),
        json!(snapshot.items_digest().as_str()),
    );
}

fn stale_goal_revision_value(
    state: &GraphSessionStateV2,
    record: &podway_core::GoalRevisionRecordV2,
) -> Result<Value, GraphViewErrorV2> {
    let assessment = state
        .goal_state()
        .assessments()
        .iter()
        .filter(|assessment| assessment.goal_revision() == record.revision())
        .max_by_key(|assessment| assessment.decision_trace());
    Ok(json!({
        "trace_sequence": record.binding_trace().get(),
        "revision": record.revision().get(),
        "predecessor_revision": record.predecessor().map(|revision| revision.get()),
        "statement": record.statement().as_str(),
        "criteria": record.criteria().criteria().iter().map(|criterion| json!({
            "criterion_id": criterion.id().as_str(),
            "statement": criterion.statement(),
            "status": assessment.and_then(|assessment| assessment.criterion_results().iter().find(|result| result.criterion_id() == criterion.id())).map_or("unassessed", |result| result.status().as_str()),
        })).collect::<Vec<_>>(),
        "actor": record.actor().map(|actor| actor.as_str()),
        "recorded_at": timestamp(record.created_at())?,
        "rework_to": record.rework_to().map(|node| node.as_str()),
        "reactivated": record.reactivated(),
    }))
}

fn stale_goal_assessment_value(
    state: &GraphSessionStateV2,
    assessment: &podway_core::GoalAssessmentRecordV2,
) -> Result<Value, GraphViewErrorV2> {
    let decision = state
        .workflow_memory()
        .decisions()
        .iter()
        .find(|record| record.attempt_id() == assessment.decision_attempt_id())
        .ok_or(GraphViewErrorV2::InconsistentState(
            "goal assessment decision record is absent",
        ))?;
    Ok(json!({
        "trace_sequence": assessment.decision_trace().get(),
        "session_id": state.trace().session_id().as_str(),
        "session_revision": decision.session_revision().get(),
        "procedure_snapshot_id": state.snapshot().snapshot_id().as_str(),
        "procedure_digest": state.snapshot().digest().as_str(),
        "graph_node_id": assessment.decision_graph_node_id().as_str(),
        "node_definition_id": decision.node_definition_id().as_str(),
        "attempt_id": assessment.decision_attempt_id().as_str(),
        "attempt_number": decision.attempt_number().get(),
        "goal_revision": assessment.goal_revision().get(),
        "assessment": "session_goal",
        "option_id": decision.selected_option().as_str(),
        "effect": decision.route_effect().as_str(),
        "target_graph_node_id": decision.route_target().as_str(),
        "mode": assessment.mode().as_str(),
        "outcome": assessment.outcome().as_str(),
        "criterion_statuses": assessment.criterion_results().iter().map(|result| json!({
            "criterion_id": result.criterion_id().as_str(),
            "status": result.status().as_str(),
            "citations": result.citations().iter().map(citation_value).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "references": assessment.evidence().references().iter().map(reference_snapshot_value).collect::<Vec<_>>(),
        "actor": assessment.actor().map(|actor| actor.as_str()),
        "recorded_at": timestamp(assessment.recorded_at())?,
        "record_digest": goal_assessment_digest(state, decision, assessment)?,
    }))
}

fn goal_assessment_digest(
    state: &GraphSessionStateV2,
    decision: &podway_core::DecisionRecordV2,
    assessment: &podway_core::GoalAssessmentRecordV2,
) -> Result<String, GraphViewErrorV2> {
    let criterion_results = assessment
        .criterion_results()
        .iter()
        .map(|result| {
            let citations = result
                .citations()
                .iter()
                .map(|citation| {
                    if let Some(source) = citation.as_evidence() {
                        json!({"kind": "evidence", "source_graph_node_id": source.as_str()})
                    } else if let Some(item) = citation.as_item() {
                        json!({"item_id": item.as_str(), "kind": "item"})
                    } else {
                        Value::Null
                    }
                })
                .collect::<Vec<_>>();
            json!({
                "citations": citations,
                "criterion_id": result.criterion_id().as_str(),
                "reason": result.reason().as_str(),
                "status": result.status().as_str(),
            })
        })
        .collect::<Vec<_>>();
    let evidence = assessment
        .evidence()
        .references()
        .iter()
        .map(|reference| match reference {
            ResolvedEvidenceReferenceV2::Resolved(value) => json!({
                "items_digest": value.items_digest().as_str(),
                "resolved_at_ms": value.resolved_at().get(),
                "source_attempt_id": value.source_attempt_id().as_str(),
                "source_attempt_number": value.source_attempt_number().get(),
                "source_graph_node_id": value.source_node().as_str(),
                "state": "resolved",
            }),
            ResolvedEvidenceReferenceV2::Skipped(value) => json!({
                "items_digest": value.items_digest().as_str(),
                "resolved_at_ms": value.resolved_at().get(),
                "source_attempt_id": value.source_attempt_id().as_str(),
                "source_attempt_number": value.source_attempt_number().get(),
                "source_graph_node_id": value.source_node().as_str(),
                "state": "skipped",
            }),
            ResolvedEvidenceReferenceV2::Unresolved { source_node } => json!({
                "source_graph_node_id": source_node.as_str(),
                "state": "unresolved",
            }),
        })
        .collect::<Vec<_>>();
    let value = json!({
        "actor": assessment.actor().map(|actor| actor.as_str()),
        "criterion_results": criterion_results,
        "decision_attempt_id": assessment.decision_attempt_id().as_str(),
        "decision_graph_node_id": assessment.decision_graph_node_id().as_str(),
        "decision_trace_sequence": assessment.decision_trace().get(),
        "evidence": evidence,
        "goal_revision": assessment.goal_revision().get(),
        "mode": assessment.mode().as_str(),
        "outcome": assessment.outcome().as_str(),
        "recorded_at_ms": assessment.recorded_at().get(),
        "route_effect": decision.route_effect().as_str(),
        "route_target_graph_node_id": decision.route_target().as_str(),
        "selected_option_id": decision.selected_option().as_str(),
        "session_id": state.trace().session_id().as_str(),
    });
    let canonical = canonicalize_json_v1(&value).map_err(|_| {
        GraphViewErrorV2::InconsistentState("goal assessment is not canonicalizable")
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
}

fn citation_value(citation: &CriterionCitationV2) -> Value {
    if let Some(node) = citation.as_evidence() {
        json!({"reference_graph_node_id": node.as_str()})
    } else if let Some(item) = citation.as_item() {
        json!({"local_item_id": item.as_str()})
    } else {
        Value::Null
    }
}

fn attempt_metadata<'a>(
    state: &'a GraphSessionStateV2,
    attempt: &SessionAttemptV2,
) -> Result<&'a podway_store::AttemptMetadataV2, GraphViewErrorV2> {
    state
        .attempt_metadata()
        .iter()
        .find(|metadata| metadata.attempt_id() == attempt.attempt_id())
        .ok_or(GraphViewErrorV2::InconsistentState(
            "attempt metadata is absent",
        ))
}

fn definition_id_for_node<'a>(
    procedure: &'a ParsedProcedureV2,
    node_id: &str,
) -> Result<&'a str, GraphViewErrorV2> {
    procedure
        .graph()
        .placements()
        .iter()
        .find(|placement| placement.id().as_str() == node_id)
        .map(|placement| match placement {
            GraphPlacementV2::Action(placement) => placement.definition().as_str(),
            GraphPlacementV2::Decision(placement) => placement.definition().as_str(),
        })
        .ok_or(GraphViewErrorV2::InvalidSnapshot)
}

fn display_item_value(value: &RecordedItemValueV2) -> String {
    match value.item_type() {
        ItemTypeV1::Confirm => "true".to_owned(),
        ItemTypeV1::Text => value.as_text().unwrap_or_default().to_owned(),
        ItemTypeV1::Choice => value.as_choice().unwrap_or_default().to_owned(),
        ItemTypeV1::Integer => value.as_integer().unwrap_or_default().to_string(),
        ItemTypeV1::List => serde_json::to_string(value.as_list().unwrap_or_default())
            .unwrap_or_else(|_| "[]".to_owned()),
        ItemTypeV1::Artifact => {
            serde_json::to_string(&typed_item_value(value)).unwrap_or_else(|_| "{}".to_owned())
        }
        ItemTypeV1::CheckResult => {
            serde_json::to_string(&typed_item_value(value)).unwrap_or_else(|_| "{}".to_owned())
        }
    }
}

fn typed_item_value(value: &RecordedItemValueV2) -> Value {
    match value.item_type() {
        ItemTypeV1::Confirm => Value::Bool(true),
        ItemTypeV1::Text => json!(value.as_text().unwrap_or_default()),
        ItemTypeV1::Choice => json!(value.as_choice().unwrap_or_default()),
        ItemTypeV1::Integer => json!(value.as_integer().unwrap_or_default()),
        ItemTypeV1::List => json!(value.as_list().unwrap_or_default()),
        ItemTypeV1::Artifact => {
            let artifact = value
                .as_artifact()
                .expect("artifact type has an artifact value");
            json!({
                "location_type": match artifact.location_kind() {
                    podway_core::ArtifactLocationKindV1::LocalPath => "path",
                    podway_core::ArtifactLocationKindV1::ExternalReference => "reference",
                },
                "location": artifact.location(),
                "sha256_digest": artifact.digest().as_str(),
                "size_bytes": artifact.size_bytes(),
                "media_type": artifact.media_type(),
            })
        }
        ItemTypeV1::CheckResult => {
            let result = value
                .as_check_result()
                .expect("check-result type has a check-result value");
            json!({
                "operation_id": result.operation_id().as_str(),
                "operation_digest": result.operation_digest().as_str(),
                "input_basis": {
                    "descriptor": result.input_basis().descriptor(),
                    "digest": result.input_basis().digest().as_str(),
                },
                "executor": {
                    "name": result.executor().name(),
                    "version": result.executor().version(),
                },
                "outcome": result.outcome().as_str(),
                "summary": result.summary(),
                "output_digest": result.output_digest().as_str(),
            })
        }
    }
}

fn check_result_preview_v1(value: &RecordedItemValueV2) -> Value {
    let result = value
        .as_check_result()
        .expect("check-result projection requires a check-result value");
    json!({
        "operation_id": result.operation_id().as_str(),
        "outcome": result.outcome().as_str(),
        "operation_digest": result.operation_digest().as_str(),
        "input_basis": { "digest": result.input_basis().digest().as_str() },
        "output_digest": result.output_digest().as_str(),
    })
}

fn check_result_encoded_bytes_v1(value: &RecordedItemValueV2) -> usize {
    canonicalize_json_v1(&typed_item_value(value))
        .expect("a bounded check-result value is canonical JSON")
        .len()
}

fn truncate_chars(value: &str, maximum: usize) -> (String, bool) {
    if value.chars().count() <= maximum {
        return (value.to_owned(), false);
    }
    (value.chars().take(maximum).collect(), true)
}

fn push_unique<'a>(values: &mut Vec<&'a str>, value: &'a str) {
    if !values.contains(&value) {
        values.push(value);
    }
}

const fn item_type(value: ItemTypeV1) -> &'static str {
    match value {
        ItemTypeV1::Confirm => "confirm",
        ItemTypeV1::Text => "text",
        ItemTypeV1::Choice => "choice",
        ItemTypeV1::Integer => "integer",
        ItemTypeV1::List => "list",
        ItemTypeV1::Artifact => "artifact",
        ItemTypeV1::CheckResult => "check_result",
    }
}

const fn lifecycle(value: SessionLifecycle) -> &'static str {
    match value {
        SessionLifecycle::Prepared => "prepared",
        SessionLifecycle::Running => "running",
        SessionLifecycle::Completed => "completed",
        SessionLifecycle::Cancelled => "cancelled",
    }
}

const fn attempt_lifecycle(value: AttemptLifecycle) -> &'static str {
    match value {
        AttemptLifecycle::Active => "active",
        AttemptLifecycle::Completed => "completed",
        AttemptLifecycle::Skipped => "skipped",
        AttemptLifecycle::Abandoned => "abandoned",
    }
}

const fn attempt_validity(value: AttemptValidityV2) -> &'static str {
    match value {
        AttemptValidityV2::Valid => "valid",
        AttemptValidityV2::Stale => "stale",
    }
}

fn timestamp(value: UnixMillis) -> Result<String, GraphViewErrorV2> {
    let seconds = value.get() / 1_000;
    let millis = value.get() % 1_000;
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_date_from_unix_days(days);
    if !(0..=9_999).contains(&year) {
        return Err(GraphViewErrorV2::TimestampOutOfRange);
    }
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60,
    ))
}

fn civil_date_from_unix_days(days: u64) -> (i128, i128, i128) {
    let z = i128::from(days) + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::{
        observation::{
            SuccessorObservationContextV1, admit_members_within_bytes_v1,
            admit_templates_within_bytes_v1, admit_within_bytes_v1,
            project_successor_observation_v1, serialized_bytes_v1, serialized_object_bytes_v1,
        },
        *,
    };
    use podway_core::MAX_OBSERVATION_STATIC_MEMBERS_BYTES_V2;

    #[test]
    fn v2gsc004_production_counter_encoder_matches_the_shared_maximum_charge() {
        let identifiers = (0..podway_core::MAX_GRAPH_NODES_PER_PROCEDURE_V2)
            .map(|index| format!("{}{:04}", "n".repeat(60), index))
            .collect::<Vec<_>>();
        let counters = encode_graph_counters_v2(
            identifiers
                .iter()
                .map(|identifier| (identifier.as_str(), u64::MAX, u64::MAX)),
        );
        let encoded = serde_json::to_vec(&counters).unwrap();
        let charged = podway_core::maximum_graph_counter_array_json_bytes_v2(
            identifiers.iter().map(String::len),
        );
        assert_eq!(encoded.len(), charged);
        assert_eq!(charged, 172_033);
    }

    fn entry(width: usize) -> Value {
        json!({ "id": "x".repeat(width) })
    }

    #[test]
    fn v2scl005_a_byte_window_cuts_and_reports_the_exact_total() {
        // Every admissible active-item set fits its allocation today, so this is the only place the
        // cutting branch is exercised. A window that stops silently is the defect ADR-0024 forbids,
        // so what the branch must produce is a short array beside an unchanged total.
        let entries: Vec<Value> = (0..8).map(|_| entry(100)).collect();
        let budget = serialized_bytes_v1(&entries[0]) * 3;
        let (admitted, total) = admit_within_bytes_v1(entries, budget, 64);
        assert_eq!(total, 8);
        assert_eq!(admitted.len(), 3);

        // A window bounded by its count rather than its bytes cuts the same way.
        let entries: Vec<Value> = (0..8).map(|_| entry(1)).collect();
        let (admitted, total) = admit_within_bytes_v1(entries, usize::MAX, 5);
        assert_eq!((admitted.len(), total), (5, 8));

        // One entry wider than the whole allocation is still admitted: a window that can answer
        // nothing tells a caller less than one that answers once and says it was cut.
        let entries = vec![entry(4_096), entry(1)];
        let (admitted, total) = admit_within_bytes_v1(entries, 16, 64);
        assert_eq!((admitted.len(), total), (1, 2));
    }

    #[test]
    fn v2gsc002_complete_observation_members_over_their_shared_allocation_fail_closed() {
        let oversized = json!({ "padding": "x".repeat(MAX_OBSERVATION_STATIC_MEMBERS_BYTES_V2) });
        assert!(matches!(
            admit_members_within_bytes_v1(
                [&oversized, &Value::Null],
                MAX_OBSERVATION_STATIC_MEMBERS_BYTES_V2
            ),
            Err(GraphViewErrorV2::InconsistentState(_))
        ));
        assert!(
            admit_members_within_bytes_v1(
                [&json!({ "padding": "x" }), &Value::Null],
                MAX_OBSERVATION_STATIC_MEMBERS_BYTES_V2
            )
            .is_ok()
        );

        // Each member fits independently. Their measured sum is accepted at equality and rejected
        // exactly one byte over, proving the helper cannot regress to max(member) or `>=`.
        let left = Value::String("x".repeat(40));
        let right = Value::String("y".repeat(40));
        let exact = serialized_bytes_v1(&left) + serialized_bytes_v1(&right);
        assert!(admit_members_within_bytes_v1([&left, &right], exact).is_ok());
        assert!(matches!(
            admit_members_within_bytes_v1([&left, &right], exact - 1),
            Err(GraphViewErrorV2::InconsistentState(_))
        ));
    }

    #[test]
    fn v2scl005_a_template_window_never_drops_a_command_it_would_have_offered() {
        // The reservation is what keeps `allowed_actions` answerable under truncation.
        let templates: Vec<Value> = ["item.set", "item.set", "item.check", "session.complete"]
            .iter()
            .map(|command| json!({ "command": command, "argv": ["podway", "x".repeat(64)] }))
            .collect();
        // Exactly the three distinct commands fit, so the duplicate `item.set` is what gets cut.
        let budget: usize = [0usize, 2, 3]
            .iter()
            .map(|index| serialized_bytes_v1(&templates[*index]))
            .sum();
        let (admitted, total) = admit_templates_within_bytes_v1(templates, budget, 64).unwrap();
        assert_eq!(total, 4);
        assert_eq!(admitted.len(), 3);
        let commands: Vec<&str> = admitted
            .iter()
            .map(|template| template["command"].as_str().unwrap())
            .collect();
        for command in ["item.set", "item.check", "session.complete"] {
            assert!(commands.contains(&command), "{command} lost its template");
        }

        // A reservation that cannot fit is refused rather than trimmed, because trimming it is
        // exactly the contradiction the reservation exists to prevent.
        let templates: Vec<Value> = (0..8)
            .map(|index| json!({ "command": format!("item.kind-{index}"), "argv": ["podway"] }))
            .collect();
        let budget = serialized_bytes_v1(&templates[0]) * 2;
        assert!(matches!(
            admit_templates_within_bytes_v1(templates, budget, 64),
            Err(GraphViewErrorV2::InconsistentState(_))
        ));
    }

    fn measure_carrier(entries: &[Value]) -> HistoryWindowMeasure<'_> {
        HistoryWindowMeasure {
            entries,
            trace_truncated: false,
            trace_window: trace_bounds(entries).map(|(first_sequence, last_sequence)| {
                HistoryTraceWindowMeasure {
                    first_sequence,
                    last_sequence,
                }
            }),
        }
    }

    /// The identity render used by value-backed test windows.
    fn ok_value(entry: Value) -> Result<Value, GraphViewErrorV2> {
        Ok(entry)
    }

    #[test]
    fn reads003_borrowed_history_measurement_matches_owned_construction_bytes() {
        let payload = "escape heavy \" \u{5c}\"\\\\ \u{2028}é   ".to_owned();
        let windows: [Vec<Value>; 5] = [
            Vec::new(),
            vec![json!({"trace_sequence": 0, "payload": payload.clone()})],
            vec![
                json!({"trace_sequence": u64::MAX, "payload": payload.clone()}),
                json!({"trace_sequence": u64::MIN, "payload": payload.clone()}),
                json!({"trace_sequence": 7, "payload": payload.clone()}),
            ],
            vec![json!({"payload": payload.clone()}), json!({"other": 1})],
            vec![json!({"trace_sequence": u64::MAX})],
        ];
        for entries in windows {
            let borrowed = serde_json::to_vec(&measure_carrier(&entries)).unwrap();
            let owned = serde_json::to_vec(&history_window_value(entries.clone(), false)).unwrap();
            assert_eq!(borrowed, owned, "window {entries:?}");
            assert_eq!(
                history_window_measure_bytes(&entries).unwrap(),
                owned.len(),
                "measured length {entries:?}"
            );
        }
    }

    #[test]
    fn reads003_history_window_exact_fit_admits_and_one_byte_over_truncates() {
        // Three no-padding entries fix the fixed overhead; every padding character adds exactly
        // one byte to the serialized window, so an exact fit and a one-byte miss are both exact.
        let bare = vec![json!({"payload": ""}); 3];
        let overhead = history_window_measure_bytes(&bare).unwrap();
        let padding = HISTORY_WINDOW_BYTES_MAX - overhead;
        let exact = vec![
            json!({ "payload": "x".repeat(padding - 2) }),
            json!({ "payload": "x" }),
            json!({ "payload": "x" }),
        ];
        assert_eq!(
            history_window_measure_bytes(&exact).unwrap(),
            HISTORY_WINDOW_BYTES_MAX
        );
        let window = history_window(exact.clone().into_iter(), ok_value, 8).unwrap();
        assert_eq!(window["entries"].as_array().unwrap().len(), 3);
        assert_eq!(window["trace_truncated"], false, "equality must admit");
        assert_eq!(window["trace_window"], Value::Null);

        let one_over = vec![
            json!({ "payload": "x".repeat(padding - 1) }),
            json!({ "payload": "x" }),
            json!({ "payload": "x" }),
        ];
        let window = history_window(one_over.into_iter(), ok_value, 8).unwrap();
        let admitted = window["entries"].as_array().unwrap();
        assert_eq!(admitted.len(), 2, "the admitted prefix is retained");
        assert_eq!(window["trace_truncated"], true);
        assert_eq!(admitted[0], json!({ "payload": "x".repeat(padding - 1) }));
    }

    #[test]
    fn reads003_history_window_swallows_trailing_error_after_a_full_window() {
        let entries = [json!({"trace_sequence": 3}), json!({"trace_sequence": 5})];
        let late = |entry: Value| {
            if entry["late"] == json!(true) {
                Err(GraphViewErrorV2::InconsistentState(
                    "an error behind a full window is only a truncation fact",
                ))
            } else {
                Ok(entry)
            }
        };
        let window = history_window(
            vec![
                entries[0].clone(),
                entries[1].clone(),
                json!({"late": true}),
                json!({"trace_sequence": 9}),
            ]
            .into_iter(),
            late,
            2,
        )
        .unwrap();
        assert_eq!(window["entries"].as_array().unwrap().len(), 2);
        assert_eq!(window["trace_truncated"], true);

        let propagated = history_window(
            vec![entries[0].clone(), json!({"late": true})].into_iter(),
            late,
            4,
        );
        assert!(matches!(
            propagated,
            Err(GraphViewErrorV2::InconsistentState(
                "an error behind a full window is only a truncation fact"
            ))
        ));
    }

    #[test]
    fn views002_history_window_incremental_total_matches_serialized_windows() {
        // The two plan fixtures pin the corrected wrapper accounting against real serde:
        // the skeleton keeps the entries bracket pair, and only the null trace-window
        // placeholder is replaced by each candidate's measured window bytes.
        let null_window = vec![Value::Null];
        assert_eq!(history_window_measure_bytes(&null_window).unwrap(), 62);
        let traced = vec![json!({"trace_sequence": 10_000, "payload": "x"})];
        assert_eq!(history_window_measure_bytes(&traced).unwrap(), 138);

        // The former whole-window algorithm as a reference: admit while the serialized
        // window fits, cut on the first refusal, and mark truncation whenever more
        // filtered entries exist.
        fn reference_window(entries: &[Value], maximum: usize) -> (usize, bool) {
            let mut admitted = 0usize;
            let mut truncated = false;
            while admitted < maximum && admitted < entries.len() {
                if history_window_measure_bytes(&entries[..admitted + 1]).unwrap()
                    > HISTORY_WINDOW_BYTES_MAX
                {
                    truncated = true;
                    break;
                }
                admitted += 1;
            }
            if !truncated && admitted < entries.len() {
                truncated = true;
            }
            (admitted, truncated)
        }

        let escape_heavy = "escape heavy \" \\ \u{2028} é \u{0007}";
        let entry =
            |trace: u64, payload: String| json!({"trace_sequence": trace, "payload": payload});
        let untraced = |payload: String| json!({"payload": payload});
        let null_traced = |payload: String| json!({"trace_sequence": null, "payload": payload});

        // Padding is derived from the oracle, never hand-counted: three traced 4-digit
        // entries fix the overhead and every payload byte adds exactly one byte.
        let bare: Vec<Value> = (0..3).map(|_| entry(9_999, String::new())).collect();
        let overhead = history_window_measure_bytes(&bare).unwrap();
        let padding = HISTORY_WINDOW_BYTES_MAX - overhead;
        let exact_fit = vec![
            entry(9_999, "x".repeat(padding - 2)),
            entry(9_999, "x".to_owned()),
            entry(9_999, "x".to_owned()),
        ];
        assert_eq!(
            history_window_measure_bytes(&exact_fit).unwrap(),
            HISTORY_WINDOW_BYTES_MAX,
            "exact-fit fixture must serialize to exactly the budget"
        );
        let one_byte_over = vec![
            entry(9_999, "x".repeat(padding - 1)),
            entry(9_999, "x".to_owned()),
            entry(9_999, "x".to_owned()),
        ];

        // A widened maximum digit must be charged in both places it appears: the entry's
        // own `trace_sequence` member and the window bound. The pair is padded so the
        // all-4-digit window hits the budget exactly; one further digit on the last
        // sequence adds two bytes and refuses the second entry.
        let pair_bare = vec![entry(9_999, String::new()), entry(9_999, String::new())];
        let pair_padding =
            HISTORY_WINDOW_BYTES_MAX - history_window_measure_bytes(&pair_bare).unwrap();
        let digits_admit = vec![
            entry(9_999, "x".repeat(pair_padding - 1)),
            entry(9_999, "x".to_owned()),
        ];
        assert_eq!(
            history_window_measure_bytes(&digits_admit).unwrap(),
            HISTORY_WINDOW_BYTES_MAX
        );
        let digits_refuse = vec![
            entry(9_999, "x".repeat(pair_padding - 1)),
            entry(10_000, "x".to_owned()),
        ];
        assert_eq!(
            history_window_measure_bytes(&digits_refuse).unwrap(),
            HISTORY_WINDOW_BYTES_MAX + 2
        );

        // A shrinking minimum is charged the same way: padding fits when the fold narrows
        // the first sequence to 4 digits (entry member and window bound each lose one
        // byte) and refuses when both entries keep 5 digits.
        let shrink_bare = vec![entry(10_000, String::new()), entry(10_000, String::new())];
        let shrink_padding =
            HISTORY_WINDOW_BYTES_MAX - history_window_measure_bytes(&shrink_bare).unwrap() + 2;
        let min_shrinks = vec![
            entry(10_000, "x".repeat(shrink_padding - 1)),
            entry(9_999, "x".to_owned()),
        ];
        assert_eq!(
            history_window_measure_bytes(&min_shrinks).unwrap(),
            HISTORY_WINDOW_BYTES_MAX
        );
        let min_stays = vec![
            entry(10_000, "x".repeat(shrink_padding - 1)),
            entry(10_000, "x".to_owned()),
        ];
        assert_eq!(
            history_window_measure_bytes(&min_stays).unwrap(),
            HISTORY_WINDOW_BYTES_MAX + 2
        );

        // Discriminating max-widen edge: the 4-digit window sits one byte under the budget
        // (65535) and the widened-max window measures 65537. An implementation that
        // charges the candidate entry's extra digit but forgets only the max-bound digit
        // would measure 65536 and wrongly admit, so the reference refusal exposes it.
        let edge_padding =
            HISTORY_WINDOW_BYTES_MAX - 1 - history_window_measure_bytes(&pair_bare).unwrap();
        let digits_edge_admit = vec![
            entry(9_999, "x".repeat(edge_padding - 1)),
            entry(9_999, "x".to_owned()),
        ];
        assert_eq!(
            history_window_measure_bytes(&digits_edge_admit).unwrap(),
            HISTORY_WINDOW_BYTES_MAX - 1
        );
        let digits_edge_refuse = vec![
            entry(9_999, "x".repeat(edge_padding - 1)),
            entry(10_000, "x".to_owned()),
        ];
        assert_eq!(
            history_window_measure_bytes(&digits_edge_refuse).unwrap(),
            HISTORY_WINDOW_BYTES_MAX + 1
        );

        // Byte-boundary None -> Some trace-window transition: an untraced admitted prefix
        // followed by the first traced candidate measures exactly one byte over, so
        // omitting the newly introduced trace object from the trial charge is observable.
        let transition_bare = vec![untraced(String::new()), entry(7, "x".to_owned())];
        let transition_padding =
            HISTORY_WINDOW_BYTES_MAX + 1 - history_window_measure_bytes(&transition_bare).unwrap();
        let trace_transition = vec![
            untraced("x".repeat(transition_padding)),
            entry(7, "x".to_owned()),
        ];
        assert_eq!(
            history_window_measure_bytes(&trace_transition).unwrap(),
            HISTORY_WINDOW_BYTES_MAX + 1
        );
        assert!(
            history_window_measure_bytes(&trace_transition[..1]).unwrap()
                <= HISTORY_WINDOW_BYTES_MAX,
            "the untraced prefix must itself be admissible"
        );

        // Trace gaps: absent, null and escaped or nested entries never contribute to the
        // bounds; the extremes come from the participating entries alone.
        let gaps = vec![
            entry(5, escape_heavy.to_owned()),
            untraced(r#"{"nested": ["a\"", 1, null]}"#.to_owned()),
            json!({"trace_sequence": 7, "payload": {"nested": ["a\"", 1, null]}}),
            null_traced("gap".to_owned()),
            entry(9_999, "tail".to_owned()),
        ];
        let untraced_window = vec![
            untraced(escape_heavy.to_owned()),
            null_traced("gap".to_owned()),
        ];

        let corpora: Vec<(Vec<Value>, usize)> = vec![
            (null_window, 8),
            (traced, 8),
            (exact_fit, 8),
            (one_byte_over, 8),
            (digits_admit, 8),
            (digits_refuse, 8),
            (digits_edge_admit, 8),
            (digits_edge_refuse, 8),
            (trace_transition, 8),
            (min_shrinks, 8),
            (min_stays, 8),
            (gaps.clone(), 8),
            (untraced_window.clone(), 8),
            (vec![entry(u64::MAX, String::new()); 32], 32),
            (vec![entry(u64::MAX, String::new()); 33], 32),
        ];
        for (entries, maximum) in corpora {
            let (expected_admitted, expected_truncated) = reference_window(&entries, maximum);
            let window = history_window(entries.clone().into_iter(), ok_value, maximum).unwrap();
            assert_eq!(
                window["entries"].as_array().unwrap().len(),
                expected_admitted,
                "admitted prefix for {entries:?}"
            );
            assert_eq!(
                window["trace_truncated"],
                json!(expected_truncated),
                "truncation for {entries:?}"
            );
        }

        let window = history_window(gaps.into_iter(), ok_value, 8).unwrap();
        assert_eq!(
            window["trace_window"],
            json!({"first_sequence": 5, "last_sequence": 9_999})
        );
        let window = history_window(untraced_window.into_iter(), ok_value, 8).unwrap();
        assert_eq!(window["trace_window"], Value::Null);
    }

    #[test]
    fn views002_history_window_full_window_lookahead_does_not_render() {
        use std::{cell::Cell, rc::Rc};

        let render_calls = Rc::new(Cell::new(0usize));
        let counter = Rc::clone(&render_calls);
        let render = move |entry: Value| {
            counter.set(counter.get() + 1);
            Ok(entry)
        };
        let raw: Vec<Value> = (0..5)
            .map(|index| json!({"trace_sequence": index}))
            .collect();
        let window = history_window(raw.into_iter(), render, 2).unwrap();
        // Exactly the admitted entries are rendered; the discarded lookahead entry behind
        // the full window never reaches its render.
        assert_eq!(render_calls.get(), 2);
        assert_eq!(window["entries"].as_array().unwrap().len(), 2);
        assert_eq!(window["trace_truncated"], true);
    }

    #[test]
    fn views002_history_window_byte_overflow_stops_all_lookahead() {
        use std::{cell::Cell, rc::Rc};

        struct CountingIter<I> {
            inner: I,
            pulls: Rc<Cell<usize>>,
        }
        impl<I: Iterator> Iterator for CountingIter<I> {
            type Item = I::Item;
            fn next(&mut self) -> Option<Self::Item> {
                self.pulls.set(self.pulls.get() + 1);
                self.inner.next()
            }
        }

        let pulls = Rc::new(Cell::new(0usize));
        let counting = CountingIter {
            inner: vec![
                json!({"trace_sequence": 1, "payload": "x".repeat(40_000)}),
                json!({"trace_sequence": 2, "payload": "y".repeat(40_000)}),
                json!({"trace_sequence": 3, "payload": "z"}),
            ]
            .into_iter(),
            pulls: Rc::clone(&pulls),
        };
        let render_calls = Rc::new(Cell::new(0usize));
        let counter = Rc::clone(&render_calls);
        let render = move |entry: Value| {
            counter.set(counter.get() + 1);
            Ok(entry)
        };
        let window = history_window(counting, render, 8).unwrap();
        let admitted = window["entries"].as_array().unwrap();
        assert_eq!(
            admitted.len(),
            1,
            "the second 40k entry overflows the budget"
        );
        assert_eq!(window["trace_truncated"], true);
        // The refused candidate is rendered and pulled once to establish the overflow;
        // nothing behind the cut is rendered or pulled.
        assert_eq!(render_calls.get(), 2);
        assert_eq!(
            pulls.get(),
            2,
            "no raw lookahead pull after a byte overflow"
        );
    }

    #[test]
    fn views002_observation_list_incremental_admission_matches_serialized_prefixes() {
        // The former whole-prefix algorithm as a reference, driven by real serde
        // serialization of every candidate prefix.
        fn reference(entries: &[String]) -> (Vec<String>, bool) {
            let mut projected: Vec<String> = Vec::new();
            let mut truncated = false;
            for entry in entries {
                let (entry, entry_truncated) =
                    truncate_chars(entry, OBSERVATION_ITEM_VALUE_CHARS_MAX);
                projected.push(entry);
                if serde_json::to_vec(&projected).map_or(true, |encoded| {
                    encoded.len() > OBSERVATION_ITEM_VALUE_BYTES_MAX
                }) {
                    projected.pop();
                    truncated = true;
                    break;
                }
                truncated |= entry_truncated;
            }
            truncated |= projected.len() < entries.len();
            (projected, truncated)
        }

        let corpora: Vec<Vec<String>> = vec![
            Vec::new(),
            vec!["x".to_owned()],
            vec!["short".to_owned(); 3],
            // Per-entry character caps in the three byte-cost regimes: one scalar per
            // byte, two bytes per scalar, and one escaped scalar per two bytes.
            vec!["a".repeat(128); 8],
            vec!["é".repeat(64); 8],
            vec!["\"".repeat(64); 8],
            vec![
                "a".repeat(128),
                "é".repeat(64),
                "\"".repeat(64),
                "mixed \"é\" a".repeat(12),
                "x".repeat(200),
                "gap entry".to_owned(),
                "tail".to_owned(),
            ],
            vec!["z".repeat(128); 7]
                .into_iter()
                .chain(["z".repeat(104)])
                .collect(),
        ];
        for entries in &corpora {
            let value = RecordedItemValueV2::list(entries.clone()).unwrap();
            let (projected, truncated) = project_observation_item_value(&value);
            let (expected, expected_truncated) = reference(entries);
            assert_eq!(projected, Value::from(expected.clone()), "{entries:?}");
            assert_eq!(truncated, expected_truncated, "{entries:?}");
        }
    }

    #[test]
    fn reads003_observation_list_projection_measures_in_place() {
        let fits = RecordedItemValueV2::list(vec!["short".to_owned(); 3]).unwrap();
        let (value, truncated) = project_observation_item_value(&fits);
        assert!(!truncated);
        assert_eq!(value, json!(["short", "short", "short"]));

        // One entry over the per-entry character maximum marks truncation even though the
        // whole list is admitted: the flag carries entry truncation, not just window cuts.
        let long_entry = RecordedItemValueV2::list(vec!["y".repeat(200)]).unwrap();
        let (value, truncated) = project_observation_item_value(&long_entry);
        assert!(truncated);
        assert_eq!(value, json!(["y".repeat(OBSERVATION_ITEM_VALUE_CHARS_MAX)]));

        // Entries of the maximum admitted length overflow the byte allocation mid-list. The
        // admitted prefix must be maximal: it fits, and appending the next entry would not.
        let wide = RecordedItemValueV2::list(vec!["z".repeat(200); 8]).unwrap();
        let (value, truncated) = project_observation_item_value(&wide);
        let admitted = value.as_array().unwrap();
        assert!(truncated);
        assert!(!admitted.is_empty() && admitted.len() < 8);
        let admitted_strings: Vec<&str> = admitted
            .iter()
            .map(|entry| entry.as_str().unwrap())
            .collect();
        let fits_bytes = serde_json::to_vec(&admitted_strings).unwrap().len();
        let next_entry = "z".repeat(OBSERVATION_ITEM_VALUE_CHARS_MAX);
        let mut next = admitted_strings.clone();
        next.push(next_entry.as_str());
        let with_next = serde_json::to_vec(&next).unwrap().len();
        assert!(fits_bytes <= OBSERVATION_ITEM_VALUE_BYTES_MAX);
        assert!(with_next > OBSERVATION_ITEM_VALUE_BYTES_MAX);
    }

    /// Projects one list value and returns the projected array's serialized byte length.
    ///
    /// VIEWS-001 characterization: the existing maximal-prefix case above proves only that the
    /// admitted prefix fits and the next entry would not. These cases pin the exact boundary
    /// against real serde serialization of the unprojected candidates: a prefix of exactly the
    /// budget is admitted, and a candidate one byte over is refused. Every entry stays within
    /// the per-entry scalar maximum so per-entry truncation cannot distort the measured bytes.
    fn observation_list_bytes(entries: &[String]) -> (Value, bool, usize) {
        let value = RecordedItemValueV2::list(entries.to_vec()).unwrap();
        let (projected, truncated) = project_observation_item_value(&value);
        let bytes = serde_json::to_vec(&projected).unwrap().len();
        (projected, truncated, bytes)
    }

    /// Exact-fit candidates whose JSON array serialization is exactly the observation budget.
    ///
    /// A JSON array charges its brackets, quotes and separators: seven 128-byte entries plus
    /// one 103-byte entry total exactly 1024 bytes. Escaped (`"`) and two-byte (`é`) scalars
    /// each render to 2 serialized bytes, so those variants use 64 scalars per wide entry and
    /// a trailing one-byte scalar in the last entry to cost the same bytes. Each candidate is
    /// verified by oracle before the projection is judged.
    fn observation_exact_fit_candidates() -> Vec<(&'static str, Vec<String>)> {
        let candidates = |wide: String, last: String| {
            let mut entries = vec![wide; 7];
            entries.push(last);
            entries
        };
        vec![
            ("ascii", candidates("a".repeat(128), "a".repeat(103))),
            (
                "multibyte",
                candidates("é".repeat(64), format!("{}a", "é".repeat(51))),
            ),
            (
                "escaped",
                candidates("\"".repeat(64), format!("{}a", "\"".repeat(51))),
            ),
        ]
    }

    #[test]
    fn views001_observation_list_exact_fit_admits_at_the_byte_budget() {
        for (label, entries) in observation_exact_fit_candidates() {
            assert_eq!(
                serde_json::to_vec(&entries).unwrap().len(),
                OBSERVATION_ITEM_VALUE_BYTES_MAX,
                "{label} fixture must serialize to exactly the budget"
            );
            let (projected, truncated, bytes) = observation_list_bytes(&entries);
            assert_eq!(bytes, OBSERVATION_ITEM_VALUE_BYTES_MAX, "{label}");
            assert_eq!(
                projected.as_array().unwrap().len(),
                entries.len(),
                "{label} exact fit admits every entry"
            );
            assert!(!truncated, "{label}");

            // The unprojected candidate is the oracle: the bounded projection can never show
            // an over-budget length itself, so one more admitted-width entry must exceed the
            // budget in raw serialization.
            let mut next = entries;
            next.push(next[0].clone());
            assert!(
                serde_json::to_vec(&next).unwrap().len() > OBSERVATION_ITEM_VALUE_BYTES_MAX,
                "{label} appending one entry must exceed the budget"
            );
        }
    }

    #[test]
    fn views001_observation_list_one_byte_over_the_budget_refuses() {
        // Seven 128-scalar entries plus one 104-scalar entry serialize to exactly 1025 bytes,
        // one over the budget: the last entry is refused, the admitted prefix keeps its
        // order and exact bytes, and the truncation flag reports the cut.
        let mut entries = vec!["z".repeat(128); 7];
        entries.push("z".repeat(104));
        assert_eq!(
            serde_json::to_vec(&entries).unwrap().len(),
            OBSERVATION_ITEM_VALUE_BYTES_MAX + 1
        );
        let (projected, truncated, bytes) = observation_list_bytes(&entries);
        assert_eq!(
            bytes,
            7 * (128 + 2) + 6 + 2,
            "admitted prefix keeps the seven whole entries"
        );
        assert_eq!(projected.as_array().unwrap().len(), 7);
        assert!(truncated);
        let admitted: Vec<&str> = projected
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry.as_str().unwrap())
            .collect();
        assert!(admitted.iter().all(|entry| entry.chars().count() == 128));

        // Re-adding the refused entry reproduces exactly one byte over the budget.
        let mut candidate = admitted.into_iter().map(str::to_owned).collect::<Vec<_>>();
        candidate.push("z".repeat(104));
        assert_eq!(
            serde_json::to_vec(&candidate).unwrap().len(),
            OBSERVATION_ITEM_VALUE_BYTES_MAX + 1
        );
    }

    #[test]
    fn reads003_serialized_object_bytes_match_wrapped_value_serialization() {
        let mut result = Map::new();
        result.insert("text".to_owned(), json!("escape heavy \" \u{5c}\"\\\\ é"));
        result.insert("limit".to_owned(), json!(u64::MAX));
        result.insert(
            "nested".to_owned(),
            json!({"inner": [1, 2, {"deep": null}]}),
        );
        assert_eq!(
            serialized_object_bytes_v1(&result),
            serialized_bytes_v1(&Value::Object(result.clone()))
        );

        // Deep nesting serializes normally on the write path: serde_json's Serializer applies
        // no recursion refusal, so the pinned invariant is that both helpers charge the same
        // real encoded bytes plus the entry overhead, whatever the content shape.
        let mut deep = Value::Null;
        for _ in 0..200 {
            deep = Value::Array(vec![deep]);
        }
        let mut result = Map::new();
        result.insert("deep".to_owned(), deep);
        let encoded = serde_json::to_vec(&Value::Object(result.clone()))
            .unwrap()
            .len();
        assert_eq!(
            serialized_object_bytes_v1(&result),
            encoded.saturating_add(EVIDENCE_ENTRY_OVERHEAD_BYTES)
        );
        assert_eq!(
            serialized_object_bytes_v1(&result),
            serialized_bytes_v1(&Value::Object(result.clone()))
        );
    }

    #[test]
    fn reads003_owned_members_move_rather_than_reserialize() {
        // String heap storage is stable across moves, so pointer identity distinguishes a real
        // move from a to_value copy that would allocate a fresh string.
        let value = json!({"member": "m".repeat(64)});
        let pointer = value["member"].as_str().unwrap().as_ptr();
        let map = into_object(value);
        assert_eq!(map["member"].as_str().unwrap().as_ptr(), pointer);

        let entries = vec![
            json!({"payload": "a".repeat(64)}),
            json!({"payload": "b".repeat(64)}),
        ];
        let pointers: Vec<*const u8> = entries
            .iter()
            .map(|entry| entry["payload"].as_str().unwrap().as_ptr())
            .collect();
        let window = history_window(entries.into_iter(), ok_value, 8).unwrap();
        for (index, pointer) in pointers.iter().enumerate() {
            assert_eq!(
                window["entries"][index]["payload"]
                    .as_str()
                    .unwrap()
                    .as_ptr(),
                *pointer
            );
        }
    }

    /// A legal boundary artifact for the indivisible charge: the media type a/a and a location
    /// of `scalars` characters, whose exact-fit lengths are 796 (path) and 791 (reference).
    fn views004_boundary_artifact(path: bool, scalars: usize) -> RecordedItemValueV2 {
        let digest = podway_core::Sha256Digest::new(format!("sha256:{}", "f".repeat(64))).unwrap();
        let artifact = if path {
            podway_core::ArtifactValueV1::local_path("a".repeat(scalars), digest, u64::MAX, "a/a")
        } else {
            podway_core::ArtifactValueV1::external_reference(
                "a".repeat(scalars),
                digest,
                u64::MAX,
                "a/a",
            )
        }
        .unwrap();
        RecordedItemValueV2::artifact(artifact)
    }

    #[test]
    fn views004_indivisible_charge_matches_the_legacy_value_charger() {
        // The typed charger must reproduce the retained recursive charge over the actual
        // materialized projection, so the indivisible admission policy cannot drift.
        let values = vec![
            RecordedItemValueV2::confirm(),
            RecordedItemValueV2::integer(0),
            RecordedItemValueV2::integer(i64::MAX),
            RecordedItemValueV2::integer(i64::MIN),
            RecordedItemValueV2::choice("c".repeat(120)).unwrap(),
            RecordedItemValueV2::choice("c".to_owned()).unwrap(),
            views004_boundary_artifact(true, 1),
            views004_boundary_artifact(true, 796),
            views004_boundary_artifact(true, 797),
            views004_boundary_artifact(false, 791),
            views004_boundary_artifact(false, 792),
            views004_boundary_artifact(false, 4_000),
        ];
        for value in values {
            assert_eq!(
                indivisible_preview_charge_v1(&value),
                legacy_projected_preview_bytes_v1(&typed_item_value(&value)),
                "typed charge diverged from the legacy charger for {:?}",
                value.item_type()
            );
        }
    }

    #[test]
    fn views004_preview_plan_and_payload_match_legacy_construction() {
        // The plan owner and its payload must reproduce the retained legacy construction for
        // every arm: presence, size, truncation, and the published data itself.
        let check_result = RecordedItemValueV2::check_result(
            podway_core::CheckResultValueV2::new(
                podway_core::OperationId::new("make-test").unwrap(),
                podway_core::Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
                podway_core::CheckResultInputBasisV2::new(
                    "HEAD and dirty-tree snapshot",
                    podway_core::Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap(),
                )
                .unwrap(),
                podway_core::CheckResultExecutorV2::new("gaori", "1.0.0").unwrap(),
                podway_core::CheckResultOutcomeV2::Pass,
                "The external operation reported pass.",
                podway_core::Sha256Digest::new(format!("sha256:{}", "c".repeat(64))).unwrap(),
            )
            .unwrap(),
        );
        // A check result's logical size is its encoded byte count; readback passes exactly
        // `evidence_logical_size_v1` of the admitted value, so the oracle must too.
        let check_result_total = evidence_logical_size_v1(&check_result).0;
        let cases: Vec<(RecordedItemValueV2, u64)> = vec![
            // Text: empty admits nothing; 938 is the untruncated maximum; one more truncates.
            (RecordedItemValueV2::text("").unwrap(), 0),
            (RecordedItemValueV2::text("a").unwrap(), 1),
            (RecordedItemValueV2::text("a".repeat(938)).unwrap(), 938),
            (RecordedItemValueV2::text("a".repeat(939)).unwrap(), 939),
            (
                RecordedItemValueV2::text("\u{1f600}".repeat(939)).unwrap(),
                939,
            ),
            (
                RecordedItemValueV2::text("e\u{0301}".repeat(939)).unwrap(),
                1_878,
            ),
            // Lists: empty admits nothing; the first entry is budget-tested like every other;
            // 936 scalars fit within the budget (8 + 936 * 6 = 5624 of 5628), 937 refuse alone,
            // a later small entry can overflow the remainder, and a whole list can sum to the
            // exact budget (6 entries, 930 scalars: 6 * 8 + 6 * 930 = 5628).
            (RecordedItemValueV2::list(vec![]).unwrap(), 0),
            (RecordedItemValueV2::list(vec!["a".repeat(937)]).unwrap(), 1),
            (RecordedItemValueV2::list(vec!["a".repeat(936)]).unwrap(), 1),
            (
                RecordedItemValueV2::list(vec!["a".repeat(936), "b".to_owned()]).unwrap(),
                2,
            ),
            (
                RecordedItemValueV2::list(vec!["ab".to_owned(); 10]).unwrap(),
                10,
            ),
            (
                RecordedItemValueV2::list(vec![
                    "a".repeat(925),
                    "b".to_owned(),
                    "b".to_owned(),
                    "b".to_owned(),
                    "b".to_owned(),
                    "b".to_owned(),
                ])
                .unwrap(),
                6,
            ),
            // Check results are always present and untruncated, sized by the same encoded byte
            // count readback passes; maximal published coverage stays with v2ast006.
            (check_result, check_result_total),
            // Indivisible values: the declared legal choice c*120 always fits, and the artifact
            // boundary flips presence by exactly one location scalar.
            (RecordedItemValueV2::choice("c".repeat(120)).unwrap(), 1),
            (RecordedItemValueV2::confirm(), 1),
            (RecordedItemValueV2::integer(i64::MAX), 1),
            (views004_boundary_artifact(true, 796), 1),
            (views004_boundary_artifact(true, 797), 1),
            (views004_boundary_artifact(false, 791), 1),
            (views004_boundary_artifact(false, 792), 1),
        ];
        for (value, total) in cases {
            let plan = evidence_preview_plan_v1(&value, total);
            let legacy = legacy_evidence_preview_v1(&value, total);
            match (plan, legacy) {
                (Some(plan), Some(legacy)) => {
                    assert_eq!(plan.size, legacy.size);
                    assert_eq!(plan.truncated, legacy.truncated);
                    assert_eq!(evidence_preview_payload_v1(&value, &plan), legacy.data);
                }
                (None, None) => {}
                (plan, legacy) => panic!(
                    "presence disagrees: plan_present {} legacy_present {}",
                    plan.is_some(),
                    legacy.is_some()
                ),
            }
        }
    }

    /// An in-memory Procedure provider for the readback-mode fixture, mirroring the registered
    /// suite's byte provider.
    struct Views004ByteProcedureV2<'a> {
        path: &'a str,
        source: &'a [u8],
    }

    impl crate::execution::ProcedureProviderV1 for Views004ByteProcedureV2<'_> {
        fn load_workspace_procedure_snapshot_v2(
            &self,
            _workspace: &podway_store::WorkspaceBindingV1,
            procedure: &str,
            snapshot_id: podway_core::ProcedureSnapshotId,
            created_at: podway_core::UnixMillis,
        ) -> Result<
            podway_store::ProcedureSnapshotV2,
            crate::execution::ProcedureV2SourceAdmissionErrorV1,
        > {
            assert_eq!(procedure, self.path);
            crate::execution::workspace_procedure_snapshot_from_bytes_v2(
                procedure,
                self.source,
                snapshot_id,
                created_at,
            )
        }
    }

    fn views004_identity() -> podway_store::DurableWorktreeIdentityV1 {
        let digest = podway_core::Sha256Digest::new(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        podway_store::DurableWorktreeIdentityV1::new(
            digest.clone(),
            podway_core::WorkspaceId::new("00000000-0000-4000-8000-000000005001").unwrap(),
            digest,
        )
    }

    /// One reference per admission case — present, truncated, refused, empty — plus one reference
    /// with no selector, which Store read-back resolves to every valued slot behind the
    /// metadata-only gate.
    fn views004_readback_modes_source() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema": "podway.procedure/v2",
            "id": "views004-readback-modes",
            "version": "1",
            "name": "Readback display modes",
            "purpose": "Exercise per-reference preview admission in both display modes.",
            "node_definitions": {
                "source": {
                    "type": "action",
                    "title": "Record selection",
                    "intent": "Record one value per admission case.",
                    "items": [
                        {"id": "present", "type": "text", "prompt": "Record text.", "required": true, "max_length": 64},
                        {"id": "truncated", "type": "text", "prompt": "Record text.", "required": true, "max_length": 2048},
                        {"id": "refused", "type": "artifact", "prompt": "Record artifact.", "required": true, "allowed_media_types": ["a/a"]},
                        {"id": "empty", "type": "text", "prompt": "Record text.", "required": true, "max_length": 64}
                    ]
                },
                "consume": {
                    "type": "action",
                    "title": "Consume selection",
                    "intent": "Read each case through its own reference."
                }
            },
            "graph": {
                "entry": "source",
                "nodes": [
                    {"id": "source", "use": "source", "next": "consume"},
                    {"id": "consume", "use": "consume", "terminal": true, "evidence_from": [
                        {"node": "source", "required": true, "items": ["present"]},
                        {"node": "source", "required": true, "items": ["truncated"]},
                        {"node": "source", "required": true, "items": ["refused"]},
                        {"node": "source", "required": true, "items": ["empty"]},
                        {"node": "source", "required": true}
                    ]}
                ]
            }
        }))
        .unwrap()
    }

    /// A coherent running session over the readback-modes procedure: one completed source attempt
    /// carrying every case value, and the active consumer attempt holding the five references.
    fn views004_readback_modes_state(source: &[u8]) -> podway_store::GraphSessionStateV2 {
        let path = "views004-readback-modes.json";
        let provider = Views004ByteProcedureV2 { path, source };
        let binding = podway_store::WorkspaceBindingV1::new(
            views004_identity(),
            podway_store::ValidatedWorkspaceRootV1::from_encoded(
                "podway.unix-path/v1:2f776f726b74726565",
            )
            .unwrap(),
        );
        let snapshot = crate::execution::workspace_procedure_snapshot_from_bytes_v2(
            path,
            source,
            podway_core::ProcedureSnapshotId::new("00000000-0000-4000-8000-000000005002").unwrap(),
            podway_core::UnixMillis::new(1_700_000_000_000),
        )
        .unwrap();
        let base = crate::execution::prepare_custom_procedure_v2_start(
            &provider,
            &binding,
            path,
            Some(snapshot.digest()),
            "Project both readback display modes",
            podway_core::SessionId::new("00000000-0000-4000-8000-000000005003").unwrap(),
            podway_core::AttemptId::new("00000000-0000-4000-8000-000000005004").unwrap(),
            podway_core::ProcedureSnapshotId::new("00000000-0000-4000-8000-000000005002").unwrap(),
            podway_core::UnixMillis::new(1_700_000_000_000),
        )
        .unwrap();

        let source_attempt_id =
            podway_core::AttemptId::new("00000000-0000-4000-8000-000000005004").unwrap();
        let consumer_attempt_id =
            podway_core::AttemptId::new("00000000-0000-4000-8000-000000005005").unwrap();
        let created_at = podway_core::UnixMillis::new(1_700_000_000_000);
        let values = base
            .workflow_memory()
            .attempts()
            .first()
            .unwrap()
            .item_slots()
            .iter()
            .map(|slot| {
                let value = match slot.item_id().as_str() {
                    "present" => RecordedItemValueV2::text("p").unwrap(),
                    "truncated" => RecordedItemValueV2::text("a".repeat(1_000)).unwrap(),
                    "refused" => {
                        let artifact = podway_core::ArtifactValueV1::local_path(
                            "a".repeat(797),
                            podway_core::Sha256Digest::new(format!("sha256:{}", "f".repeat(64)))
                                .unwrap(),
                            u64::MAX,
                            "a/a",
                        )
                        .unwrap();
                        RecordedItemValueV2::artifact(artifact)
                    }
                    "empty" => RecordedItemValueV2::text("").unwrap(),
                    other => panic!("unexpected readback-modes fixture item {other}"),
                };
                podway_store::ItemSlotStateV2::new(
                    source_attempt_id.clone(),
                    slot.item_id().clone(),
                    slot.item_type(),
                    podway_core::Revision::new(1),
                    Some(value),
                    slot.created_at(),
                    podway_core::UnixMillis::new(slot.created_at().get() + 1),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let source_memory = podway_store::AttemptWorkflowMemoryV2::new(
            source_attempt_id.clone(),
            values,
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let source_digest = source_memory.recorded_items_digest().unwrap();
        let selected: [&[&str]; 5] = [&["present"], &["truncated"], &["refused"], &["empty"], &[]];
        let references = selected
            .iter()
            .enumerate()
            .map(|(order, ids)| {
                podway_store::EvidenceResolutionStateV2::new(
                    order as u32,
                    true,
                    ids.iter()
                        .map(|id| podway_core::ItemId::new(id.to_string()).unwrap())
                        .collect(),
                    podway_core::ResolvedEvidenceReferenceV2::resolved(
                        podway_core::EvidenceReferenceSnapshotV2::new(
                            podway_core::GraphNodeId::new("source").unwrap(),
                            source_attempt_id.clone(),
                            podway_core::AttemptNumberV2::FIRST,
                            source_digest.clone(),
                            podway_core::UnixMillis::new(1_700_000_000_010),
                        )
                        .unwrap(),
                    ),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let consumer_memory = podway_store::AttemptWorkflowMemoryV2::new(
            consumer_attempt_id.clone(),
            Vec::new(),
            Vec::new(),
            references,
        )
        .unwrap();
        let trace = podway_core::SessionTraceV2::from_parts(
            podway_core::SessionId::new("00000000-0000-4000-8000-000000005003").unwrap(),
            podway_core::SessionLifecycle::Running,
            podway_core::Revision::new(2),
            vec![
                podway_core::SessionAttemptV2::new(
                    source_attempt_id,
                    podway_core::GraphNodeId::new("source").unwrap(),
                    podway_core::AttemptNumberV2::FIRST,
                    podway_core::TraceSequenceV2::FIRST,
                    podway_core::AttemptLifecycle::Completed,
                    podway_core::AttemptValidityV2::Valid,
                    None,
                )
                .unwrap(),
                podway_core::SessionAttemptV2::new(
                    consumer_attempt_id.clone(),
                    podway_core::GraphNodeId::new("consume").unwrap(),
                    podway_core::AttemptNumberV2::FIRST,
                    podway_core::TraceSequenceV2::new(2),
                    podway_core::AttemptLifecycle::Active,
                    podway_core::AttemptValidityV2::Valid,
                    None,
                )
                .unwrap(),
            ],
        )
        .unwrap();
        podway_store::GraphSessionStateV2::new_with_goal_state(
            podway_core::Revision::new(2),
            base.task_title(),
            base.snapshot().clone(),
            trace,
            vec![
                podway_store::GraphNodeCounterV2::new(
                    podway_core::GraphNodeId::new("source").unwrap(),
                    1,
                    0,
                ),
                podway_store::GraphNodeCounterV2::new(
                    podway_core::GraphNodeId::new("consume").unwrap(),
                    1,
                    0,
                ),
            ],
            vec![
                podway_store::AttemptMetadataV2::new(
                    base.workflow_memory()
                        .attempts()
                        .first()
                        .unwrap()
                        .attempt_id()
                        .clone(),
                    created_at,
                    Some(podway_core::UnixMillis::new(1_700_000_000_010)),
                    None,
                )
                .unwrap(),
                podway_store::AttemptMetadataV2::new(
                    consumer_attempt_id,
                    podway_core::UnixMillis::new(1_700_000_000_010),
                    None,
                    None,
                )
                .unwrap(),
            ],
            podway_store::WorkflowMemoryStateV2::new(
                vec![source_memory, consumer_memory],
                Vec::new(),
                Vec::new(),
            )
            .unwrap(),
            base.goal_state().clone(),
            base.created_at(),
            None,
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn views004_readback_value_modes_share_slots_and_stripped_output() {
        let source = views004_readback_modes_source();
        let state = views004_readback_modes_state(&source);
        let view = podway_store::GraphWorkspaceViewV2::new(
            views004_identity(),
            Some(state),
            1,
            podway_core::UnixMillis::new(1_700_000_000_100),
        )
        .with_current_terminal_disposition(false);
        let scope = ReadScope::from_graph(&view);
        let graph = view.graph_state().unwrap();
        let ParsedProcedure::V2(procedure) =
            parse_procedure_document(&source, ProcedureDocumentFormat::Json).unwrap();
        // The real caller must admit this pairing before any readback projection runs.
        let current = CurrentProjection::derive(&scope, graph, &procedure)
            .unwrap()
            .expect("a valid running session derives a current projection");
        let entries = &current.evidence_readback;
        assert_eq!(entries.len(), 5);
        let single = |needle: &str| {
            entries
                .iter()
                .find(|entry| {
                    let items = entry.items().items();
                    items.len() == 1 && items[0].id().as_str() == needle
                })
                .unwrap_or_else(|| panic!("no single-item entry for {needle}"))
        };
        let metadata_only = entries
            .iter()
            .find(|entry| entry.items().items().len() == 4)
            .expect("the omitted selector includes every valued slot");

        for (name, entry, expected_slots, expect_preview, expect_truncated, expected_items) in [
            ("present", single("present"), 1usize, true, false, 1u64),
            ("truncated", single("truncated"), 1, true, true, 1),
            ("refused", single("refused"), 0, false, false, 1),
            ("empty", single("empty"), 0, false, false, 1),
            ("metadata-only", metadata_only, 0, false, false, 4),
        ] {
            for initial in [0usize, 1] {
                let mut publish_remaining = initial;
                let (publish_value, publish_count) = readback_value(
                    graph,
                    entry,
                    &procedure,
                    &mut publish_remaining,
                    EvidencePreviewDisplay::Publish,
                )
                .unwrap();
                let mut suppress_remaining = initial;
                let (suppress_value, suppress_count) = readback_value(
                    graph,
                    entry,
                    &procedure,
                    &mut suppress_remaining,
                    EvidencePreviewDisplay::Suppress,
                )
                .unwrap();
                assert_eq!(publish_count, expected_items, "{name} items at {initial}");
                assert_eq!(suppress_count, expected_items, "{name} items at {initial}");

                // Absolute expected slot consumption per mode, not mere parity.
                let expected_after = initial.saturating_sub(expected_slots);
                assert_eq!(
                    publish_remaining, expected_after,
                    "{name} publish slots at {initial}"
                );
                assert_eq!(
                    suppress_remaining, expected_after,
                    "{name} suppress slots at {initial}"
                );

                let publish_items = publish_value["items"].as_array().cloned().unwrap();
                if expect_preview && initial > 0 {
                    assert_eq!(
                        publish_items
                            .iter()
                            .filter(|item| item.get("preview").is_some())
                            .count(),
                        1,
                        "{name} preview count at {initial}"
                    );
                    let item = publish_items
                        .iter()
                        .find(|item| item.get("preview").is_some())
                        .unwrap();
                    assert_eq!(item["preview_truncated"], json!(expect_truncated));
                    assert_eq!(
                        item.get("next_page_token").is_some(),
                        expect_truncated,
                        "{name} token presence at {initial}"
                    );
                    if name == "truncated" {
                        assert_eq!(item["preview"], json!("a".repeat(938)));
                    }
                } else {
                    for item in &publish_items {
                        assert!(item.get("preview").is_none(), "{name} preview at {initial}");
                        assert!(
                            item.get("preview_truncated").is_none(),
                            "{name} truncation flag at {initial}"
                        );
                        assert!(
                            item.get("next_page_token").is_none(),
                            "{name} token at {initial}"
                        );
                    }
                }

                // Suppressed output equals the published output with its preview members removed.
                let mut stripped = publish_value;
                for item in stripped
                    .get_mut("items")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    if let Some(item) = item.as_object_mut() {
                        item.remove("preview");
                        item.remove("preview_truncated");
                        item.remove("next_page_token");
                    }
                }
                assert_eq!(suppress_value, stripped, "{name} equality at {initial}");
            }
        }
    }

    fn agent004_bootstrap_store() -> (tempfile::TempDir, podway_store::direct_store::DirectStoreV1)
    {
        use std::time::{Duration, Instant};

        let tempdir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tempdir.path()).unwrap();
        let digest = |c: char| {
            podway_core::Sha256Digest::new(format!("sha256:{}", c.to_string().repeat(64))).unwrap()
        };
        let store = podway_store::direct_store::DirectStoreV1::new(
            podway_store::direct_store::DirectWorkspaceLayoutV1::new(
                podway_store::ValidatedWorkspaceRootV1::from_path(&root).unwrap(),
            )
            .unwrap(),
            digest('c'),
            digest('a'),
            digest('b'),
        );
        let request = podway_store::direct_store::DirectMutationRequestV1::new(
            podway_store::IdempotencyKeyV1::new("agent004-bootstrap").unwrap(),
            digest('d'),
            "workspace.init",
            "00000000-0000-4000-8000-000000006000",
            podway_store::direct_store::DirectOutcomeScopeV1::Workspace,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let lease = podway_store::direct_store::ExclusiveMaintenanceLeaseV1::acquire(
            store.layout(),
            deadline,
            true,
        )
        .unwrap();
        store
            .bootstrap(
                &lease,
                podway_core::WorkspaceId::new("00000000-0000-4000-8000-000000005001").unwrap(),
                "agent004-generation",
                &request,
                deadline,
                || UnixMillis::new(1_700_000_000_000),
                |transaction| {
                    transaction.now();
                    Ok(podway_store::CanonicalExecutionJsonV1::new(
                        serde_json::to_string(&json!({
                            "command": "workspace.init",
                            "generated_at": "2023-11-14T22:13:20.000Z",
                            "request_id": "00000000-0000-4000-8000-000000006000",
                            "result": {"initialized": true, "schema": "podway.workspace-init-result/v2"},
                            "schema": "podway.output/v4",
                            "warnings": []
                        }))
                        .unwrap(),
                    )
                    .unwrap())
                },
            )
            .unwrap();
        drop(lease);
        (tempdir, store)
    }

    fn agent004_commit_mutation<F>(
        store: &podway_store::direct_store::DirectStoreV1,
        key: &str,
        request_id: &str,
        session_id: &podway_core::SessionId,
        now: UnixMillis,
        f: F,
    ) -> podway_store::direct_store::DirectMutationResultV1
    where
        F: for<'a, 'b> FnOnce(
            &mut podway_store::direct_store::DirectTransactionV1<'a, 'b, &dyn Fn() -> UnixMillis>,
        ) -> podway_store::direct_store::DirectStoreResultV1<()>,
    {
        use std::time::{Duration, Instant};

        let digest = podway_core::Sha256Digest::new(
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        )
        .unwrap();
        let request = podway_store::direct_store::DirectMutationRequestV1::new(
            podway_store::IdempotencyKeyV1::new(key).unwrap(),
            digest,
            "session.complete",
            request_id,
            podway_store::direct_store::DirectOutcomeScopeV1::Session(session_id.clone()),
        )
        .unwrap();
        let clock = || now;
        let clock_ref: &dyn Fn() -> UnixMillis = &clock;
        store
            .mutate(
                &request,
                Instant::now() + Duration::from_secs(5),
                clock_ref,
                |transaction| {
                    transaction.now();
                    let seq = transaction.next_workspace_sequence()?;
                    f(transaction)?;
                    Ok(podway_store::CanonicalExecutionJsonV1::new(
                        serde_json::to_string(&json!({
                            "command": "session.complete",
                            "generated_at": "2023-11-14T22:13:20.000Z",
                            "request_id": request_id,
                            "result": {"ok": true},
                            "schema": "podway.output/v4",
                            "warnings": [],
                            "workspace": {
                                "latest_workspace_sequence": seq,
                                "root": "/worktree",
                                "uuid": "00000000-0000-4000-8000-000000005001"
                            }
                        }))
                        .unwrap(),
                    )
                    .unwrap())
                },
            )
            .unwrap()
    }

    #[test]
    fn agent004_successor_context_matches_direct_observation_across_lifecycle_and_pre_write_transactions()
     {
        use std::time::{Duration, Instant};

        let (_tempdir, store) = agent004_bootstrap_store();
        let source = serde_json::to_vec(&json!({
            "schema": "podway.procedure/v2",
            "id": "agent004-lifecycle",
            "version": "1",
            "name": "AGENT-004 lifecycle observation test",
            "purpose": "Verify in-transaction successor observation across lifecycle states.",
            "goal_tracking": true,
            "node_definitions": {
                "draft": {
                    "type": "action",
                    "title": "Draft",
                    "intent": "Record initial summary.",
                    "items": [
                        {"id": "summary", "type": "text", "prompt": "Summary.", "required": true}
                    ]
                },
                "review": {
                    "type": "decision",
                    "title": "Review goal",
                    "objective": "Assess goal and choose route.",
                    "prompt": "Proceed or rework.",
                    "assessment": {
                        "target": "session_goal",
                        "outcomes": {
                            "proceed": "achieved",
                            "rework": "not_achieved",
                            "superseded": "superseded"
                        }
                    },
                    "options": [
                        {
                            "id": "proceed",
                            "label": "Proceed to finalize"
                        },
                        {
                            "id": "rework",
                            "label": "Rework draft"
                        },
                        {
                            "id": "superseded",
                            "label": "Rework after goal revision"
                        }
                    ],
                    "reason": {"required": true}
                },
                "finalize": {
                    "type": "action",
                    "title": "Finalize",
                    "intent": "Record final note.",
                    "items": [
                        {"id": "final-note", "type": "text", "prompt": "Final note.", "required": true}
                    ]
                }
            },
            "graph": {
                "entry": "draft",
                "nodes": [
                    {"id": "draft", "use": "draft", "next": "review"},
                    {
                        "id": "review",
                        "use": "review",
                        "routes": {
                            "proceed": {"to": "finalize", "effect": "advance"},
                            "rework": {"to": "draft", "effect": "rework"},
                            "superseded": {"to": "draft", "effect": "rework"}
                        }
                    },
                    {"id": "finalize", "use": "finalize", "terminal": true}
                ]
            }
        }))
        .unwrap();

        let snapshot = crate::execution::workspace_procedure_snapshot_from_bytes_v2(
            "agent004-lifecycle.json",
            &source,
            podway_core::ProcedureSnapshotId::new("00000000-0000-4000-8000-000000006001").unwrap(),
            UnixMillis::new(1_700_000_000_000),
        )
        .unwrap();
        let session_id =
            podway_core::SessionId::new("00000000-0000-4000-8000-000000006002").unwrap();
        let prepared = GraphSessionStateV2::prepared(
            podway_core::Revision::new(1),
            "AGENT-004 lifecycle task",
            snapshot.clone(),
            session_id.clone(),
            UnixMillis::new(1_700_000_000_000),
        )
        .unwrap();

        // 1. Prepared state: verify nested store.read fails with ConnectionBusy inside the write
        //    callback while transaction-based successor projection succeeds before & after write.
        let mut prepared_obs = Map::new();
        let mut prepared_seq = 0u64;
        let m1 = agent004_commit_mutation(
            &store,
            "agent004-k1",
            "00000000-0000-4000-8000-000000006011",
            &session_id,
            UnixMillis::new(1_700_000_000_010),
            |tx| {
                let nested_read = store
                    .read(Instant::now() + Duration::from_millis(1), |snapshot| {
                        snapshot.view(UnixMillis::new(1_700_000_000_010))
                    });
                assert!(
                    matches!(
                        nested_read,
                        Err(podway_store::direct_store::DirectStoreErrorV1::ConnectionBusy)
                    ),
                    "nested store.read inside a write callback must return ConnectionBusy, got {nested_read:?}"
                );

                let ctx_before = SuccessorObservationContextV1::from_transaction(tx, &prepared)?;
                let obs_before = project_successor_observation_v1(&ctx_before).unwrap();
                let seq_before = ctx_before.workspace_sequence();
                tx.create_graph(&prepared)?;
                let ctx_after = SuccessorObservationContextV1::from_transaction(tx, &prepared)?;
                let obs_after = project_successor_observation_v1(&ctx_after).unwrap();
                assert_eq!(obs_before, obs_after);
                assert_eq!(seq_before, 2);
                assert_eq!(ctx_after.workspace_sequence(), 2);
                assert_eq!(obs_before["schema"], json!("podway.observation-result/v6"));
                assert_eq!(
                    obs_before["guidance"]["schema"],
                    json!("podway.prepared-next-result/v3")
                );
                prepared_obs = obs_before;
                prepared_seq = seq_before;
                Ok(())
            },
        );
        assert_eq!(prepared_seq, m1.outcome().sequence());
        let committed_prepared = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_015))?;
                assert_eq!(view.sequence(), m1.outcome().sequence());
                assert_eq!(view.sequence(), prepared_seq);
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(prepared_obs, committed_prepared);

        // 2. Running state: begin_v2 with initial goal, then mutate_active_items_v2.
        let attempt_1 =
            podway_core::AttemptId::new("00000000-0000-4000-8000-000000006003").unwrap();
        let initial_goal = podway_core::GraphInitialGoalV2::new(
            podway_core::GoalStatementV2::new("Complete lifecycle with coherent observation.")
                .unwrap(),
            podway_core::GoalDefinitionV2::new(vec![
                podway_core::GoalCriterionV2::new(
                    podway_core::CriterionId::new("c1").unwrap(),
                    "Draft summary recorded.",
                )
                .unwrap(),
                podway_core::GoalCriterionV2::new(
                    podway_core::CriterionId::new("c2").unwrap(),
                    "Review completed.",
                )
                .unwrap(),
            ])
            .unwrap(),
            None,
        );
        let begun = prepared
            .begin_v2(
                podway_core::Revision::ZERO,
                attempt_1.clone(),
                Some(initial_goal),
                UnixMillis::new(1_700_000_000_020),
            )
            .unwrap()
            .into_state();
        let mutated = begun
            .mutate_active_items_v2(
                &attempt_1,
                &[podway_core::ActiveItemMutationRequestV2::new(
                    podway_core::ItemId::new("summary").unwrap(),
                    podway_core::Revision::ZERO,
                    podway_core::ActiveItemMutationV2::Set {
                        value: "Draft summary ready.".to_owned(),
                    },
                )],
                UnixMillis::new(1_700_000_000_025),
            )
            .unwrap()
            .into_state();

        let mut begun_obs = Map::new();
        let mut begun_seq = 0u64;
        let m2a = agent004_commit_mutation(
            &store,
            "agent004-k2a",
            "00000000-0000-4000-8000-000000006012",
            &session_id,
            UnixMillis::new(1_700_000_000_020),
            |tx| {
                // While the DB still holds `prepared`, projecting `begun` must reflect the running
                // attempt and defined goal, never the stale prepared row.
                let ctx_before = SuccessorObservationContextV1::from_transaction(tx, &begun)?;
                let obs_before = project_successor_observation_v1(&ctx_before).unwrap();
                tx.replace_graph(
                    prepared.workspace_revision(),
                    prepared.trace().revision(),
                    &begun,
                )?;
                let ctx_after = SuccessorObservationContextV1::from_transaction(tx, &begun)?;
                let obs_after = project_successor_observation_v1(&ctx_after).unwrap();
                assert_eq!(obs_before, obs_after);
                assert_eq!(
                    obs_before["status"]["session"]["lifecycle"],
                    json!("running")
                );
                assert_eq!(obs_before["status"]["goal_defined"], json!(true));
                begun_obs = obs_before;
                begun_seq = ctx_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(begun_seq, m2a.outcome().sequence());
        let committed_begun = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_021))?;
                assert_eq!(view.sequence(), m2a.outcome().sequence());
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(begun_obs, committed_begun);

        let mut running_obs = Map::new();
        let mut running_seq = 0u64;
        let m2b = agent004_commit_mutation(
            &store,
            "agent004-k2b",
            "00000000-0000-4000-8000-000000006013",
            &session_id,
            UnixMillis::new(1_700_000_000_025),
            |tx| {
                let ctx_before = SuccessorObservationContextV1::from_transaction(tx, &mutated)?;
                let obs_before = project_successor_observation_v1(&ctx_before).unwrap();
                tx.replace_graph(
                    begun.workspace_revision(),
                    begun.trace().revision(),
                    &mutated,
                )?;
                let ctx_after = SuccessorObservationContextV1::from_transaction(tx, &mutated)?;
                let obs_after = project_successor_observation_v1(&ctx_after).unwrap();
                assert_eq!(obs_before, obs_after);
                assert_eq!(
                    obs_before["status"]["session"]["lifecycle"],
                    json!("running")
                );
                assert_eq!(
                    obs_before["guidance"]["schema"],
                    json!("podway.next-result/v5")
                );
                assert_eq!(
                    obs_before["status"]["current"]["attempt"]["attempt_id"],
                    json!(attempt_1)
                );
                assert_eq!(obs_before["status"]["goal_defined"], json!(true));
                running_obs = obs_before;
                running_seq = ctx_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(running_seq, m2b.outcome().sequence());
        let committed_running = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_026))?;
                assert_eq!(view.sequence(), m2b.outcome().sequence());
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(running_obs, committed_running);

        // 3. Changed-cursor & batch goal assessment: complete draft -> review, assess c1 & c2, decide proceed -> finalize.
        let attempt_2 =
            podway_core::AttemptId::new("00000000-0000-4000-8000-000000006004").unwrap();
        let at_review = mutated
            .complete_active_action_v2(
                mutated.trace().revision(),
                &attempt_1,
                Some(attempt_2.clone()),
                UnixMillis::new(1_700_000_000_030),
            )
            .unwrap()
            .into_state();
        let assessed = at_review
            .assess_goal_criteria_v2(
                at_review.trace().revision(),
                &attempt_2,
                podway_core::GoalRevisionNumberV2::FIRST,
                vec![
                    podway_core::CriterionAssessmentBatchEntryV2::new(
                        podway_core::CriterionId::new("c1").unwrap(),
                        "satisfied".to_owned(),
                        "Summary verified.".to_owned(),
                        Vec::new(),
                    ),
                    podway_core::CriterionAssessmentBatchEntryV2::new(
                        podway_core::CriterionId::new("c2").unwrap(),
                        "satisfied".to_owned(),
                        "Review verified.".to_owned(),
                        Vec::new(),
                    ),
                ],
                None,
                UnixMillis::new(1_700_000_000_035),
            )
            .unwrap()
            .into_state();
        let attempt_3 =
            podway_core::AttemptId::new("00000000-0000-4000-8000-000000006005").unwrap();
        let at_finalize = assessed
            .decide_active_route_with_goal_revision_v2(
                assessed.trace().revision(),
                &attempt_2,
                podway_core::OptionId::new("proceed").unwrap(),
                attempt_3.clone(),
                Some(podway_core::GoalRevisionNumberV2::FIRST),
                Some(podway_core::ReasonV2::new("All criteria met.").unwrap()),
                None,
                UnixMillis::new(1_700_000_000_040),
            )
            .unwrap()
            .into_state();

        let mut review_obs = Map::new();
        let mut review_seq = 0u64;
        let m3a = agent004_commit_mutation(
            &store,
            "agent004-k3a",
            "00000000-0000-4000-8000-000000006014",
            &session_id,
            UnixMillis::new(1_700_000_000_030),
            |tx| {
                let ctx_before = SuccessorObservationContextV1::from_transaction(tx, &at_review)?;
                let obs_before = project_successor_observation_v1(&ctx_before).unwrap();
                tx.replace_graph(
                    mutated.workspace_revision(),
                    mutated.trace().revision(),
                    &at_review,
                )?;
                let ctx_after = SuccessorObservationContextV1::from_transaction(tx, &at_review)?;
                let obs_after = project_successor_observation_v1(&ctx_after).unwrap();
                assert_eq!(obs_before, obs_after);
                assert_eq!(
                    obs_before["status"]["current"]["node"]["graph_node_id"],
                    json!("review")
                );
                review_obs = obs_before;
                review_seq = ctx_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(review_seq, m3a.outcome().sequence());
        let committed_review = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_031))?;
                assert_eq!(view.sequence(), m3a.outcome().sequence());
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(review_obs, committed_review);

        let mut assessed_obs = Map::new();
        let mut assessed_seq = 0u64;
        let m3b = agent004_commit_mutation(
            &store,
            "agent004-k3b",
            "00000000-0000-4000-8000-000000006015",
            &session_id,
            UnixMillis::new(1_700_000_000_035),
            |tx| {
                let ctx_before = SuccessorObservationContextV1::from_transaction(tx, &assessed)?;
                let obs_before = project_successor_observation_v1(&ctx_before).unwrap();
                tx.replace_graph(
                    at_review.workspace_revision(),
                    at_review.trace().revision(),
                    &assessed,
                )?;
                let ctx_after = SuccessorObservationContextV1::from_transaction(tx, &assessed)?;
                let obs_after = project_successor_observation_v1(&ctx_after).unwrap();
                assert_eq!(obs_before, obs_after);
                assert_eq!(
                    obs_before["guidance"]["readiness"]["goal_ready"],
                    json!(true)
                );
                assessed_obs = obs_before;
                assessed_seq = ctx_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(assessed_seq, m3b.outcome().sequence());
        let committed_assessed = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_036))?;
                assert_eq!(view.sequence(), m3b.outcome().sequence());
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(assessed_obs, committed_assessed);

        let mut finalize_obs = Map::new();
        let mut finalize_seq = 0u64;
        let m3c = agent004_commit_mutation(
            &store,
            "agent004-k3c",
            "00000000-0000-4000-8000-000000006016",
            &session_id,
            UnixMillis::new(1_700_000_000_040),
            |tx| {
                let ctx_before = SuccessorObservationContextV1::from_transaction(tx, &at_finalize)?;
                let obs_before = project_successor_observation_v1(&ctx_before).unwrap();
                tx.replace_graph(
                    assessed.workspace_revision(),
                    assessed.trace().revision(),
                    &at_finalize,
                )?;
                let ctx_after = SuccessorObservationContextV1::from_transaction(tx, &at_finalize)?;
                let obs_after = project_successor_observation_v1(&ctx_after).unwrap();
                assert_eq!(obs_before, obs_after);
                assert_eq!(
                    obs_before["status"]["current"]["node"]["graph_node_id"],
                    json!("finalize")
                );
                assert_eq!(
                    obs_before["status"]["current"]["attempt"]["attempt_id"],
                    json!(attempt_3)
                );
                finalize_obs = obs_before;
                finalize_seq = ctx_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(finalize_seq, m3c.outcome().sequence());
        let committed_finalize = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_041))?;
                assert_eq!(view.sequence(), m3c.outcome().sequence());
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(finalize_obs, committed_finalize);

        // 4. Terminal state: set final-note, then complete finalize -> Completed, guidance is null, undisposed templates.
        let finalize_ready = at_finalize
            .mutate_active_item_v2(
                &attempt_3,
                &podway_core::ItemId::new("final-note").unwrap(),
                podway_core::Revision::ZERO,
                podway_core::ActiveItemMutationV2::Set {
                    value: "Done.".to_owned(),
                },
                UnixMillis::new(1_700_000_000_045),
            )
            .unwrap()
            .into_state();
        let completed = finalize_ready
            .complete_active_action_v2(
                finalize_ready.trace().revision(),
                &attempt_3,
                None,
                UnixMillis::new(1_700_000_000_050),
            )
            .unwrap()
            .into_state();

        let mut finalize_ready_seq = 0u64;
        let m4a = agent004_commit_mutation(
            &store,
            "agent004-k4a",
            "00000000-0000-4000-8000-000000006017",
            &session_id,
            UnixMillis::new(1_700_000_000_045),
            |tx| {
                let ctx_before =
                    SuccessorObservationContextV1::from_transaction(tx, &finalize_ready)?;
                let obs_before = project_successor_observation_v1(&ctx_before).unwrap();
                tx.replace_graph(
                    at_finalize.workspace_revision(),
                    at_finalize.trace().revision(),
                    &finalize_ready,
                )?;
                let ctx_after =
                    SuccessorObservationContextV1::from_transaction(tx, &finalize_ready)?;
                assert_eq!(
                    obs_before,
                    project_successor_observation_v1(&ctx_after).unwrap()
                );
                finalize_ready_seq = ctx_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(finalize_ready_seq, m4a.outcome().sequence());

        let mut completed_obs = Map::new();
        let mut completed_seq = 0u64;
        let m4b = agent004_commit_mutation(
            &store,
            "agent004-k4b",
            "00000000-0000-4000-8000-000000006018",
            &session_id,
            UnixMillis::new(1_700_000_000_050),
            |tx| {
                let ctx_before = SuccessorObservationContextV1::from_transaction(tx, &completed)?;
                let obs_before = project_successor_observation_v1(&ctx_before).unwrap();
                tx.replace_graph(
                    finalize_ready.workspace_revision(),
                    finalize_ready.trace().revision(),
                    &completed,
                )?;
                let ctx_after = SuccessorObservationContextV1::from_transaction(tx, &completed)?;
                let obs_after = project_successor_observation_v1(&ctx_after).unwrap();
                assert_eq!(obs_before, obs_after);
                assert_eq!(
                    obs_before["status"]["session"]["lifecycle"],
                    json!("completed")
                );
                assert_eq!(obs_before["guidance"], Value::Null);
                let commands: Vec<&str> = obs_before["mutation_templates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| entry["command"].as_str().unwrap())
                    .collect();
                assert_eq!(
                    commands,
                    vec![
                        "session.terminal_disposition",
                        "session.terminal_disposition"
                    ]
                );
                completed_obs = obs_before;
                completed_seq = ctx_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(completed_seq, m4b.outcome().sequence());
        let committed_completed = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_051))?;
                assert_eq!(view.sequence(), m4b.outcome().sequence());
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(completed_obs, committed_completed);

        // 5. Replacing an existing session via archive_graph + create_graph within one transaction.
        let snapshot_2 = crate::execution::workspace_procedure_snapshot_from_bytes_v2(
            "agent004-lifecycle.json",
            &source,
            podway_core::ProcedureSnapshotId::new("00000000-0000-4000-8000-000000006020").unwrap(),
            UnixMillis::new(1_700_000_000_060),
        )
        .unwrap();
        let session_2 =
            podway_core::SessionId::new("00000000-0000-4000-8000-000000006006").unwrap();
        let prepared_2 = GraphSessionStateV2::prepared(
            completed.workspace_revision().checked_next().unwrap(),
            "Replacement prepared session",
            snapshot_2,
            session_2.clone(),
            UnixMillis::new(1_700_000_000_060),
        )
        .unwrap();
        let mut replacement_obs = Map::new();
        let mut replacement_seq = 0u64;
        let m5 = agent004_commit_mutation(
            &store,
            "agent004-k5",
            "00000000-0000-4000-8000-000000006019",
            &session_2,
            UnixMillis::new(1_700_000_000_060),
            |tx| {
                let ctx_before = SuccessorObservationContextV1::from_transaction(tx, &prepared_2)?;
                assert!(!ctx_before.read_scope().current_terminal_disposition());
                let obs_before = project_successor_observation_v1(&ctx_before).unwrap();
                let disposition = podway_core::TerminalDispositionV2::not_required(
                    completed.trace().session_id().clone(),
                    completed.trace().revision(),
                    "Completed in test.",
                    None,
                    UnixMillis::new(1_700_000_000_060),
                )
                .unwrap();
                tx.record_disposition(&disposition)?;
                assert_eq!(tx.terminal_dispositions()?.len(), 1);
                let ctx_fresh_undisposed =
                    SuccessorObservationContextV1::from_transaction(tx, &prepared_2)?;
                assert!(
                    !ctx_fresh_undisposed
                        .read_scope()
                        .current_terminal_disposition()
                );

                tx.archive_graph(completed.workspace_revision(), completed.trace().revision())?;
                tx.create_graph(&prepared_2)?;
                assert!(tx.terminal_dispositions()?.is_empty());
                let ctx_after = SuccessorObservationContextV1::from_transaction(tx, &prepared_2)?;
                let obs_after = project_successor_observation_v1(&ctx_after).unwrap();
                assert_eq!(obs_before, obs_after);
                replacement_obs = obs_before;
                replacement_seq = ctx_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(replacement_seq, m5.outcome().sequence());
        let committed_replacement = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_061))?;
                assert_eq!(view.sequence(), m5.outcome().sequence());
                assert!(view.terminal_dispositions().is_empty());
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(replacement_obs, committed_replacement);
    }

    #[test]
    fn agent004_successor_context_reflects_delegation_readiness_for_in_memory_successor() {
        use std::time::{Duration, Instant};

        let (_tempdir, store) = agent004_bootstrap_store();
        let role_digest = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
        let source = serde_json::to_vec(&json!({
            "schema": "podway.procedure/v2",
            "id": "agent004-delegation",
            "version": "1",
            "name": "AGENT-004 delegation readiness test",
            "purpose": "Verify delegation_receipts_for_graph against in-memory successor states.",
            "node_definitions": {
                "delegate-step": {
                    "type": "action",
                    "title": "Delegated step",
                    "intent": "Run delegated work.",
                    "items": [
                        {"id": "note", "type": "text", "prompt": "Delegated note.", "required": true}
                    ]
                },
                "direct-step": {
                    "type": "action",
                    "title": "Direct step",
                    "intent": "Finish directly."
                }
            },
            "graph": {
                "entry": "delegate-step",
                "nodes": [
                    {
                        "id": "delegate-step",
                        "use": "delegate-step",
                        "next": "direct-step",
                        "delegation": {
                            "binding_ref": "worker-1",
                            "role_ref": "reviewer",
                            "role_snapshot_digest": role_digest,
                            "max_rounds": 2,
                            "after_accept": "retain"
                        }
                    },
                    {"id": "direct-step", "use": "direct-step", "terminal": true}
                ]
            }
        }))
        .unwrap();

        let snapshot = crate::execution::workspace_procedure_snapshot_from_bytes_v2(
            "agent004-delegation.json",
            &source,
            podway_core::ProcedureSnapshotId::new("00000000-0000-4000-8000-000000006101").unwrap(),
            UnixMillis::new(1_700_000_000_000),
        )
        .unwrap();
        let session_id =
            podway_core::SessionId::new("00000000-0000-4000-8000-000000006102").unwrap();
        let attempt_1 =
            podway_core::AttemptId::new("00000000-0000-4000-8000-000000006103").unwrap();
        let prepared = GraphSessionStateV2::prepared(
            podway_core::Revision::new(1),
            "AGENT-004 delegation task",
            snapshot.clone(),
            session_id.clone(),
            UnixMillis::new(1_700_000_000_000),
        )
        .unwrap();
        let begun = prepared
            .begin_v2(
                podway_core::Revision::ZERO,
                attempt_1.clone(),
                None,
                UnixMillis::new(1_700_000_000_010),
            )
            .unwrap()
            .into_state();
        let with_note = begun
            .mutate_active_items_v2(
                &attempt_1,
                &[podway_core::ActiveItemMutationRequestV2::new(
                    podway_core::ItemId::new("note").unwrap(),
                    podway_core::Revision::ZERO,
                    podway_core::ActiveItemMutationV2::Set {
                        value: "Initial accepted note.".to_owned(),
                    },
                )],
                UnixMillis::new(1_700_000_000_020),
            )
            .unwrap()
            .into_state();

        // Persist `prepared`, `begun`, then `with_note` (1 revision step at a time) and verify ReceiptMissing.
        let mut prep_del_seq = 0u64;
        let m_prep = agent004_commit_mutation(
            &store,
            "agent004-del-k0a",
            "00000000-0000-4000-8000-000000006109",
            &session_id,
            UnixMillis::new(1_700_000_000_005),
            |tx| {
                tx.create_graph(&prepared)?;
                let ctx = SuccessorObservationContextV1::from_transaction(tx, &prepared)?;
                prep_del_seq = ctx.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(prep_del_seq, m_prep.outcome().sequence());

        let mut begun_seq = 0u64;
        let m_begun = agent004_commit_mutation(
            &store,
            "agent004-del-k0b",
            "00000000-0000-4000-8000-000000006110",
            &session_id,
            UnixMillis::new(1_700_000_000_010),
            |tx| {
                tx.replace_graph(
                    prepared.workspace_revision(),
                    prepared.trace().revision(),
                    &begun,
                )?;
                let ctx = SuccessorObservationContextV1::from_transaction(tx, &begun)?;
                begun_seq = ctx.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(begun_seq, m_begun.outcome().sequence());

        let mut with_note_seq = 0u64;
        let m_note = agent004_commit_mutation(
            &store,
            "agent004-del-k1",
            "00000000-0000-4000-8000-000000006111",
            &session_id,
            UnixMillis::new(1_700_000_000_020),
            |tx| {
                let ctx_before = SuccessorObservationContextV1::from_transaction(tx, &with_note)?;
                let obs_before = project_successor_observation_v1(&ctx_before).unwrap();
                assert_eq!(
                    obs_before["status"]["delegation"]["state"],
                    json!("awaiting_acceptance")
                );
                assert_eq!(
                    obs_before["status"]["delegation"]["reason"],
                    json!("receipt_required")
                );
                tx.replace_graph(
                    begun.workspace_revision(),
                    begun.trace().revision(),
                    &with_note,
                )?;
                let ctx_after = SuccessorObservationContextV1::from_transaction(tx, &with_note)?;
                assert_eq!(
                    obs_before,
                    project_successor_observation_v1(&ctx_after).unwrap()
                );
                with_note_seq = ctx_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(with_note_seq, m_note.outcome().sequence());

        // Accept delegation receipt and verify ReceiptCurrent.
        let view = GraphWorkspaceViewV2::new(
            views004_identity(),
            Some(with_note.clone()),
            with_note_seq,
            UnixMillis::new(1_700_000_000_025),
        );
        let package = delegation_work_package_from_view_v1(&view).unwrap();
        let accepted_state = with_note
            .accept_delegation_successor_v1(&attempt_1)
            .unwrap();
        let items_digest =
            podway_store::v2_delegation::accepted_items_digest_v1(&with_note).unwrap();
        let cfg_digest = "sha256:3333333333333333333333333333333333333333333333333333333333333333";
        let task_digest = "sha256:4444444444444444444444444444444444444444444444444444444444444444";
        let receipt = DelegationReceiptV1::new(json!({
            "schema": "podway.delegation-receipt/v1",
            "receipt_id": "00000000-0000-4000-8000-000000006104",
            "workspace_uuid": "00000000-0000-4000-8000-000000005001",
            "procedure_digest": with_note.snapshot().digest(),
            "session_id": session_id,
            "placement_id": "delegate-step",
            "attempt_id": attempt_1,
            "delegation": package.package()["delegation"],
            "package_digest": package.digest(),
            "execution": {
                "binding_ref": "worker-1",
                "adapter_id": "herdr",
                "adapter_version": "1.0.0",
                "primary_host": "managed",
                "scope_kind": "task",
                "scope_ref": "scope-1",
                "supervisor_ref": "sup-1",
                "worker_ref": "w-1",
                "task_ref": "task-1",
                "role_ref": "reviewer",
                "role_snapshot_digest": role_digest,
                "agent_configuration_digest": cfg_digest,
                "accepted_task_digest": task_digest
            },
            "rounds_used": 1,
            "result_digest": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
            "summary": "Accepted delegated step.",
            "worker_disposition": "idle",
            "accepted": true,
            "accepted_items_digest": items_digest,
            "accepted_at": "2023-11-14T22:13:20.030Z"
        }))
        .unwrap();

        let mut accepted_obs = Map::new();
        let mut accepted_seq = 0u64;
        let m_accept = agent004_commit_mutation(
            &store,
            "agent004-del-k2",
            "00000000-0000-4000-8000-000000006112",
            &session_id,
            UnixMillis::new(1_700_000_000_030),
            |tx| {
                tx.accept_delegation(
                    with_note.workspace_revision(),
                    with_note.trace().revision(),
                    &accepted_state,
                    package.digest(),
                    &receipt,
                )?;
                let ctx_accepted =
                    SuccessorObservationContextV1::from_transaction(tx, &accepted_state)?;
                let obs = project_successor_observation_v1(&ctx_accepted).unwrap();
                assert_eq!(obs["status"]["delegation"]["state"], json!("accepted"));
                assert_eq!(
                    obs["status"]["delegation"]["reason"],
                    json!("current_receipt")
                );
                assert_eq!(ctx_accepted.read_scope().receipts().total(), 1);
                assert_eq!(
                    ctx_accepted.read_scope().receipts().summaries(),
                    vec![receipt.summary()]
                );
                accepted_obs = obs;
                accepted_seq = ctx_accepted.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(accepted_seq, m_accept.outcome().sequence());
        let committed_accepted = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_031))?;
                assert_eq!(view.sequence(), m_accept.outcome().sequence());
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(accepted_obs, committed_accepted);

        // (b.1) Successful no-op record-many on delegated attempt: item/session revisions remain
        //       unchanged (`!noop_outcome.changed()`), workspace sequence increases, and
        //       `ReceiptCurrent` (`accepted` / `current_receipt`) is preserved.
        let noop_outcome = accepted_state
            .mutate_active_items_v2(
                &attempt_1,
                &[podway_core::ActiveItemMutationRequestV2::new(
                    podway_core::ItemId::new("note").unwrap(),
                    podway_core::Revision::new(1),
                    podway_core::ActiveItemMutationV2::Set {
                        value: "Initial accepted note.".to_owned(),
                    },
                )],
                UnixMillis::new(1_700_000_000_035),
            )
            .unwrap();
        assert!(!noop_outcome.changed());
        let noop_state = noop_outcome.into_state();
        assert_eq!(
            noop_state.trace().revision(),
            accepted_state.trace().revision()
        );
        assert_eq!(
            noop_state.workflow_memory().attempts()[0].item_slots()[0].revision(),
            podway_core::Revision::new(1)
        );
        let mut noop_obs = Map::new();
        let mut noop_seq = 0u64;
        let m_noop = agent004_commit_mutation(
            &store,
            "agent004-del-noop",
            "00000000-0000-4000-8000-000000006113",
            &session_id,
            UnixMillis::new(1_700_000_000_035),
            |tx| {
                let ctx_noop = SuccessorObservationContextV1::from_transaction(tx, &noop_state)?;
                let obs = project_successor_observation_v1(&ctx_noop).unwrap();
                assert_eq!(
                    ctx_noop.read_scope().receipts().readiness(),
                    DelegationReadinessV1::ReceiptCurrent
                );
                assert_eq!(obs["status"]["delegation"]["state"], json!("accepted"));
                assert_eq!(
                    obs["status"]["delegation"]["reason"],
                    json!("current_receipt")
                );
                assert_eq!(
                    obs["status"]["session"]["revision"],
                    json!(accepted_state.trace().revision().get())
                );
                assert_eq!(obs["status"]["items"][0]["revision"], json!(1));
                noop_obs = obs;
                noop_seq = ctx_noop.workspace_sequence();
                Ok(())
            },
        );
        assert!(noop_seq > accepted_seq);
        assert_eq!(noop_seq, m_noop.outcome().sequence());
        let committed_noop = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_036))?;
                assert_eq!(view.sequence(), m_noop.outcome().sequence());
                assert_eq!(view.sequence(), noop_seq);
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(noop_obs, committed_noop);

        // (b.2) Changed record-many on delegated attempt (`edited_after_accept`) vs cursor
        //       transition (`advanced_to_direct`): verify pre-write evaluation of both against
        //       persisted `accepted_state`, then persist `advanced_to_direct` and `completed_delegated`
        //       to verify same-session receipt summary/total retention and fresh-session exclusion.
        let changed_outcome = accepted_state
            .mutate_active_items_v2(
                &attempt_1,
                &[podway_core::ActiveItemMutationRequestV2::new(
                    podway_core::ItemId::new("note").unwrap(),
                    podway_core::Revision::new(1),
                    podway_core::ActiveItemMutationV2::Set {
                        value: "Edited after acceptance.".to_owned(),
                    },
                )],
                UnixMillis::new(1_700_000_000_040),
            )
            .unwrap();
        assert!(changed_outcome.changed());
        let edited_after_accept = changed_outcome.into_state();
        assert_eq!(
            edited_after_accept.trace().revision(),
            accepted_state.trace().revision().checked_next().unwrap()
        );
        assert_eq!(
            edited_after_accept.workflow_memory().attempts()[0].item_slots()[0].revision(),
            podway_core::Revision::new(2)
        );
        let attempt_2 =
            podway_core::AttemptId::new("00000000-0000-4000-8000-000000006105").unwrap();
        let advanced_to_direct = accepted_state
            .complete_active_action_v2(
                accepted_state.trace().revision(),
                &attempt_1,
                Some(attempt_2.clone()),
                UnixMillis::new(1_700_000_000_040),
            )
            .unwrap()
            .into_state();

        let mut direct_obs = Map::new();
        let mut direct_seq = 0u64;
        let m_direct = agent004_commit_mutation(
            &store,
            "agent004-del-k3",
            "00000000-0000-4000-8000-000000006114",
            &session_id,
            UnixMillis::new(1_700_000_000_040),
            |tx| {
                // The persisted graph in SQLite is still `accepted_state` (`ReceiptCurrent`).
                assert_eq!(
                    tx.delegation_receipts()?.readiness(),
                    DelegationReadinessV1::ReceiptCurrent
                );
                // Evaluating against `edited_after_accept` before `replace_graph` reports
                // `ItemsChanged` (`stale_acceptance` / `items_changed`) while retaining the
                // same-session receipt summary and total.
                let ctx_edited =
                    SuccessorObservationContextV1::from_transaction(tx, &edited_after_accept)?;
                assert_eq!(
                    ctx_edited.read_scope().receipts().readiness(),
                    DelegationReadinessV1::ItemsChanged
                );
                assert_eq!(ctx_edited.read_scope().receipts().total(), 1);
                assert_eq!(
                    ctx_edited.read_scope().receipts().summaries(),
                    vec![receipt.summary()]
                );
                let obs_edited = project_successor_observation_v1(&ctx_edited).unwrap();
                assert_eq!(
                    obs_edited["status"]["delegation"]["state"],
                    json!("stale_acceptance")
                );
                assert_eq!(
                    obs_edited["status"]["delegation"]["reason"],
                    json!("items_changed")
                );

                // Evaluating against `advanced_to_direct` before and after `replace_graph`
                // reports `Direct` (`not_applicable` / `direct_placement`) while retaining the
                // same-session receipt summary and total (`receipt_summaries_total == 1`).
                let ctx_direct_before =
                    SuccessorObservationContextV1::from_transaction(tx, &advanced_to_direct)?;
                let obs_direct_before =
                    project_successor_observation_v1(&ctx_direct_before).unwrap();
                assert_eq!(
                    obs_direct_before["status"]["delegation"]["state"],
                    json!("not_applicable")
                );
                assert_eq!(
                    obs_direct_before["status"]["delegation"]["reason"],
                    json!("direct_placement")
                );
                assert_eq!(ctx_direct_before.read_scope().receipts().total(), 1);
                assert_eq!(
                    ctx_direct_before.read_scope().receipts().summaries(),
                    vec![receipt.summary()]
                );
                let standard_direct = project_graph_status_in(
                    &ctx_direct_before.read_scope(),
                    GraphStatusTierV2::Standard,
                    None,
                )
                .unwrap();
                assert_eq!(standard_direct["receipt_summaries_total"], json!(1));
                assert_eq!(
                    standard_direct["receipt_summaries"],
                    json!([receipt.summary()])
                );

                tx.replace_graph(
                    accepted_state.workspace_revision(),
                    accepted_state.trace().revision(),
                    &advanced_to_direct,
                )?;
                let ctx_direct_after =
                    SuccessorObservationContextV1::from_transaction(tx, &advanced_to_direct)?;
                let obs_direct_after = project_successor_observation_v1(&ctx_direct_after).unwrap();
                assert_eq!(obs_direct_before, obs_direct_after);
                direct_obs = obs_direct_before;
                direct_seq = ctx_direct_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(direct_seq, m_direct.outcome().sequence());
        let committed_direct = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_041))?;
                assert_eq!(view.sequence(), m_direct.outcome().sequence());
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(direct_obs, committed_direct);

        // (c.1) Terminal transition in the same delegated session: receipt summary & total are
        //       retained (`receipts().total() == 1`, `receipt_summaries_total == 1`).
        let completed_delegated = advanced_to_direct
            .complete_active_action_v2(
                advanced_to_direct.trace().revision(),
                &attempt_2,
                None,
                UnixMillis::new(1_700_000_000_050),
            )
            .unwrap()
            .into_state();
        let mut completed_del_obs = Map::new();
        let mut completed_del_seq = 0u64;
        let m_term = agent004_commit_mutation(
            &store,
            "agent004-del-k4",
            "00000000-0000-4000-8000-000000006115",
            &session_id,
            UnixMillis::new(1_700_000_000_050),
            |tx| {
                let ctx_term =
                    SuccessorObservationContextV1::from_transaction(tx, &completed_delegated)?;
                assert_eq!(ctx_term.read_scope().receipts().total(), 1);
                assert_eq!(
                    ctx_term.read_scope().receipts().summaries(),
                    vec![receipt.summary()]
                );
                let standard_term = project_graph_status_in(
                    &ctx_term.read_scope(),
                    GraphStatusTierV2::Standard,
                    None,
                )
                .unwrap();
                assert_eq!(standard_term["receipt_summaries_total"], json!(1));
                assert_eq!(
                    standard_term["receipt_summaries"],
                    json!([receipt.summary()])
                );
                let obs_before = project_successor_observation_v1(&ctx_term).unwrap();
                assert_eq!(
                    obs_before["status"]["delegation"]["state"],
                    json!("inactive")
                );
                assert_eq!(
                    obs_before["status"]["delegation"]["reason"],
                    json!("historical_attempt")
                );
                tx.replace_graph(
                    advanced_to_direct.workspace_revision(),
                    advanced_to_direct.trace().revision(),
                    &completed_delegated,
                )?;
                let ctx_after =
                    SuccessorObservationContextV1::from_transaction(tx, &completed_delegated)?;
                assert_eq!(
                    obs_before,
                    project_successor_observation_v1(&ctx_after).unwrap()
                );
                completed_del_obs = obs_before;
                completed_del_seq = ctx_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(completed_del_seq, m_term.outcome().sequence());
        let committed_term = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_051))?;
                assert_eq!(view.sequence(), m_term.outcome().sequence());
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(completed_del_obs, committed_term);

        // (c.2) Fresh replacement session over the same delegated procedure: predecessor receipt
        //       and predecessor terminal disposition are excluded both before and after
        //       `archive_graph` + `create_graph`.
        let snapshot_2 = crate::execution::workspace_procedure_snapshot_from_bytes_v2(
            "agent004-delegation.json",
            &source,
            podway_core::ProcedureSnapshotId::new("00000000-0000-4000-8000-000000006120").unwrap(),
            UnixMillis::new(1_700_000_000_060),
        )
        .unwrap();
        let session_2 =
            podway_core::SessionId::new("00000000-0000-4000-8000-000000006106").unwrap();
        let prepared_2 = GraphSessionStateV2::prepared(
            completed_delegated
                .workspace_revision()
                .checked_next()
                .unwrap(),
            "Fresh delegated session",
            snapshot_2,
            session_2.clone(),
            UnixMillis::new(1_700_000_000_060),
        )
        .unwrap();
        let attempt_fresh =
            podway_core::AttemptId::new("00000000-0000-4000-8000-000000006107").unwrap();
        let begun_2 = prepared_2
            .begin_v2(
                podway_core::Revision::ZERO,
                attempt_fresh.clone(),
                None,
                UnixMillis::new(1_700_000_000_060),
            )
            .unwrap()
            .into_state();
        let cancelled_2 = begun_2
            .cancel_active_session_v2(
                begun_2.trace().revision(),
                &attempt_fresh,
                podway_core::ReasonV2::new("Cancelled fresh session in memory.").unwrap(),
                UnixMillis::new(1_700_000_000_061),
            )
            .unwrap()
            .into_state();
        let mut fresh_obs = Map::new();
        let mut fresh_seq = 0u64;
        let m_fresh = agent004_commit_mutation(
            &store,
            "agent004-del-k5",
            "00000000-0000-4000-8000-000000006116",
            &session_2,
            UnixMillis::new(1_700_000_000_060),
            |tx| {
                let disposition = podway_core::TerminalDispositionV2::not_required(
                    completed_delegated.trace().session_id().clone(),
                    completed_delegated.trace().revision(),
                    "Completed delegated session.",
                    None,
                    UnixMillis::new(1_700_000_000_060),
                )
                .unwrap();
                tx.record_disposition(&disposition)?;
                assert_eq!(tx.terminal_dispositions()?.len(), 1);

                // Even while `completed_delegated` and its receipt + disposition are still current
                // in SQLite, `prepared_2` and `cancelled_2` (bound to `session_2`) exclude the
                // predecessor session's receipt and terminal disposition.
                let ctx_fresh_before =
                    SuccessorObservationContextV1::from_transaction(tx, &prepared_2)?;
                assert_eq!(ctx_fresh_before.read_scope().receipts().total(), 0);
                assert!(
                    ctx_fresh_before
                        .read_scope()
                        .receipts()
                        .summaries()
                        .is_empty()
                );
                let standard_fresh = project_graph_status_in(
                    &ctx_fresh_before.read_scope(),
                    GraphStatusTierV2::Standard,
                    None,
                )
                .unwrap();
                assert_eq!(standard_fresh["receipt_summaries_total"], json!(0));
                assert_eq!(standard_fresh["receipt_summaries"], json!([]));

                let ctx_cancelled_2 =
                    SuccessorObservationContextV1::from_transaction(tx, &cancelled_2)?;
                assert!(!ctx_cancelled_2.read_scope().current_terminal_disposition());
                let obs_cancelled_2 = project_successor_observation_v1(&ctx_cancelled_2).unwrap();
                assert_eq!(
                    obs_cancelled_2["mutation_templates"]
                        .as_array()
                        .unwrap()
                        .len(),
                    2
                );

                let obs_before = project_successor_observation_v1(&ctx_fresh_before).unwrap();
                tx.archive_graph(
                    completed_delegated.workspace_revision(),
                    completed_delegated.trace().revision(),
                )?;
                tx.create_graph(&prepared_2)?;
                assert!(tx.terminal_dispositions()?.is_empty());
                let ctx_fresh_after =
                    SuccessorObservationContextV1::from_transaction(tx, &prepared_2)?;
                let obs_after = project_successor_observation_v1(&ctx_fresh_after).unwrap();
                assert_eq!(obs_before, obs_after);
                fresh_obs = obs_before;
                fresh_seq = ctx_fresh_after.workspace_sequence();
                Ok(())
            },
        );
        assert_eq!(fresh_seq, m_fresh.outcome().sequence());
        let committed_fresh = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_061))?;
                assert_eq!(view.sequence(), m_fresh.outcome().sequence());
                assert!(view.terminal_dispositions().is_empty());
                assert_eq!(view.delegation_receipts().total(), 0);
                assert!(view.delegation_receipts().summaries().is_empty());
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(fresh_obs, committed_fresh);

        // (b.3) Commit a changed delegated `record-many` successor (`edited_after_accept_2`) on
        //       `session_2`, verify persisted `ReceiptCurrent` while projecting the changed
        //       in-memory successor (`ItemsChanged`, `stale_acceptance` / `items_changed`),
        //       persist it, and compare outcome sequence, view sequence, and full v6 observation.
        let with_note_2 = begun_2
            .mutate_active_items_v2(
                &attempt_fresh,
                &[podway_core::ActiveItemMutationRequestV2::new(
                    podway_core::ItemId::new("note").unwrap(),
                    podway_core::Revision::ZERO,
                    podway_core::ActiveItemMutationV2::Set {
                        value: "Initial accepted note 2.".to_owned(),
                    },
                )],
                UnixMillis::new(1_700_000_000_063),
            )
            .unwrap()
            .into_state();
        let m_fresh_begun = agent004_commit_mutation(
            &store,
            "agent004-del-k6",
            "00000000-0000-4000-8000-000000006117",
            &session_2,
            UnixMillis::new(1_700_000_000_062),
            |tx| {
                tx.replace_graph(
                    prepared_2.workspace_revision(),
                    prepared_2.trace().revision(),
                    &begun_2,
                )
            },
        );
        let m_fresh_note = agent004_commit_mutation(
            &store,
            "agent004-del-k7",
            "00000000-0000-4000-8000-000000006118",
            &session_2,
            UnixMillis::new(1_700_000_000_063),
            |tx| {
                tx.replace_graph(
                    begun_2.workspace_revision(),
                    begun_2.trace().revision(),
                    &with_note_2,
                )
            },
        );
        assert!(m_fresh_note.outcome().sequence() > m_fresh_begun.outcome().sequence());
        let view_2 = GraphWorkspaceViewV2::new(
            views004_identity(),
            Some(with_note_2.clone()),
            m_fresh_note.outcome().sequence(),
            UnixMillis::new(1_700_000_000_064),
        );
        let package_2 = delegation_work_package_from_view_v1(&view_2).unwrap();
        let accepted_2 = with_note_2
            .accept_delegation_successor_v1(&attempt_fresh)
            .unwrap();
        let items_digest_2 =
            podway_store::v2_delegation::accepted_items_digest_v1(&with_note_2).unwrap();
        let receipt_2 = DelegationReceiptV1::new(json!({
            "schema": "podway.delegation-receipt/v1",
            "receipt_id": "00000000-0000-4000-8000-000000006121",
            "workspace_uuid": "00000000-0000-4000-8000-000000005001",
            "procedure_digest": with_note_2.snapshot().digest(),
            "session_id": session_2,
            "placement_id": "delegate-step",
            "attempt_id": attempt_fresh,
            "delegation": package_2.package()["delegation"],
            "package_digest": package_2.digest(),
            "execution": {
                "binding_ref": "worker-1",
                "adapter_id": "herdr",
                "adapter_version": "1.0.0",
                "primary_host": "managed",
                "scope_kind": "task",
                "scope_ref": "scope-1",
                "supervisor_ref": "sup-1",
                "worker_ref": "w-1",
                "task_ref": "task-2",
                "role_ref": "reviewer",
                "role_snapshot_digest": role_digest,
                "agent_configuration_digest": cfg_digest,
                "accepted_task_digest": task_digest
            },
            "rounds_used": 1,
            "result_digest": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
            "summary": "Accepted delegated step in session 2.",
            "worker_disposition": "idle",
            "accepted": true,
            "accepted_items_digest": items_digest_2,
            "accepted_at": "2023-11-14T22:13:20.065Z"
        }))
        .unwrap();
        let m_fresh_accept = agent004_commit_mutation(
            &store,
            "agent004-del-k8",
            "00000000-0000-4000-8000-000000006119",
            &session_2,
            UnixMillis::new(1_700_000_000_065),
            |tx| {
                tx.accept_delegation(
                    with_note_2.workspace_revision(),
                    with_note_2.trace().revision(),
                    &accepted_2,
                    package_2.digest(),
                    &receipt_2,
                )
            },
        );
        let changed_outcome_2 = accepted_2
            .mutate_active_items_v2(
                &attempt_fresh,
                &[podway_core::ActiveItemMutationRequestV2::new(
                    podway_core::ItemId::new("note").unwrap(),
                    podway_core::Revision::new(1),
                    podway_core::ActiveItemMutationV2::Set {
                        value: "Committed edit after acceptance.".to_owned(),
                    },
                )],
                UnixMillis::new(1_700_000_000_070),
            )
            .unwrap();
        assert!(changed_outcome_2.changed());
        let edited_after_accept_2 = changed_outcome_2.into_state();
        let mut edited_obs = Map::new();
        let mut edited_seq = 0u64;
        let m_edited = agent004_commit_mutation(
            &store,
            "agent004-del-k9",
            "00000000-0000-4000-8000-000000006122",
            &session_2,
            UnixMillis::new(1_700_000_000_070),
            |tx| {
                assert_eq!(
                    tx.delegation_receipts()?.readiness(),
                    DelegationReadinessV1::ReceiptCurrent
                );
                let ctx_edited_before =
                    SuccessorObservationContextV1::from_transaction(tx, &edited_after_accept_2)?;
                assert_eq!(
                    ctx_edited_before.read_scope().receipts().readiness(),
                    DelegationReadinessV1::ItemsChanged
                );
                assert_eq!(ctx_edited_before.read_scope().receipts().total(), 1);
                assert_eq!(
                    ctx_edited_before.read_scope().receipts().summaries(),
                    vec![receipt_2.summary()]
                );
                edited_seq = ctx_edited_before.workspace_sequence();
                let obs_edited_before =
                    project_successor_observation_v1(&ctx_edited_before).unwrap();
                assert_eq!(
                    obs_edited_before["status"]["delegation"]["state"],
                    json!("stale_acceptance")
                );
                assert_eq!(
                    obs_edited_before["status"]["delegation"]["reason"],
                    json!("items_changed")
                );
                assert_eq!(
                    obs_edited_before["status"]["current"]["readiness"]["can_advance"],
                    json!(false)
                );
                assert_eq!(
                    obs_edited_before["status"]["session"]["revision"],
                    json!(edited_after_accept_2.trace().revision().get())
                );
                assert_eq!(
                    obs_edited_before["status"]["items"][0]["revision"],
                    json!(2)
                );
                tx.replace_graph(
                    accepted_2.workspace_revision(),
                    accepted_2.trace().revision(),
                    &edited_after_accept_2,
                )?;
                let ctx_edited_after =
                    SuccessorObservationContextV1::from_transaction(tx, &edited_after_accept_2)?;
                assert_eq!(
                    obs_edited_before,
                    project_successor_observation_v1(&ctx_edited_after).unwrap()
                );
                assert_eq!(ctx_edited_after.workspace_sequence(), edited_seq);
                edited_obs = obs_edited_before;
                Ok(())
            },
        );
        assert!(edited_seq > m_fresh_accept.outcome().sequence());
        assert_eq!(edited_seq, m_edited.outcome().sequence());
        let committed_edited = store
            .read(Instant::now() + Duration::from_secs(5), |snapshot| {
                let view = snapshot.view(UnixMillis::new(1_700_000_000_071))?;
                assert_eq!(view.sequence(), m_edited.outcome().sequence());
                assert_eq!(view.sequence(), edited_seq);
                Ok(crate::direct_read_service::project_direct_observation_v1(
                    &view,
                    ContractGenerationV1::Generation2,
                )
                .unwrap())
            })
            .unwrap();
        assert_eq!(edited_obs, committed_edited);
    }

    #[test]
    fn agent004_serialized_json_len_substitutions_preserve_exact_bytes_windows_and_fallbacks() {
        // 1. serialized_bytes_v1 & serialized_object_bytes_v1 exact parity with serde_json::to_vec.
        let samples = [
            json!({}),
            json!({"ascii": "plain text", "n": 42}),
            json!({"utf8": "é 雪 😀", "escaped": "quote\" slash\\ tab\t ctrl\u{0001}"}),
            json!({"nested": [{"a": [true, false, null]}, {"b": u64::MAX}]}),
        ];
        for sample in samples {
            let expected = serde_json::to_vec(&sample)
                .unwrap()
                .len()
                .saturating_add(EVIDENCE_ENTRY_OVERHEAD_BYTES);
            assert_eq!(serialized_bytes_v1(&sample), expected);
            if let Value::Object(map) = &sample {
                assert_eq!(serialized_object_bytes_v1(map), expected);
            }
        }

        // 2. CurrentProjection::item_values and CurrentProjection::blocker_window exact boundary,
        //    one-byte-over refusal, and comma-separator accounting across ASCII, multibyte UTF-8,
        //    and JSON-escaped text using reachable domain inputs.
        let item_defs: Vec<Value> = (0..40)
            .map(|idx| {
                json!({
                    "id": format!("i{idx:02}"),
                    "type": "text",
                    "prompt": "Item.",
                    "required": false,
                    "max_length": 2050
                })
            })
            .collect();
        let source = serde_json::to_vec(&json!({
            "schema": "podway.procedure/v2",
            "id": "agent004-windows",
            "version": "1",
            "name": "AGENT-004 window byte accounting",
            "purpose": "Verify item_values and blocker_window byte boundaries.",
            "node_definitions": {
                "work": {
                    "type": "action",
                    "title": "Work",
                    "intent": "Exercise status windows.",
                    "items": item_defs
                }
            },
            "graph": {
                "entry": "work",
                "nodes": [
                    {"id": "work", "use": "work", "terminal": true}
                ]
            }
        }))
        .unwrap();
        let snapshot = crate::execution::workspace_procedure_snapshot_from_bytes_v2(
            "agent004-windows.json",
            &source,
            podway_core::ProcedureSnapshotId::new("00000000-0000-4000-8000-000000006201").unwrap(),
            UnixMillis::new(1_700_000_000_000),
        )
        .unwrap();
        let session_id =
            podway_core::SessionId::new("00000000-0000-4000-8000-000000006202").unwrap();
        let attempt_id =
            podway_core::AttemptId::new("00000000-0000-4000-8000-000000006203").unwrap();
        let prepared = GraphSessionStateV2::prepared(
            podway_core::Revision::new(1),
            "AGENT-004 window task",
            snapshot,
            session_id,
            UnixMillis::new(1_700_000_000_000),
        )
        .unwrap();
        let running = prepared
            .begin_v2(
                podway_core::Revision::ZERO,
                attempt_id.clone(),
                None,
                UnixMillis::new(1_700_000_000_000),
            )
            .unwrap()
            .into_state();

        // Populate `item_values` with reachable domain inputs covering:
        // - i00: ASCII, 2-byte escapes (\" \\ \t), and 2/3/4-byte UTF-8 (é 雪 😀)
        // - i01: 2050-scalar text truncated to ITEM_DISPLAY_CHARS_MAX (2048) with value_truncated: true
        // - i02..: wide 4-byte UTF-8 ("😀".repeat(2048)) and ASCII ("a".repeat(1500)) items until
        //          the remaining budget to STATUS_VALUES_BYTES_MAX (262,144) fits in a single
        //          final untruncated ASCII item (`exact_last_item_len <= 2000`).
        let empty_item_entry = json!({
            "item_id": "i99",
            "value": "",
            "value_truncated": false,
        });
        let item_base_bytes = serde_json::to_vec(&empty_item_entry).unwrap().len();
        let mut item_state = running;
        let mut item_used = 2usize;
        let mut item_count = 0usize;

        let i00_val = "quote\" slash\\ tab\t é 雪 😀".repeat(80);
        item_state = item_state
            .mutate_active_item_v2(
                &attempt_id,
                &podway_core::ItemId::new("i00").unwrap(),
                podway_core::Revision::ZERO,
                podway_core::ActiveItemMutationV2::Set {
                    value: i00_val.clone(),
                },
                UnixMillis::new(1_700_000_000_000),
            )
            .unwrap()
            .into_state();
        let i00_candidate = json!({
            "item_id": "i00",
            "value": i00_val,
            "value_truncated": false,
        });
        item_used += serde_json::to_vec(&i00_candidate).unwrap().len();
        item_count += 1;

        let i01_val = "x".repeat(2050);
        item_state = item_state
            .mutate_active_item_v2(
                &attempt_id,
                &podway_core::ItemId::new("i01").unwrap(),
                podway_core::Revision::ZERO,
                podway_core::ActiveItemMutationV2::Set { value: i01_val },
                UnixMillis::new(1_700_000_000_000),
            )
            .unwrap()
            .into_state();
        let i01_candidate = json!({
            "item_id": "i01",
            "value": "x".repeat(ITEM_DISPLAY_CHARS_MAX),
            "value_truncated": true,
        });
        item_used += 1 + serde_json::to_vec(&i01_candidate).unwrap().len();
        item_count += 1;

        while STATUS_VALUES_BYTES_MAX - (item_used + 1 + item_base_bytes) > 2000 {
            let rem = STATUS_VALUES_BYTES_MAX - (item_used + 1 + item_base_bytes);
            let val = if rem > 8500 {
                "😀".repeat(2048)
            } else {
                "a".repeat(1500)
            };
            let id_str = format!("i{item_count:02}");
            let candidate = json!({
                "item_id": id_str,
                "value": val,
                "value_truncated": false,
            });
            item_state = item_state
                .mutate_active_item_v2(
                    &attempt_id,
                    &podway_core::ItemId::new(id_str).unwrap(),
                    podway_core::Revision::ZERO,
                    podway_core::ActiveItemMutationV2::Set { value: val },
                    UnixMillis::new(1_700_000_000_000),
                )
                .unwrap()
                .into_state();
            item_used += 1 + serde_json::to_vec(&candidate).unwrap().len();
            item_count += 1;
        }
        let exact_last_item_len = STATUS_VALUES_BYTES_MAX - (item_used + 1 + item_base_bytes);
        assert!((1..=2000).contains(&exact_last_item_len));
        let last_item_id = podway_core::ItemId::new(format!("i{item_count:02}")).unwrap();
        let exact_items_state = item_state
            .mutate_active_item_v2(
                &attempt_id,
                &last_item_id,
                podway_core::Revision::ZERO,
                podway_core::ActiveItemMutationV2::Set {
                    value: "a".repeat(exact_last_item_len),
                },
                UnixMillis::new(1_700_000_000_000),
            )
            .unwrap()
            .into_state();
        let over_items_state = item_state
            .mutate_active_item_v2(
                &attempt_id,
                &last_item_id,
                podway_core::Revision::ZERO,
                podway_core::ActiveItemMutationV2::Set {
                    value: "a".repeat(exact_last_item_len + 1),
                },
                UnixMillis::new(1_700_000_000_000),
            )
            .unwrap()
            .into_state();

        // Measure blocker_window exact fit (49,152 bytes) vs 1-byte overflow (49,153 bytes)
        // across ASCII, 2-byte escapes (\" \\ \t \n), 6-byte control escapes (\u{0001}), and
        // 2/3/4-byte UTF-8 (é 雪 😀) using independently measured serde_json::to_vec sizes.
        let sample_blocker_id =
            podway_core::BlockerId::new("00000000-0000-4000-8000-000000006299").unwrap();
        let empty_reason_entry = json!({
            "blocker_id": sample_blocker_id.as_str(),
            "reason": "",
            "created_at": "2023-11-14T22:13:20.000Z",
        });
        let base_entry_bytes = serde_json::to_vec(&empty_reason_entry).unwrap().len();
        let mut blocked = exact_items_state;
        let mut used = 2usize;
        let mut count = 0usize;

        let b0_id = podway_core::BlockerId::new("00000000-0000-4000-8000-000000007000").unwrap();
        let b0_reason = "quote\" slash\\ tab\t nl\n ctrl\u{0001} é 雪 😀".repeat(25);
        let b0_entry = json!({
            "blocker_id": b0_id.as_str(),
            "reason": b0_reason,
            "created_at": "2023-11-14T22:13:20.000Z",
        });
        blocked = blocked
            .block_active_attempt_v2(
                blocked.trace().revision(),
                &attempt_id,
                b0_id,
                b0_reason,
                UnixMillis::new(1_700_000_000_000),
            )
            .unwrap()
            .into_state();
        used += serde_json::to_vec(&b0_entry).unwrap().len();
        count += 1;

        while BLOCKER_WINDOW_BYTES_MAX - (used + 1 + base_entry_bytes) > 900 {
            let rem = BLOCKER_WINDOW_BYTES_MAX - (used + 1 + base_entry_bytes);
            let reason = if rem > 2700 {
                "é雪😀\"\\\t\u{0001}".repeat(120)
            } else {
                "b".repeat(800)
            };
            let bid = podway_core::BlockerId::new(format!(
                "00000000-0000-4000-8000-{:012}",
                7000 + count
            ))
            .unwrap();
            let entry = json!({
                "blocker_id": bid.as_str(),
                "reason": reason,
                "created_at": "2023-11-14T22:13:20.000Z",
            });
            blocked = blocked
                .block_active_attempt_v2(
                    blocked.trace().revision(),
                    &attempt_id,
                    bid,
                    reason,
                    UnixMillis::new(1_700_000_000_000),
                )
                .unwrap()
                .into_state();
            used += 1 + serde_json::to_vec(&entry).unwrap().len();
            count += 1;
        }
        let exact_last_reason_bytes = BLOCKER_WINDOW_BYTES_MAX - (used + 1 + base_entry_bytes);
        let last_prefix = "é雪😀\"\\\t\u{0001}";
        let prefix_json_bytes = serde_json::to_vec(&Value::String(last_prefix.to_owned()))
            .unwrap()
            .len()
            - 2;
        assert!((prefix_json_bytes + 1..=900).contains(&exact_last_reason_bytes));
        let exact_last_reason = format!(
            "{last_prefix}{}",
            "b".repeat(exact_last_reason_bytes - prefix_json_bytes)
        );
        let over_last_reason = format!(
            "{last_prefix}{}",
            "b".repeat(exact_last_reason_bytes - prefix_json_bytes + 1)
        );
        let last_bid =
            podway_core::BlockerId::new(format!("00000000-0000-4000-8000-{:012}", 7000 + count))
                .unwrap();
        let exact_blocked = blocked
            .block_active_attempt_v2(
                blocked.trace().revision(),
                &attempt_id,
                last_bid.clone(),
                exact_last_reason,
                UnixMillis::new(1_700_000_000_000),
            )
            .unwrap()
            .into_state();
        let over_blocked = blocked
            .block_active_attempt_v2(
                blocked.trace().revision(),
                &attempt_id,
                last_bid,
                over_last_reason,
                UnixMillis::new(1_700_000_000_000),
            )
            .unwrap()
            .into_state();

        let ParsedProcedure::V2(procedure) =
            parse_procedure_document(&source, ProcedureDocumentFormat::Json).unwrap();
        let exact_view = GraphWorkspaceViewV2::new(
            views004_identity(),
            Some(exact_blocked),
            1,
            UnixMillis::new(1_700_000_000_100),
        );
        let exact_scope = ReadScope::from_graph(&exact_view);
        let exact_proj =
            CurrentProjection::derive(&exact_scope, exact_view.graph_state().unwrap(), &procedure)
                .unwrap()
                .unwrap();
        let (window, truncated) = exact_proj.blocker_window().unwrap();
        assert_eq!(window.len(), count + 1);
        assert!(
            !truncated,
            "exact BLOCKER_WINDOW_BYTES_MAX fit must not truncate"
        );
        assert!(window.iter().all(|entry| {
            entry["reason"].as_str().is_some_and(|reason| {
                reason.contains('é')
                    && reason.contains('雪')
                    && reason.contains('😀')
                    && reason.contains('\u{0001}')
            })
        }));
        assert_eq!(
            serde_json::to_vec(&Value::Array(window)).unwrap().len(),
            BLOCKER_WINDOW_BYTES_MAX
        );
        let (item_vals, item_total, item_trunc) = exact_proj.item_values();
        assert_eq!(item_total, item_count + 1);
        assert!(
            !item_trunc,
            "exact STATUS_VALUES_BYTES_MAX fit must not truncate"
        );
        assert_eq!(item_vals.len(), item_count + 1);
        assert_eq!(
            serde_json::to_vec(&Value::Array(item_vals.clone()))
                .unwrap()
                .len(),
            STATUS_VALUES_BYTES_MAX
        );
        assert_eq!(item_vals[0]["value_truncated"], json!(false));
        assert_eq!(item_vals[1]["value_truncated"], json!(true));

        let over_items_view = GraphWorkspaceViewV2::new(
            views004_identity(),
            Some(over_items_state),
            1,
            UnixMillis::new(1_700_000_000_100),
        );
        let over_items_scope = ReadScope::from_graph(&over_items_view);
        let over_items_proj = CurrentProjection::derive(
            &over_items_scope,
            over_items_view.graph_state().unwrap(),
            &procedure,
        )
        .unwrap()
        .unwrap();
        let (over_item_vals, over_item_total, over_item_trunc) = over_items_proj.item_values();
        assert_eq!(over_item_total, item_count + 1);
        assert!(
            over_item_trunc,
            "one byte over STATUS_VALUES_BYTES_MAX must truncate"
        );
        assert_eq!(over_item_vals.len(), item_count);

        let over_view = GraphWorkspaceViewV2::new(
            views004_identity(),
            Some(over_blocked),
            1,
            UnixMillis::new(1_700_000_000_100),
        );
        let over_scope = ReadScope::from_graph(&over_view);
        let over_proj =
            CurrentProjection::derive(&over_scope, over_view.graph_state().unwrap(), &procedure)
                .unwrap()
                .unwrap();
        let (over_window, over_truncated) = over_proj.blocker_window().unwrap();
        assert_eq!(over_window.len(), count);
        assert!(
            over_truncated,
            "one byte over BLOCKER_WINDOW_BYTES_MAX must truncate"
        );
    }
}
