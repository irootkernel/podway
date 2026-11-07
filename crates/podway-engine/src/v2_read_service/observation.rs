//! Coherent Procedure v2 observation assembly, lifecycle templates, and observation byte budgets.

use std::cell::RefCell;

use podway_config::ParsedProcedureV2;
use podway_core::{
    EVIDENCE_ENTRY_OVERHEAD_BYTES_V2 as EVIDENCE_ENTRY_OVERHEAD_BYTES, MAX_ACTIVE_ITEM_WINDOW_V2,
    MAX_MUTATION_TEMPLATE_WINDOW_V2, MAX_OBSERVATION_ACTIVE_ITEM_BYTES_V2,
    MAX_OBSERVATION_STATIC_MEMBERS_BYTES_V2, MAX_OBSERVATION_TEMPLATE_BYTES_V2, SessionLifecycle,
    UnixMillis,
};
use podway_protocol::{ContractGenerationV1, serialized_json_len_v1};
use podway_store::{
    DurableWorktreeIdentityV1, GraphSessionStateV2, v2_delegation::DelegationReceiptSnapshotV1,
};
use serde_json::{Map, Value, json};

use super::{
    CurrentProjection, EvidencePreviewDisplay, GraphViewErrorV2, ReadScope, add_goal_summary,
    counters, delegated_placement_count, delegation_projection, into_object, procedure_identity,
    project_running_next, session_identity,
};

pub(crate) struct SuccessorObservationContextV1<'a> {
    identity: &'a DurableWorktreeIdentityV1,
    successor: &'a GraphSessionStateV2,
    receipts: DelegationReceiptSnapshotV1,
    workspace_sequence: u64,
}

impl<'a> SuccessorObservationContextV1<'a> {
    pub(crate) fn from_transaction<Clock: FnOnce() -> UnixMillis>(
        transaction: &'a podway_store::direct_store::DirectTransactionV1<'_, '_, Clock>,
        successor: &'a GraphSessionStateV2,
    ) -> podway_store::direct_store::DirectStoreResultV1<Self> {
        let workspace_sequence = transaction.next_workspace_sequence()?;
        let receipts = transaction.delegation_receipts_for_graph(successor)?;
        Ok(Self {
            identity: transaction.binding().identity(),
            successor,
            receipts,
            workspace_sequence,
        })
    }

    pub(crate) fn workspace_sequence(&self) -> u64 {
        self.workspace_sequence
    }

    pub(super) fn read_scope(&self) -> ReadScope<'_> {
        ReadScope {
            identity: self.identity,
            graph: Some(self.successor),
            receipts: &self.receipts,
            current_terminal_disposition: false,
            validated: RefCell::new(None),
        }
    }
}

pub(crate) fn project_successor_observation_v1(
    context: &SuccessorObservationContextV1<'_>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    finalize_direct_observation_in(&context.read_scope(), ContractGenerationV1::Generation2)
}

pub(crate) fn finalize_direct_observation_in(
    scope: &ReadScope<'_>,
    generation: ContractGenerationV1,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let schema = match generation {
        ContractGenerationV1::Generation1 => "podway.observation-result/v5",
        ContractGenerationV1::Generation2 => "podway.observation-result/v6",
    };
    let mut result = project_graph_observation_in(scope, generation)?;
    result.insert("schema".into(), Value::String(schema.into()));
    result
        .get("status")
        .and_then(Value::as_object)
        .ok_or(GraphViewErrorV2::InconsistentState(
            "observation has no structured status",
        ))?;
    if let Some(guidance) = result.get_mut("guidance").and_then(Value::as_object_mut) {
        let guidance_schema = crate::direct_read_service::direct_next_schema_for_lifecycle(
            scope.graph_state().map(|graph| graph.trace().lifecycle()),
        );
        guidance.insert("schema".into(), Value::String(guidance_schema.into()));
    }
    crate::direct_read_service::validate_projection(schema, result)
}

