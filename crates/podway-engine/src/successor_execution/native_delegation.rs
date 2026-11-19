//! Scoped native delegation packages, structural acceptance, and immutable receipt reads.

use super::*;
use crate::{
    direct_execution::{
        DirectDelegationFailureV1, DirectExecutionErrorV1, DirectReadCommandV1, DirectReadRequestV1,
    },
    execution::ExecutionClockV1,
    workspace::{DirectWorkspaceResolutionErrorV1, DirectWorkspaceResolverV1},
};
use podway_core::{
    MAX_DELEGATION_WORK_PACKAGE_BYTES_V1, RecordedItemValueV2, Sha256Digest,
    recorded_value_digest_v2,
};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{OutputEnvelopeV4, ScopedDelegationAcceptInputV1};
use podway_store::{
    StoreErrorV1,
    v2_delegation::{
        DelegationBindingMismatchReasonV1, acceptance_binding_mismatch_v1,
        accepted_slot_set_digest_v1,
    },
};
use std::time::Instant;

const PACKAGE_SCHEMA: &str = "podway.delegation-work-package/v2";
const PACKAGE_RESULT: &str = "podway.delegation-package-result/v2";
const RECEIPT_SCHEMA: &str = "podway.delegation-receipt/v2";
const RECEIPT_RESULT: &str = "podway.delegation-receipt-result/v2";
const ACCEPT_RESULT: &str = "podway.delegation-accept-result/v3";

type DirectResult<T> = std::result::Result<T, DirectExecutionErrorV1>;

pub(crate) struct DirectDelegationAcceptRequestV1 {
    pub request_id: RequestIdV1,
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub key: IdempotencyKeyV1,
    pub deadline: Instant,
    pub input: ScopedDelegationAcceptInputV1,
}

pub(crate) fn dispatch_successor_delegation_accept<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectDelegationAcceptRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> DirectResult<OutputEnvelopeV4> {
    if request.expected_workspace_id.as_ref() != Some(&request.input.context.workspace_uuid)
        || request.key.as_str() != request.input.context.idempotency_key.as_str()
    {
        return Err(DirectExecutionErrorV1::InvalidRequest);
    }
    let workspace = request.input.context.workspace_uuid;
    let (resolved, store) =
        resolver.resolve_existing(request.selector, Some(&workspace), request.deadline)?;
    let cursor = SuccessorCursor {
        execution: request.input.context.cursor.execution(),
        selected_group: request.input.context.cursor.selected_group,
    };
    let mutation = SuccessorMutationRequest {
        workspace: workspace.clone(),
        key: request.key,
        request_id: request.request_id.clone(),
        cursor,
        response: SuccessorMutationResponseV1::Public {
            return_observation: false,
        },
    };
    let exact = outcome_request(
        &mutation,
        "delegation.accept",
        json!({
            "package_digest": request.input.package_digest,
            "result": request.input.result,
            "accepted": true,
        }),
    )?;
    let committed = store
        .mutate_typed(
            &exact,
            request.deadline,
            || clock.now(),
            |transaction| {
                resolver
                    .revalidate(&resolved, transaction.binding())
                    .map_err(DirectWorkspaceResolutionErrorV1::from)?;
                let (mut state, sources) = transaction
                    .successor_state(admit_candidate_definition)?
                    .ok_or_else(|| {
                        DirectExecutionErrorV1::Domain(DomainError::SessionIdentityMismatch {
                            expected: mutation.cursor.execution.session.clone(),
                            actual: None,
                        })
                    })?;
                verify_cursor(&state, &mutation.cursor)?;
                let active = state.active_attempt().ok_or_else(not_applicable)?;
                let target = active.target().clone();
                if transaction
                    .successor_delegation_receipt(&state, target.attempt())?
                    .is_some()
                {
                    return Err(DirectExecutionErrorV1::Delegation(
                        DirectDelegationFailureV1::AlreadyAccepted,
                    ));
                }
                let metadata = transaction
                    .successor_metadata(target.session())?
                    .ok_or(DirectStoreErrorV1::Unreadable)?;
                let package = package(&workspace, &metadata.task_title, &state, &sources)?;
                if package.digest != request.input.package_digest {
                    return Err(binding_failure(
                        DelegationBindingMismatchReasonV1::PackageMismatch,
                    ));
                }
                request
                    .input
                    .result
                    .validate()
                    .map_err(DirectExecutionErrorV1::Domain)?;
                let submitted = serde_json::to_value(&request.input.result)
                    .map_err(|_| DirectExecutionErrorV1::InvalidRequest)?;
                if let Some(failure) =
                    acceptance_binding_mismatch_v1(&package.body["delegation"], &submitted)
                {
                    return Err(binding_failure(failure));
                }
                state
                    .current_readiness()
                    .map_err(DirectExecutionErrorV1::Domain)?;
                let item_set = accepted_slot_set_digest_v1(active.slots())
                    .map_err(|_| DirectStoreErrorV1::Unreadable)?;
                let binding =
                    ScopedDelegationBindingV1::new(target.clone(), package.digest.clone());
                let now = transaction.now();
                state
                    .accept_delegation(
                        &mutation.cursor.execution,
                        binding.clone(),
                        &package.digest,
                        now,
                    )
                    .map_err(DirectExecutionErrorV1::Domain)?;
                let receipt = ScopedDelegationReceiptV1 {
                    receipt_id: ids.next_delegation_receipt_id(),
                    binding,
                    result: request.input.result,
                    accepted_items_digest: item_set,
                    accepted_at: now,
                };
                transaction.accept_successor_delegation(
                    mutation.cursor.execution.revision,
                    &state,
                    &sources,
                    &receipt,
                )?;
                let output = super::native_lifecycle::lifecycle_output(
                    request.request_id,
                    "delegation.accept",
                    now,
                    transaction.next_workspace_sequence()?,
                    &resolved,
                    Some(DirectSessionOutputV1 {
                        id: target.session().clone(),
                        title: metadata.task_title,
                        lifecycle: state.lifecycle(),
                        revision_before: mutation.cursor.execution.revision,
                        revision_after: state.fence().revision,
                    }),
                    json!({
                        "schema": ACCEPT_RESULT,
                        "target": target,
                        "receipt_id": receipt.receipt_id,
                        "session_revision": state.fence().revision,
                        "package_digest": receipt.binding.package_digest(),
                        "result_digest": receipt.result.result_digest,
                        "acceptance_state": "accepted",
                    }),
                )?;
                super::native_lifecycle::canonical_lifecycle_output(output)
            },
        )
        .map_err(podway_store::direct_store::DirectMutationFailureV1::into_error)?;
    serde_json::from_str(committed.outcome().response().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable.into())
}

