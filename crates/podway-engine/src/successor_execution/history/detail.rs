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
            Self::Base(_) => Err(SuccessorExecutionError::HistoryDefinitionMismatch),
            Self::Group { execution, digest } => {
                let admitted = state
                    .group_history()
                    .iter()
                    .find(|group| group.identity().execution() == execution)
                    .ok_or(SuccessorExecutionError::HistoryGroupNotFound)?
                    .identity()
                    .definition_digest();
                if digest != admitted {
                    return Err(SuccessorExecutionError::HistoryDefinitionMismatch);
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

/// Read the canonical graph projection in Unicode-scalar pages.
pub fn read_successor_history_dag(
    store: &mut CandidateExecutionStore,
    target: SuccessorHistorySession<'_>,
    definition: SuccessorHistoryDefinition<'_>,
    continuation: Option<&str>,
) -> Result<CanonicalExecutionJsonV1> {
    let (state, sources, _) = store.history_snapshot(
        target.workspace,
        target.session,
        target.location,
        admit_candidate_definition,
    )?;
    let (scope, digest) = definition.resolve(&state)?;
    let source = sources
        .get(digest)
        .ok_or(SuccessorExecutionError::HistoryDefinitionMismatch)?;
    let admitted = admit_canonical_definition(source)?;
    let graph = project_retained_successor_graph(&admitted)?;
    let detail = json!({
        "scope": scope, "definition_digest": digest, "projection_digest": graph.digest(),
        "encoding": "canonical_json",
    });
    let bounds = HistoryPagination::new(
        target,
        &state,
        SuccessorHistoryPage {
            limit: 1,
            continuation,
        },
        DAG_SCHEMA,
        json!([scope, digest, graph.digest()]),
    )?;
    content_page(bounds, detail, HistoryData::String(graph.projection()))
}

/// List retained attempts in original trace order in the exact definition scope.
pub fn list_successor_history_attempts(
    store: &mut CandidateExecutionStore,
    target: SuccessorHistorySession<'_>,
    definition: SuccessorHistoryDefinition<'_>,
    page: SuccessorHistoryPage<'_>,
) -> Result<CanonicalExecutionJsonV1> {
    page.check_limit()?;
    let state = read_state(store, target)?;
    let (scope, digest) = definition.resolve(&state)?;
    let matching = || {
        state
            .attempt_history()
            .iter()
            .enumerate()
            .filter(|(_, attempt)| attempt.target().scope() == &scope)
    };
    let bounds = HistoryPagination::new(
        target,
        &state,
        page,
        ATTEMPTS_SCHEMA,
        json!([scope, digest]),
    )?;
    bounds.project(
        matching().count(),
        None,
        matching().map(|(index, attempt)| {
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
            Ok(row)
        }),
    )
}

/// Page the retained blockers of the exact attempt in established Store order.
pub fn list_successor_history_blockers(
    store: &mut CandidateExecutionStore,
    target: SuccessorHistorySession<'_>,
    source: &ScopedAttemptV1,
    page: SuccessorHistoryPage<'_>,
) -> Result<CanonicalExecutionJsonV1> {
    page.check_limit()?;
    let state = read_state(store, target)?;
    let blockers = exact_attempt(&state, source)?.blockers();
    let mut bounds = HistoryPagination::new(target, &state, page, BLOCKERS_SCHEMA, json!(source))?;
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
    store: &mut CandidateExecutionStore,
    target: SuccessorHistorySession<'_>,
    source: &ScopedAttemptV1,
    item: &ItemId,
    continuation: Option<&str>,
) -> Result<CanonicalExecutionJsonV1> {
    let state = read_state(store, target)?;
    let attempt = exact_attempt(&state, source)?;
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
        &state,
        SuccessorHistoryPage {
            limit: 1,
            continuation,
        },
        ITEM_SCHEMA,
        json!([source, item, slot.revision(), digest]),
    )?;
    let detail = json!({
        "source_attempt": attempt_provenance(attempt),
        "item": item, "item_type": slot.item_type().wire_name(),
        "item_revision": slot.revision(), "value_digest": digest, "recorded": value.is_some(),
        "artifact_availability": value.and_then(RecordedItemValueV2::as_artifact)
            .map(|artifact| artifact_availability(store.workspace_root(), artifact)),
    });
    let data = match value {
        None => HistoryData::Absent,
        Some(value) => {
            if let Some(text) = value.as_text() {
                HistoryData::String(text)
            } else if let Some(entries) = value.as_list() {
                HistoryData::List(entries)
            } else {
                HistoryData::Scalar(item_value_json_v2(value).map_err(|error| {
                    DomainError::InvalidState {
                        reason: error.reason(),
                    }
                })?)
            }
        }
    };
    content_page(bounds, detail, data)
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
    match podway_git::test_support::observe_candidate_artifact_presence(
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
    if projected.as_str().len() > MAX_EVIDENCE_RESPONSE_BYTES_V2 {
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