pub(super) fn project_graph_observation_in(
    scope: &ReadScope<'_>,
    generation: ContractGenerationV1,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let state = scope.graph()?;
    let procedure = scope.procedure()?;
    let current = CurrentProjection::derive(scope, state, &procedure)?;
    let delegated = delegated_placement_count(&procedure) != 0;
    // ADR-0024 reduces the observation status member to a value-free compact projection: guidance
    // and the active-item window already carry what a mutation needs, and a caller that wants the
    // standard or verbose status projection calls `session.status`.
    let mut status = observation_status_v1(state, current.as_ref())?;
    if delegated {
        status.insert(
            "delegation".to_owned(),
            delegation_projection(scope, state, current.as_ref()),
        );
    }
    let guidance = if current.is_some() || state.trace().lifecycle() == SessionLifecycle::Prepared {
        let next = if state.trace().lifecycle() == SessionLifecycle::Prepared {
            project_prepared_next_v1(state, &procedure)?
        } else {
            // Suppressed guidance admits previews through the same slot and truncation gates as
            // `session.next` — including the snapshot, consumer, and token-constructibility
            // checks — without materializing the preview payloads ADR-0024 removes here.
            project_running_next(
                scope,
                state,
                &procedure,
                current.as_ref().unwrap(),
                EvidencePreviewDisplay::Suppress,
            )?
        };
        Value::Object(next)
    } else {
        Value::Null
    };

    // Status and guidance are single structured members, not windows. Both carry the complete
    // graph-wide counter set, so they share one allocation: long graphs spend more on counters,
    // while content-rich active placements spend more on guidance. Neither member is trimmed.
    let status = Value::Object(status);
    admit_members_within_bytes_v1(
        [&status, &guidance],
        MAX_OBSERVATION_STATIC_MEMBERS_BYTES_V2,
    )?;

    let mut result = Map::new();
    let observation_schema = match generation {
        ContractGenerationV1::Generation1 => "podway.observation-result/v5",
        ContractGenerationV1::Generation2 => "podway.observation-result/v6",
    };
    result.insert("schema".to_owned(), json!(observation_schema));
    result.insert("status".to_owned(), status);
    result.insert("guidance".to_owned(), guidance);
    let active_items = current
        .as_ref()
        .map(CurrentProjection::active_item_descriptors)
        .unwrap_or_default();
    // Each window keeps its exact total and admits entries in declaration order until its byte
    // allocation is spent. ADR-0024 forbids a silent cut, so the flag travels with the count.
    let (active_items, active_items_total) = admit_within_bytes_v1(
        active_items,
        MAX_OBSERVATION_ACTIVE_ITEM_BYTES_V2,
        MAX_ACTIVE_ITEM_WINDOW_V2,
    );
    result.insert("active_items_total".to_owned(), json!(active_items_total));
    result.insert(
        "active_items_truncated".to_owned(),
        json!(active_items.len() < active_items_total),
    );
    result.insert("active_items".to_owned(), Value::Array(active_items));
    let mutation_templates = if state.trace().lifecycle() == SessionLifecycle::Prepared {
        match generation {
            ContractGenerationV1::Generation1 => prepared_mutation_templates(scope, state),
            ContractGenerationV1::Generation2 => prepared_mutation_templates_v6(scope, state),
        }
    } else if matches!(
        state.trace().lifecycle(),
        SessionLifecycle::Completed | SessionLifecycle::Cancelled
    ) {
        match generation {
            ContractGenerationV1::Generation1 => terminal_mutation_templates(scope, state),
            ContractGenerationV1::Generation2 => terminal_mutation_templates_v6(scope, state),
        }
    } else {
        current
            .as_ref()
            .map(|current| current.mutation_templates(scope, state, &procedure))
            .unwrap_or_default()
    };
    let (mutation_templates, mutation_templates_total) = admit_templates_within_bytes_v1(
        mutation_templates,
        MAX_OBSERVATION_TEMPLATE_BYTES_V2,
        MAX_MUTATION_TEMPLATE_WINDOW_V2,
    )?;
    result.insert(
        "mutation_templates_total".to_owned(),
        json!(mutation_templates_total),
    );
    result.insert(
        "mutation_templates_truncated".to_owned(),
        json!(mutation_templates.len() < mutation_templates_total),
    );
    result.insert(
        "mutation_templates".to_owned(),
        Value::Array(mutation_templates),
    );
    Ok(result)
}