pub(crate) fn dispatch_successor_delegation_read<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectReadRequestV1,
    clock: &impl ExecutionClockV1,
) -> DirectResult<OutputEnvelopeV4> {
    resolver.resolve_existing_and_read(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
        |resolved, snapshot| {
            resolver.revalidate(resolved, snapshot.binding()).map_err(DirectWorkspaceResolutionErrorV1::from)?;
            let current = snapshot.successor_state(admit_candidate_definition)?;
            let workspace = snapshot.binding().identity().workspace_uuid();
            let sequence = snapshot.committed_sequence()?;
            let (state, title, result) = match &request.command {
                DirectReadCommandV1::DelegationPackage => {
                    let session = current.as_ref().map(|(state, _)| state.fence().session);
                    if let Some(expected) = request.expected_session_id
                        && session.as_ref() != Some(&expected)
                    {
                        return Err(DirectExecutionErrorV1::Domain(DomainError::SessionIdentityMismatch { expected, actual: session }));
                    }
                    let (state, sources) = current.ok_or_else(not_applicable)?;
                    let title = snapshot.successor_metadata(&state.fence().session)?
                        .ok_or(DirectStoreErrorV1::Unreadable)?.task_title;
                    let package = package(workspace, &title, &state, &sources)?;
                    let body = json!({
                        "schema": PACKAGE_RESULT, "package":package.body, "package_digest":package.digest,
                        "cursor": cursor_json(&SuccessorCursor::from_state(&state)),
                    });
                    (state, title, body)
                }
                DirectReadCommandV1::DelegationReceipt { session_id, attempt_id } => {
                    if let Some(expected) = request.expected_session_id
                        && &expected != session_id
                    {
                        return Err(DirectExecutionErrorV1::Domain(DomainError::SessionIdentityMismatch {
                            expected, actual: Some(session_id.clone()),
                        }));
                    }
                    let (state, _) = match current {
                        Some((state, sources)) if &state.fence().session == session_id => Some((state, sources)),
                        _ => snapshot.archived_successor_state(session_id, admit_candidate_definition)?,
                    }.ok_or_else(|| StoreErrorV1::SessionArchiveNotFoundV1 { session_id: session_id.clone() })?;
                    let title = snapshot.successor_metadata(session_id)?
                        .ok_or(DirectStoreErrorV1::Unreadable)?.task_title;
                    let attempt = state.history_attempt(attempt_id).ok_or(SuccessorExecutionError::HistoryAttemptNotFound)?;
                    let receipt = snapshot.successor_delegation_receipt(&state, attempt_id)?;
                    let applicability = if state.active_attempt().map(GroupAttemptStateV1::target)
                        == Some(attempt.target()) {
                        "current"
                    } else {
                        "historical"
                    };
                    let body = json!({
                        "schema": RECEIPT_RESULT,
                        "target": attempt.target(),
                        "applicability": applicability,
                        "acceptance_state": match (&receipt, attempt.delegation()) {
                            (None, _) => "missing",
                            (Some(_), None) => "stale",
                            (Some(_), Some(_)) => "accepted",
                        },
                        "receipt": receipt.as_ref().map(|receipt| receipt_json(workspace, receipt)).transpose()?,
                    });
                    (state, title, body)
                }
                _ => return Err(DirectExecutionErrorV1::InvalidRequest),
            };
            let fence = state.fence();
            super::native_lifecycle::lifecycle_output(
                request.request_id, request.command.command_name(), clock.now(), sequence, resolved,
                Some(DirectSessionOutputV1 {
                    id: fence.session, title, lifecycle: state.lifecycle(),
                    revision_before: fence.revision, revision_after: fence.revision,
                }), result,
            )
        },
    )
}

