//! Scope-bound retained definitions, attempts, blockers, and item values.

use super::*;
use podway_config::execution_groups::project_retained_successor_graph;
use podway_core::{
    ArtifactLocationKindV1, ArtifactValueV1, AttemptLifecycle, BlockerState, ItemId,
    MAX_EVIDENCE_PAGE_BYTES_V2, MAX_EVIDENCE_RESPONSE_BYTES_V2, RecordedItemValueV2,
    workflow_v2::item_value_json_v2,
};
use podway_git::LocalPathPresenceV1;
use podway_protocol::serialized_json_len_v1;
use std::path::Path;

const DAG_SCHEMA: &str = "podway.successor-history-dag-fixture-result/v1";
const ATTEMPTS_SCHEMA: &str = "podway.successor-history-attempts-fixture-result/v1";
const BLOCKERS_SCHEMA: &str = "podway.successor-history-blockers-fixture-result/v1";
const PRIOR_WORK_SCHEMA: &str = "podway.successor-history-prior-work-fixture-result/v1";
const ITEM_SCHEMA: &str = "podway.successor-history-item-fixture-result/v1";

/// Scope and immutable digest of a retained definition.
#[derive(Clone, Copy, Debug)]
pub enum SuccessorHistoryDefinition<'a> {
    Base(&'a Sha256Digest),
    Group {
        execution: &'a GroupExecutionIdV1,
        digest: &'a Sha256Digest,
    },
}

impl SuccessorHistoryDefinition<'_> {
    fn resolve(self, state: &ExecutionSessionV3) -> Result<(ExecutionScopeV1, &Sha256Digest)> {
        match self {
            Self::Base(digest) if digest == state.base_digest() => {
                Ok((ExecutionScopeV1::Base, state.base_digest()))
            }
            Self::Base(digest) => {
                if state
                    .group_history()
                    .iter()
                    .any(|group| digest == group.identity().definition_digest())
                {
                    return Err(SuccessorExecutionError::HistoryGroupExecutionRequired);
                }
                Err(SuccessorExecutionError::GroupDefinitionMismatch)
            }
            Self::Group { execution, digest } => {
                let admitted = retained_group(state, execution)?
                    .1
                    .identity()
                    .definition_digest();
                if digest != admitted {
                    return Err(SuccessorExecutionError::GroupDefinitionMismatch);
                }
                Ok((
                    ExecutionScopeV1::Group {
                        group_execution_id: execution.clone(),
                        definition_digest: admitted.clone(),
                    },
                    admitted,
                ))
            }
        }
    }
}

/// List the activation-time applicability judgments of one exact retained group.
pub fn list_successor_prior_work(
    store: &mut impl SuccessorReadStore,
    target: SuccessorHistorySession<'_>,
    execution: &GroupExecutionIdV1,
    definition_digest: &Sha256Digest,
    page: SuccessorHistoryPage<'_>,
) -> Result<CanonicalExecutionJsonV1> {
    page.check_limit()?;
    let snapshot = read_history(store, target)?;
    let state = &snapshot.state;
    let (index, group) = retained_group(state, execution)?;
    let identity = group.identity();
    if identity.definition_digest() != definition_digest {
        return Err(SuccessorExecutionError::GroupDefinitionMismatch);
    }
    let design = state
        .history_attempt(identity.plan().design_attempt())
        .ok_or(SuccessorExecutionError::HistoryAttemptNotFound)?;
    let predecessor = identity
        .predecessor()
        .map(|execution| retained_group(state, execution))
        .transpose()?
        .map(|(_, group)| group_reference(group.identity()));
    let detail = json!({
        "activation_sequence": index + 1,
        "execution": execution,
        "definition_digest": definition_digest,
        "goal_revision": identity.goal_revision().map(GoalRevisionNumberV2::get),
        "design_attempt": design.target(),
        "plan": plan_binding_json(identity.plan()),
        "working_copy_basis": identity.working_copy_basis(),
        "predecessor": predecessor,
    });
    let bounds = HistoryPagination::new(
        target,
        &snapshot,
        page,
        PRIOR_WORK_SCHEMA,
        json!([execution, definition_digest]),
    )?
    .with_response_headroom(store.response_headroom());
    let prior_work = identity.prior_work();
    bounds.project(
        prior_work.len(),
        Some(detail),
        prior_work.iter().map(|work| prior_work_row(state, work)),
    )
}