/// Measures complete observation members against their shared allocation.
///
/// A member is not a window: there is no honest way to return half a status or half a guidance
/// object, and their own collections are already bounded. So a member that outgrows its allocation
/// is an inconsistency in the composition rather than something to ship over budget.
pub(super) fn admit_members_within_bytes_v1<const N: usize>(
    members: [&Value; N],
    allocation: usize,
) -> Result<(), GraphViewErrorV2> {
    let charged = members.iter().fold(0usize, |total, member| {
        total.saturating_add(serialized_bytes_v1(member))
    });
    if charged > allocation {
        return Err(GraphViewErrorV2::InconsistentState(
            "the complete observation members exceeded their shared byte allocation",
        ));
    }
    Ok(())
}

/// Admits window entries in declaration order until either allocation is spent.
///
/// Returns the admitted entries and the exact total the caller must publish alongside them. The
/// first entry is always admitted: a window that can answer nothing tells a caller less than a
/// window that answers once and says it was cut.
pub(super) fn admit_within_bytes_v1(
    entries: Vec<Value>,
    budget: usize,
    window: usize,
) -> (Vec<Value>, usize) {
    let total = entries.len();
    let mut charged = 0usize;
    let mut admitted = 0usize;
    for entry in entries.iter().take(window) {
        let cost = serialized_bytes_v1(entry);
        if admitted > 0 && charged.saturating_add(cost) > budget {
            break;
        }
        charged = charged.saturating_add(cost);
        admitted += 1;
    }
    let mut entries = entries;
    entries.truncate(admitted);
    (entries, total)
}

/// Admits mutation templates, keeping one per command before spending the rest of the allocation.
///
/// Guidance publishes `allowed_actions`, and a caller turns each one into a request by finding its
/// template. Cutting purely in declaration order can therefore leave an allowed action with no
/// template at all, which reads as a contradiction rather than as a truncated window. Reserving
/// the first template of each command keeps the window's cut confined to repetition.
pub(super) fn admit_templates_within_bytes_v1(
    templates: Vec<Value>,
    budget: usize,
    window: usize,
) -> Result<(Vec<Value>, usize), GraphViewErrorV2> {
    let total = templates.len();
    let mut seen: Vec<&str> = Vec::new();
    let mut reserved = vec![false; total];
    for (index, template) in templates.iter().enumerate() {
        let Some(command) = template.get("command").and_then(Value::as_str) else {
            continue;
        };
        if !seen.contains(&command) {
            seen.push(command);
            reserved[index] = true;
        }
    }

    // The reservation is admitted whole, because dropping it would leave an `allowed_actions`
    // entry with no template — a contradiction, not a truncation. It is still charged: the command
    // set is closed and small, so it fits by construction, and if it ever stopped fitting that is
    // an inconsistency in the composition rather than something to silently ship over budget.
    let mut charged = 0usize;
    let mut admitted = 0usize;
    let mut keep = reserved.clone();
    for (index, template) in templates.iter().enumerate() {
        if !reserved[index] {
            continue;
        }
        charged = charged.saturating_add(serialized_bytes_v1(template));
        admitted += 1;
    }
    if charged > budget || admitted > window {
        return Err(GraphViewErrorV2::InconsistentState(
            "one template per command does not fit the template allocation",
        ));
    }
    for (index, template) in templates.iter().enumerate() {
        if keep[index] {
            continue;
        }
        if admitted >= window {
            break;
        }
        let cost = serialized_bytes_v1(template);
        if charged.saturating_add(cost) > budget {
            continue;
        }
        charged = charged.saturating_add(cost);
        admitted += 1;
        keep[index] = true;
    }

    let admitted_templates = templates
        .into_iter()
        .zip(keep)
        .filter_map(|(template, keep)| keep.then_some(template))
        .collect();
    Ok((admitted_templates, total))
}

/// The bytes one projected value serializes to, plus its array-element structure.
pub(super) fn serialized_bytes_v1(value: &Value) -> usize {
    serialized_json_len_v1(value)
        .unwrap_or(usize::MAX)
        .saturating_add(EVIDENCE_ENTRY_OVERHEAD_BYTES)
}

