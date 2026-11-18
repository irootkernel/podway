//! Current-only candidate projections and scoped outcome recovery.

use super::*;
use podway_core::{
    BlockerState, ItemId, RecordedItemValueV2, SessionLifecycle, recorded_value_digest_v2,
};
use podway_store::direct_store::DirectRecordedOutcomeV1;

#[derive(Debug)]
pub struct SuccessorObservation {
    pub cursor: SuccessorCursor,
    pub workspace_sequence: u64,
    pub projection: CanonicalExecutionJsonV1,
}

pub fn observe_successor(
    store: &mut CandidateExecutionStore,
    workspace: &WorkspaceId,
    session: &SessionId,
) -> Result<SuccessorObservation> {
    let (state, sources, sequence) =
        store.current_snapshot(workspace, session, admit_candidate_definition)?;
    let view = observation(workspace, &state, &sources, sequence)?;
    let projection = canonical_response(&view)?;
    Ok(SuccessorObservation {
        cursor: SuccessorCursor::from_state(&state),
        workspace_sequence: sequence,
        projection,
    })
}

pub fn lookup_successor_outcome(
    store: &CandidateExecutionStore,
    workspace: &WorkspaceId,
    key: &IdempotencyKeyV1,
) -> Result<Option<DirectRecordedOutcomeV1>> {
    Ok(store.lookup_outcome(workspace, key)?)
}

#[derive(Debug)]
pub struct CurrentItemEvidence {
    pub source: ScopedAttemptV1,
    pub item: PlanItemBindingV1,
    pub value: RecordedItemValueV2,
}

pub fn read_current_evidence(
    store: &mut CandidateExecutionStore,
    workspace: &WorkspaceId,
    cursor: &SuccessorCursor,
    source: &EvidenceSourceV3,
    item: &ItemId,
) -> Result<Option<CurrentItemEvidence>> {
    let state = state_at_cursor(store, workspace, cursor)?;
    let Some((target, slot)) = state.current_evidence(source, item) else {
        return Ok(None);
    };
    let value = slot.value().expect("current evidence has a recorded value");
    let digest = recorded_value_digest_v2(value).map_err(|error| DomainError::InvalidState {
        reason: error.reason(),
    })?;
    Ok(Some(CurrentItemEvidence {
        source: target.clone(),
        item: PlanItemBindingV1::new(slot.item_id().clone(), slot.revision(), digest)?,
        value: value.clone(),
    }))
}

/// Obtain the private package binding for the requested, current opted-in Action.
pub fn current_delegation_package(
    store: &mut CandidateExecutionStore,
    workspace: &WorkspaceId,
    cursor: &SuccessorCursor,
) -> Result<ScopedDelegationBindingV1> {
    let state = state_at_cursor(store, workspace, cursor)?;
    delegation_package(workspace, &state)
}

/// Read the exact design item-set binding for a fresh activation request.
pub fn current_plan_binding(
    store: &mut CandidateExecutionStore,
    workspace: &WorkspaceId,
    cursor: &SuccessorCursor,
) -> Result<PlanBindingV1> {
    Ok(state_at_cursor(store, workspace, cursor)?.plan_binding()?)
}

fn state_at_cursor(
    store: &mut CandidateExecutionStore,
    workspace: &WorkspaceId,
    cursor: &SuccessorCursor,
) -> Result<ExecutionSessionV3> {
    let (state, _, _) = store.current_snapshot(
        workspace,
        &cursor.execution.session,
        admit_candidate_definition,
    )?;
    verify_cursor(&state, cursor)?;
    Ok(state)
}