fn group_reference(identity: &ActivationIdentityV1) -> Value {
    json!({"execution": identity.execution(), "definition_digest": identity.definition_digest()})
}

fn prior_work_row(state: &ExecutionSessionV3, work: &PriorWorkDispositionV1) -> Result<Value> {
    let (_, source_group) = retained_group(state, work.group())?;
    // Snapshot admission has already checked ownership and the immutable value digest.
    let retained_attempt = |id| {
        state
            .history_attempt(id)
            .ok_or(SuccessorExecutionError::HistoryAttemptNotFound)
    };
    let (source_attempt, source_item) = match work.item() {
        None => (None, None),
        Some(HistoricalReferenceV1::Attempt(id)) => (Some(retained_attempt(id)?.target()), None),
        Some(HistoricalReferenceV1::Item(item)) => {
            let attempt = retained_attempt(item.attempt())?;
            let slot = attempt
                .slots()
                .iter()
                .find(|slot| slot.item_id() == item.item())
                .ok_or(SuccessorExecutionError::HistoryItemNotFound)?;
            let binding = json!({
                "item": item.item(), "revision": slot.revision(), "value_digest": item.value_digest(),
            });
            (Some(attempt.target()), Some(binding))
        }
    };
    Ok(json!({
        "choice": work.choice(),
        "rationale": work.rationale(),
        "source": {
            "group": group_reference(source_group.identity()),
            "attempt": source_attempt,
            "item": source_item,
        },
    }))
}

/// Read the canonical graph projection in Unicode-scalar pages.
pub fn read_successor_history_dag(
    store: &mut impl SuccessorReadStore,
    target: SuccessorHistorySession<'_>,
    definition: SuccessorHistoryDefinition<'_>,
    continuation: Option<&str>,
) -> Result<CanonicalExecutionJsonV1> {
    let snapshot = read_history(store, target)?;
    let (scope, digest) = definition.resolve(&snapshot.state)?;
    let source = snapshot
        .sources
        .get(digest)
        .ok_or(SuccessorExecutionError::GroupDefinitionMismatch)?;
    let admitted = admit_canonical_definition(source)?;
    let graph = project_retained_successor_graph(&admitted)?;
    let detail = json!({
        "scope": scope, "definition_digest": digest, "projection_digest": graph.digest(),
        "encoding": "canonical_json",
    });
    let bounds = HistoryPagination::new(
        target,
        &snapshot,
        SuccessorHistoryPage {
            limit: 1,
            continuation,
        },
        DAG_SCHEMA,
        json!([scope, digest, graph.digest()]),
    )?
    .with_response_headroom(store.response_headroom());
    content_page(bounds, detail, HistoryData::String(graph.projection()))
}

/// List retained attempts in original trace order in the exact definition scope.
pub fn list_successor_history_attempts(
    store: &mut impl SuccessorReadStore,
    target: SuccessorHistorySession<'_>,
    definition: SuccessorHistoryDefinition<'_>,
    page: SuccessorHistoryPage<'_>,
) -> Result<CanonicalExecutionJsonV1> {
    page.check_limit()?;
    let snapshot = read_history(store, target)?;
    let (scope, digest) = definition.resolve(&snapshot.state)?;
    let matching = || {
        snapshot
            .state
            .attempt_history()
            .iter()
            .enumerate()
            .filter(|(_, attempt)| attempt.target().scope() == &scope)
    };
    let bounds = HistoryPagination::new(
        target,
        &snapshot,
        page,
        ATTEMPTS_SCHEMA,
        json!([scope, digest]),
    )?
    .with_response_headroom(store.response_headroom());
    bounds.project(
        matching().count(),
        None,
        matching().map(|(index, attempt)| Ok(attempt_history_row(index, attempt))),
    )
}