/// The bytes one projected result map serializes to, plus the same entry overhead
/// [`serialized_bytes_v1`] charges, measured without wrapping the map in an owned `Value`.
///
/// `Value::Object` serializes by delegating to the map's own `Serialize` implementation, so the
/// measured bytes are identical to the wrapped form this replaces; the difference is that the
/// measured map is no longer cloned once just to carry it into the measurement.
pub(super) fn serialized_object_bytes_v1(result: &Map<String, Value>) -> usize {
    serialized_json_len_v1(result)
        .unwrap_or(usize::MAX)
        .saturating_add(EVIDENCE_ENTRY_OVERHEAD_BYTES)
}

/// The observation-scoped compact status projection.
///
/// It is embedded structured status and carries the item window's exact total and truncation state.
fn observation_status_v1(
    state: &GraphSessionStateV2,
    current: Option<&CurrentProjection<'_>>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let mut status = Map::new();
    status.insert("procedure".to_owned(), procedure_identity(state));
    status.insert("session".to_owned(), session_identity(state));
    status.insert(
        "current".to_owned(),
        current
            .map(CurrentProjection::identity)
            .unwrap_or(Value::Null),
    );
    status.insert(
        "goal_tracking".to_owned(),
        json!(state.snapshot().goal_tracking()),
    );
    status.insert(
        "goal_defined".to_owned(),
        json!(state.goal_state().current_revision().is_some()),
    );
    add_goal_summary(&mut status, state);
    status.insert(
        "trace_length".to_owned(),
        json!(state.trace().attempts().len()),
    );
    status.insert("counters".to_owned(), counters(state));
    let items = current
        .map(CurrentProjection::compact_items)
        .unwrap_or_default();
    status.insert("items_total".to_owned(), json!(items.len()));
    status.insert("items_truncated".to_owned(), json!(false));
    status.insert("items".to_owned(), Value::Array(items));
    Ok(status)
}

pub(super) fn project_prepared_next_v1(
    state: &GraphSessionStateV2,
    procedure: &ParsedProcedureV2,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let delegated_placement_count = delegated_placement_count(procedure);
    let mut result = into_object(json!({
        "schema": "podway.prepared-next-result/v3",
        "procedure_schema": "podway.procedure/v2",
        "procedure_digest": state.snapshot().digest().as_str(),
        "session_id": state.trace().session_id(),
        "session_state": "prepared",
        "revision": 0,
        "goal_tracking": state.snapshot().goal_tracking(),
        "goal_defined": false,
        "trace_length": 0,
        "allowed_actions": ["session.begin", "session.reset", "session.start_replace"],
        "suggestions": [
            {"command":"session.begin","argv":["podway","begin"]},
            {"command":"session.reset","argv":["podway","reset"]},
            {"command":"session.start_replace","argv":["podway","start","--on-existing","delete"]},
        ],
    }));
    if delegated_placement_count != 0 {
        result.insert(
            "delegated_placement_count".to_owned(),
            json!(delegated_placement_count),
        );
    }
    Ok(result)
}

fn prepared_mutation_templates(scope: &ReadScope<'_>, state: &GraphSessionStateV2) -> Vec<Value> {
    let template = |command: &str, argv: Vec<&str>| {
        json!({
            "command": command,
            "argv": argv,
            "preconditions": {
                "workspace_uuid": scope.identity().workspace_uuid(),
                "session_id": state.trace().session_id(),
                "session_revision": state.trace().revision(),
            },
            "authority": "optimistic_concurrency_only",
            "idempotency_key_required": true,
            "requires_explicit_authorization": false,
        })
    };
    vec![
        template("session.begin", vec!["podway", "begin"]),
        template("session.reset", vec!["podway", "reset"]),
        template(
            "session.start_replace",
            vec!["podway", "start", "--on-existing", "delete"],
        ),
    ]
}