fn not_applicable() -> DirectExecutionErrorV1 {
    DirectExecutionErrorV1::Delegation(DirectDelegationFailureV1::NotApplicable)
}

fn binding_failure(reason: DelegationBindingMismatchReasonV1) -> DirectExecutionErrorV1 {
    DirectExecutionErrorV1::Delegation(DirectDelegationFailureV1::BindingMismatch(reason))
}

struct WorkPackage {
    body: Value,
    digest: Sha256Digest,
}

fn package(
    workspace: &WorkspaceId,
    title: &str,
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
) -> DirectResult<WorkPackage> {
    let active = state.active_attempt().ok_or_else(not_applicable)?;
    let placement = state.current_placement().ok_or_else(not_applicable)?;
    if !matches!(
        placement.kind(),
        PlacementKindV3::Action {
            delegation: Some(_),
            ..
        }
    ) {
        return Err(not_applicable());
    }
    let guidance = current_guidance(state, sources, active.target())?;
    let declared = guidance["placement"]["evidence_from"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if declared.len() != placement.evidence().len() {
        return Err(DirectStoreErrorV1::Unreadable.into());
    }
    let mut evidence = Vec::new();
    for (reference, selector) in placement.evidence().iter().zip(declared) {
        let source = state.current_evidence_source(reference.source());
        let mut items = Vec::new();
        if let Some(source) = source {
            for slot in source.slots().iter().filter(|slot| {
                reference.items().is_empty() || reference.items().contains(slot.item_id())
            }) {
                let digest = slot
                    .value()
                    .map(recorded_value_digest_v2)
                    .transpose()
                    .map_err(|_| DirectStoreErrorV1::Unreadable)?;
                items.push(json!({
                    "item": slot.item_id(), "type":slot.item_type().wire_name(), "revision":slot.revision(),
                    "value_digest":digest, "size":slot.value().map(value_size),
                }));
            }
        } else {
            items.extend(reference.items().iter().map(|item| {
                json!({
                   "item":item, "type":null, "revision":null, "value_digest":null, "size":null,
                })
            }));
        }
        let item_set_digest = source
            .map(|attempt| accepted_slot_set_digest_v1(attempt.slots()))
            .transpose()
            .map_err(|_| DirectStoreErrorV1::Unreadable)?;
        evidence.push(json!({
            "selector":selector,
            "resolution": if source.is_some() { "resolved" } else { "unresolved" },
            "source":source.map(GroupAttemptStateV1::target),
            "item_set_digest":item_set_digest,
            "items":items,
        }));
    }
    let goal = state.current_goal().map(|goal| {
        json!({
            "revision":state.fence().goal_revision.map(GoalRevisionNumberV2::get),
            "statement":goal.statement.as_str(),
            "criteria":goal.criteria.criteria().iter().map(|criterion| json!({
                "id":criterion.id(),"statement":criterion.statement(),
            })).collect::<Vec<_>>(),
        })
    });
    let body = json!({
        "schema":PACKAGE_SCHEMA, "workspace_uuid":workspace, "task_title":title,
        "target":active.target(), "selected_group":selected_group_id(state),
        "delegation":guidance["placement"]["delegation"], "action":guidance["definition"],
        "goal":goal, "evidence":evidence,
    });
    let canonical =
        canonicalize_json_v1(&body).map_err(|_| SuccessorExecutionError::ResponseBudget)?;
    if canonical.len() > MAX_DELEGATION_WORK_PACKAGE_BYTES_V1 {
        return Err(SuccessorExecutionError::ResponseBudget.into());
    }
    let digest = canonical_digest(&format!("{PACKAGE_SCHEMA}\n{canonical}"))?;
    Ok(WorkPackage { body, digest })
}

fn value_size(value: &RecordedItemValueV2) -> Value {
    if let Some(text) = value.as_text() {
        return json!({"amount":text.chars().count(),"unit":"unicode_scalar"});
    }
    if let Some(list) = value.as_list() {
        return json!({"amount":list.len(),"unit":"list_entry"});
    }
    if let Some(artifact) = value.as_artifact() {
        return json!({"amount":artifact.size_bytes(),"unit":"byte"});
    }
    json!({"amount":1,"unit":"value"})
}

fn receipt_json(
    workspace: &WorkspaceId,
    receipt: &ScopedDelegationReceiptV1,
) -> DirectResult<Value> {
    let mut record = serde_json::to_value(receipt).map_err(|_| DirectStoreErrorV1::Unreadable)?;
    record["schema"] = json!(RECEIPT_SCHEMA);
    record["workspace_uuid"] = json!(workspace);
    Ok(record)
}