/// Page the retained blockers of the exact attempt in established Store order.
pub fn list_successor_history_blockers(
    store: &mut impl SuccessorReadStore,
    target: SuccessorHistorySession<'_>,
    source: &ScopedAttemptV1,
    page: SuccessorHistoryPage<'_>,
) -> Result<CanonicalExecutionJsonV1> {
    page.check_limit()?;
    let snapshot = read_history(store, target)?;
    let blockers = exact_attempt(&snapshot.state, source)?.blockers();
    let mut bounds =
        HistoryPagination::new(target, &snapshot, page, BLOCKERS_SCHEMA, json!(source))?
            .with_response_headroom(store.response_headroom());
    bounds.header["attempt"] = json!(source);
    bounds.project(
        blockers.len(),
        None,
        blockers.iter().map(|blocker| {
            Ok(json!({
                "blocker_id": blocker.blocker_id(), "attempt_id": blocker.attempt_id(),
                "state": match blocker.state() {
                    BlockerState::Open => "open",
                    BlockerState::Resolved => "resolved",
                },
                "reason": blocker.reason(), "created_at": blocker.created_at(),
                "resolved_at": blocker.resolved_at(),
            }))
        }),
    )
}

/// Read a declared item of one exactly scoped attempt. Its stored data never becomes current
/// evidence. Artifact availability is an observation of metadata at read time.
pub fn read_successor_history_item(
    store: &mut impl SuccessorReadStore,
    target: SuccessorHistorySession<'_>,
    source: &ScopedAttemptV1,
    item: &ItemId,
    continuation: Option<&str>,
) -> Result<CanonicalExecutionJsonV1> {
    let snapshot = read_history(store, target)?;
    let attempt = exact_attempt(&snapshot.state, source)?;
    let slot = attempt
        .slots()
        .iter()
        .find(|slot| slot.item_id() == item)
        .ok_or(SuccessorExecutionError::HistoryItemNotFound)?;
    let value = slot.value();
    let digest = value
        .map(recorded_value_digest_v2)
        .transpose()
        .map_err(|error| DomainError::InvalidState {
            reason: error.reason(),
        })?;
    let bounds = HistoryPagination::new(
        target,
        &snapshot,
        SuccessorHistoryPage {
            limit: 1,
            continuation,
        },
        ITEM_SCHEMA,
        json!([source, item, slot.revision(), digest]),
    )?
    .with_response_headroom(store.response_headroom());
    let detail = json!({
        "source_attempt": attempt_provenance(attempt),
        "item": item, "item_type": slot.item_type().wire_name(),
        "item_revision": slot.revision(), "value_digest": digest, "recorded": value.is_some(),
        "artifact_availability": value.and_then(RecordedItemValueV2::as_artifact)
            .map(|artifact| artifact_availability(store.workspace_root(), artifact)),
    });
    let data = value
        .map(HistoryData::recorded_value)
        .transpose()?
        .unwrap_or(HistoryData::Absent);
    content_page(bounds, detail, data)
}