fn terminal_mutation_templates(scope: &ReadScope<'_>, state: &GraphSessionStateV2) -> Vec<Value> {
    let template = |command: &str, argv: Vec<&str>| {
        json!({
            "command": command,
            "argv": argv,
            "preconditions": {
                "workspace_uuid": scope.identity().workspace_uuid(),
                "session_id": state.trace().session_id(),
                "session_revision": state.trace().revision(),
            },
            "authority": "optimistic_concurrency_only",
            "idempotency_key_required": true,
            "requires_explicit_authorization": false,
        })
    };
    if scope.current_terminal_disposition() {
        vec![
            template("session.reset", vec!["podway", "reset"]),
            template("session.start_replace", vec!["podway", "start"]),
        ]
    } else {
        vec![template(
            "session.terminal_disposition",
            vec!["podway", "disposition"],
        )]
    }
}

/// One generation-2 lifecycle template (ADR-0039): the semantic argv first,
/// then the fence tail with current values and the idempotency-key
/// placeholder. The tail equals the R1 correlation exactly, and the revision
/// token is the canonical JSON-number text of the preconditions value.
fn lifecycle_template_v6(
    scope: &ReadScope<'_>,
    state: &GraphSessionStateV2,
    command: &str,
    argv: &[&str],
    requires_explicit_authorization: bool,
) -> Value {
    let revision = state.trace().revision().get();
    let revision_text = revision.to_string();
    let mut fenced = argv.to_vec();
    fenced.extend([
        "--if-workspace-uuid",
        scope.identity().workspace_uuid().as_str(),
        "--if-session-id",
        state.trace().session_id().as_str(),
        "--if-session-revision",
        revision_text.as_str(),
        "--idempotency-key",
        "<idempotency-key>",
    ]);
    json!({
        "command": command,
        "argv": fenced,
        "preconditions": {
            "workspace_uuid": scope.identity().workspace_uuid(),
            "session_id": state.trace().session_id(),
            "session_revision": revision,
        },
        "authority": "optimistic_concurrency_only",
        "idempotency_key_required": true,
        "requires_explicit_authorization": requires_explicit_authorization,
    })
}

fn prepared_mutation_templates_v6(
    scope: &ReadScope<'_>,
    state: &GraphSessionStateV2,
) -> Vec<Value> {
    vec![
        lifecycle_template_v6(scope, state, "session.begin", &["podway", "begin"], false),
        lifecycle_template_v6(scope, state, "session.reset", &["podway", "reset"], true),
        lifecycle_template_v6(
            scope,
            state,
            "session.start_replace",
            &[
                "podway",
                "start",
                "--on-existing",
                "delete",
                "--preset",
                "<preset>",
                "--task",
                "<task>",
            ],
            true,
        ),
        lifecycle_template_v6(
            scope,
            state,
            "session.start_replace",
            &[
                "podway",
                "start",
                "--on-existing",
                "delete",
                "--procedure",
                "<procedure-file>",
                "--expect-procedure-digest",
                "<procedure-digest>",
                "--task",
                "<task>",
            ],
            true,
        ),
    ]
}

fn terminal_mutation_templates_v6(
    scope: &ReadScope<'_>,
    state: &GraphSessionStateV2,
) -> Vec<Value> {
    if scope.current_terminal_disposition() {
        vec![
            lifecycle_template_v6(
                scope,
                state,
                "session.archive",
                &["podway", "archive"],
                true,
            ),
            lifecycle_template_v6(scope, state, "session.reset", &["podway", "reset"], true),
            lifecycle_template_v6(
                scope,
                state,
                "session.start",
                &[
                    "podway", "start", "--preset", "<preset>", "--task", "<task>",
                ],
                true,
            ),
            lifecycle_template_v6(
                scope,
                state,
                "session.start",
                &[
                    "podway",
                    "start",
                    "--procedure",
                    "<procedure-file>",
                    "--expect-procedure-digest",
                    "<procedure-digest>",
                    "--task",
                    "<task>",
                ],
                true,
            ),
        ]
    } else {
        vec![
            lifecycle_template_v6(
                scope,
                state,
                "session.terminal_disposition",
                &[
                    "podway",
                    "disposition",
                    "handed-off",
                    "--summary",
                    "<summary>",
                    "--reference",
                    "<reference>",
                ],
                false,
            ),
            lifecycle_template_v6(
                scope,
                state,
                "session.terminal_disposition",
                &[
                    "podway",
                    "disposition",
                    "not-required",
                    "--reason",
                    "<reason>",
                ],
                false,
            ),
        ]
    }
}