pub(super) fn observation(
    workspace: &WorkspaceId,
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
    sequence: u64,
) -> Result<Value> {
    let cursor = SuccessorCursor::from_state(state);
    let selection = state
        .selected()
        .map(|group| {
            let definition = state
                .group_definition(group.identity().definition_digest())
                .ok_or(SuccessorExecutionError::Invalid(
                    "candidate selected definition is absent",
                ))?;
            Ok::<_, SuccessorExecutionError>((definition, group.identity().execution()))
        })
        .transpose()?;
    let guidance = state
        .active_attempt()
        .map(|active| current_guidance(state, sources, active.target()))
        .transpose()?;
    let active = state.active_attempt();
    let ordinal = active.and_then(GroupAttemptStateV1::decision_entry);
    let item_revisions = active.map(|attempt| {
        attempt.slots().iter().map(|slot| json!({
        "item": slot.item_id(), "revision": slot.revision(), "recorded": slot.value().is_some(),
    })).collect::<Vec<_>>()
    });
    let open_blockers = active.map(|attempt| {
        attempt
            .blockers()
            .iter()
            .filter(|blocker| blocker.state() == BlockerState::Open)
            .map(|blocker| blocker.blocker_id())
            .collect::<Vec<_>>()
    });
    let lifecycle = match state.lifecycle() {
        SessionLifecycle::Prepared => "prepared",
        SessionLifecycle::Running => "running",
        SessionLifecycle::Completed => "completed",
        SessionLifecycle::Cancelled => "cancelled",
    };
    Ok(json!({
        "schema": "podway.successor-observation-fixture-result/v1",
        "workspace": workspace,
        "workspace_sequence": sequence,
        "cursor": cursor_json(&cursor),
        "lifecycle": lifecycle,
        "decision_entry": ordinal,
        "next_decision_entry": state.upcoming_decision_entry(),
        "counters": combined_counters(state, selection),
        "guidance": guidance,
        "items": item_revisions,
        "open_blockers": open_blockers,
        "delegation": active.and_then(GroupAttemptStateV1::delegation),
        "retained_group_count": state.group_history().len(),
        "remaining_activations": MAX_GROUP_EXECUTIONS_V1 - state.group_history().len(),
        "remaining_attempts": MAX_SESSION_ATTEMPTS_V1 - state.attempt_history().len(),
    }))
}

pub(super) fn delegation_package(
    workspace: &WorkspaceId,
    state: &ExecutionSessionV3,
) -> Result<ScopedDelegationBindingV1> {
    let placement = state
        .current_placement()
        .ok_or(SuccessorExecutionError::Invalid(
            "candidate delegation requires a current placement",
        ))?;
    let PlacementKindV3::Action {
        delegation: Some(declaration),
        ..
    } = placement.kind()
    else {
        return Err(SuccessorExecutionError::Invalid(
            "candidate placement does not declare delegation",
        ));
    };
    let attempt = state
        .active_attempt()
        .expect("current placement has an active attempt");
    let slots = item_bindings(attempt.slots().iter())?;
    let mut evidence = Vec::with_capacity(placement.evidence().len());
    for reference in placement.evidence() {
        let mut items = Vec::new();
        let source = state.current_evidence_source(reference.source());
        if let Some(source) = source {
            let allowed = source.slots().iter().filter(|slot| {
                reference.items().is_empty() || reference.items().contains(slot.item_id())
            });
            items = item_bindings(allowed)?;
        }
        evidence.push(json!({"source": source.map(GroupAttemptStateV1::target), "items": items}));
    }
    let package = canonicalize_json_v1(&json!({
        "schema": "podway.successor-delegation-package-fixture/v1",
        "workspace": workspace,
        "target": attempt.target(),
        "selected_group": selected_group_id(state),
        "goal_revision": state.fence().goal_revision.map(|revision| revision.get()),
        "role_snapshot_digest": declaration.role_snapshot_digest(),
        "evidence": evidence,
        "current_items": slots,
    }))
    .map_err(|_| SuccessorExecutionError::ResponseBudget)?;
    if package.len() > podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1 {
        return Err(SuccessorExecutionError::ResponseBudget);
    }
    Ok(ScopedDelegationBindingV1::new(
        attempt.target().clone(),
        canonical_digest(&package)?,
    ))
}

fn item_bindings<'a>(
    slots: impl Iterator<Item = &'a podway_core::ItemSlotStateV2>,
) -> Result<Vec<Value>> {
    slots
        .map(|slot| {
            let digest = slot
                .value()
                .map(recorded_value_digest_v2)
                .transpose()
                .map_err(|error| DomainError::InvalidState {
                    reason: error.reason(),
                })?;
            Ok(json!({"item": slot.item_id(), "revision": slot.revision(), "value_digest": digest}))
        })
        .collect()
}

pub(super) fn verify_delegated_completion(
    workspace: &WorkspaceId,
    state: &ExecutionSessionV3,
) -> Result<()> {
    if let Some(accepted) = state
        .active_attempt()
        .and_then(GroupAttemptStateV1::delegation)
        && accepted != &delegation_package(workspace, state)?
    {
        return Err(SuccessorExecutionError::Invalid(
            "candidate delegated acceptance is no longer current",
        ));
    }
    Ok(())
}