// This function lives in successor_execution/history/detail.rs so current and
// retained values use exactly the same bounded scalar/list paginator.
pub fn read_successor_current_evidence(
    store: &mut impl SuccessorReadStore,
    target: SuccessorHistorySession<'_>,
    expected_attempt: Option<&AttemptId>,
    source: &EvidenceSourceV3,
    item: &ItemId,
    continuation: Option<&str>,
) -> Result<CanonicalExecutionJsonV1> {
    const SCHEMA: &str = "podway.scoped-evidence-read-result/v1";
    let snapshot = read_history(store, target)?;
    let state = &snapshot.state;
    let consumer = state
        .active_attempt()
        .ok_or(SuccessorExecutionError::Invalid(
            "current evidence requires a running attempt",
        ))?;
    if let Some(expected) = expected_attempt
        && expected != consumer.target().attempt()
    {
        return Err(DomainError::AttemptNotCurrent {
            expected: expected.clone(),
            actual: Some(consumer.target().attempt().clone()),
        }
        .into());
    }
    let source_selector = match source {
        EvidenceSourceV3::Local(node) => json!({"scope":"local","node":node}),
        EvidenceSourceV3::Plan => json!({"scope":"plan"}),
        EvidenceSourceV3::ExecutionResult => json!({"scope":"execution_result"}),
    };
    let (source_attempt, slot) = state.current_evidence(source, item).ok_or_else(|| {
        SuccessorExecutionError::CurrentEvidenceUnavailable {
            consumer: Box::new(consumer.target().clone()),
            selector: source_selector.clone(),
            item: item.clone(),
        }
    })?;
    let value = slot
        .value()
        .expect("Core current evidence returns a recorded slot");
    let digest = recorded_value_digest_v2(value).map_err(|error| DomainError::InvalidState {
        reason: error.reason(),
    })?;
    let bounds = HistoryPagination::new(
        target,
        &snapshot,
        SuccessorHistoryPage {
            limit: 1,
            continuation,
        },
        SCHEMA,
        json!([
            consumer.target(),
            source_selector,
            source_attempt,
            item,
            slot.revision(),
            digest
        ]),
    )?
    .with_response_headroom(store.response_headroom());
    let detail = json!({
        "consumer": consumer.target(),
        "source": source_selector,
        "source_attempt": source_attempt,
        "item": item,
        "item_type": slot.item_type().wire_name(),
        "item_revision": slot.revision(),
        "value_digest": digest,
        "artifact_availability": value.as_artifact()
            .map(|artifact| artifact_availability(store.workspace_root(), artifact)),
    });
    content_page(bounds, detail, HistoryData::recorded_value(value)?)
}

fn exact_attempt<'a>(
    state: &'a ExecutionSessionV3,
    source: &ScopedAttemptV1,
) -> Result<&'a GroupAttemptStateV1> {
    state
        .history_attempt(source.attempt())
        .filter(|attempt| attempt.target() == source)
        .ok_or(SuccessorExecutionError::HistoryAttemptNotFound)
}

pub(crate) fn attempt_history_row(index: usize, attempt: &GroupAttemptStateV1) -> Value {
    let mut row = attempt_provenance(attempt);
    row["trace_sequence"] = json!(index + 1);
    row["blocker_count"] = json!(attempt.blockers().len());
    row["items"] = json!(
        attempt
            .slots()
            .iter()
            .map(|slot| json!({
                "item": slot.item_id(), "item_type": slot.item_type().wire_name(),
                "revision": slot.revision(), "recorded": slot.value().is_some(),
            }))
            .collect::<Vec<_>>()
    );
    row
}

fn attempt_provenance(attempt: &GroupAttemptStateV1) -> Value {
    let lifecycle = match attempt.lifecycle() {
        AttemptLifecycle::Active => "active",
        AttemptLifecycle::Completed => "completed",
        AttemptLifecycle::Skipped => "skipped",
        AttemptLifecycle::Abandoned => "abandoned",
    };
    json!({
        "attempt": attempt.target(), "lifecycle": lifecycle,
        "validity": if attempt.validity().is_valid() { "valid" } else { "stale" },
        "opened_at": attempt.opened_at(), "closed_at": attempt.closed_at(),
        "reason": attempt.reason().map(|reason| reason.as_str()),
        "actor": attempt.closed_actor().map(|actor| actor.as_str()),
        "decision_entry": attempt.decision_entry(),
        "selected_group": attempt.selected_decision_entry().map(DecisionEntryV1::selected_group),
        "decision": attempt.decision().map(|decision| json!({
            "option": decision.option, "reason": decision.reason,
            "goal_assessment": decision.goal_assessment.map(|(revision, outcome)| json!({
                "revision": revision.get(), "outcome": outcome.as_str(),
            })),
        })),
        "delegation": attempt.delegation(),
    })
}

fn artifact_availability(
    root: &podway_store::ValidatedWorkspaceRootV1,
    artifact: &ArtifactValueV1,
) -> &'static str {
    if artifact.location_kind() != ArtifactLocationKindV1::LocalPath {
        return "unverified";
    }
    match podway_git::observe_local_artifact_presence_v1(
        &root.to_path_buf(),
        Path::new(artifact.location()),
    ) {
        Ok(LocalPathPresenceV1::Present) => "present",
        Ok(LocalPathPresenceV1::Missing) => "missing",
        Err(_) => "unverified",
    }
}

enum HistoryData<'a> {
    Absent,
    String(&'a str),
    List(&'a [String]),
    Scalar(Value),
}

impl<'a> HistoryData<'a> {
    fn recorded_value(value: &'a RecordedItemValueV2) -> Result<Self> {
        if let Some(text) = value.as_text() {
            return Ok(Self::String(text));
        }
        if let Some(entries) = value.as_list() {
            return Ok(Self::List(entries));
        }
        let scalar = item_value_json_v2(value).map_err(|error| DomainError::InvalidState {
            reason: error.reason(),
        })?;
        Ok(Self::Scalar(scalar))
    }
}

fn content_page(
    bounds: HistoryPagination,
    detail: Value,
    data: HistoryData<'_>,
) -> Result<CanonicalExecutionJsonV1> {
    let (total, unit) = match &data {
        HistoryData::String(text) => (text.chars().count(), "unicode_scalar"),
        HistoryData::List(entries) => (entries.len(), "list_entry"),
        HistoryData::Scalar(_) => (1, "value"),
        HistoryData::Absent => (0, "value"),
    };
    let offset = bounds.offset;
    if offset > 0 && offset >= total {
        return Err(SuccessorExecutionError::InvalidHistoryPageToken);
    }
    let (size, chunk) = match data {
        HistoryData::String(text) => string_page(text, offset),
        HistoryData::List(entries) => list_page(entries, offset),
        HistoryData::Scalar(value) => (1, value),
        HistoryData::Absent => (0, Value::Null),
    };
    if (size == 0 && total > offset)
        || serialized_json_len_v1(&chunk).map_err(|_| SuccessorExecutionError::ResponseBudget)?
            > MAX_EVIDENCE_PAGE_BYTES_V2
    {
        return Err(SuccessorExecutionError::ResponseBudget);
    }
    let mut result = bounds.header;
    let next = offset + size;
    result["detail"] = detail;
    result["offset"] = json!(offset);
    result["size"] = json!(size);
    result["total"] = json!(total);
    result["unit"] = json!(unit);
    result["data"] = chunk;
    result["truncated"] = json!(next < total);
    result["next_page_token"] =
        json!((next < total).then(|| history_token(&bounds.identity, next)));
    let projected = canonical_response(&result)?;
    if projected.as_str().len() + bounds.response_headroom > MAX_EVIDENCE_RESPONSE_BYTES_V2 {
        return Err(SuccessorExecutionError::ResponseBudget);
    }
    Ok(projected)
}

fn string_page(text: &str, offset: usize) -> (usize, Value) {
    let start = text
        .char_indices()
        .nth(offset)
        .map_or(text.len(), |(start, _)| start);
    let mut encoded = 2; // JSON string quotes
    let mut count = 0;
    let mut end = start;
    for scalar in text[start..].chars() {
        encoded += encoded_scalar_bytes(scalar);
        if encoded > MAX_EVIDENCE_PAGE_BYTES_V2 {
            break;
        }
        count += 1;
        end += scalar.len_utf8();
    }
    (count, json!(&text[start..end]))
}

fn list_page(entries: &[String], offset: usize) -> (usize, Value) {
    let mut encoded = 2; // JSON array brackets
    let mut count = 0;
    for entry in &entries[offset..] {
        encoded +=
            2 + usize::from(count != 0) + entry.chars().map(encoded_scalar_bytes).sum::<usize>();
        if encoded > MAX_EVIDENCE_PAGE_BYTES_V2 {
            break;
        }
        count += 1;
    }
    (count, json!(&entries[offset..offset + count]))
}

fn encoded_scalar_bytes(scalar: char) -> usize {
    match scalar {
        '\\' | '"' | '\u{0008}' | '\t' | '\n' | '\u{000c}' | '\r' => 2,
        '\u{0000}'..='\u{001f}' => 6,
        _ => scalar.len_utf8(),
    }
}
