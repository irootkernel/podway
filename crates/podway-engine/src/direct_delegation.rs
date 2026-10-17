//! Delegated acceptance binds the current package, declaration and item values atomically.

use std::time::Instant;

use podway_core::{SessionLifecycle, Sha256Digest, WorkspaceId};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{
    DelegationAcceptV1, OutputEnvelopeV4, RequestIdV1, Rfc3339MillisV1, SliceCommandV1,
    canonical_direct_slice_mutation_identity_v1, validate_delegation_document_v1,
};
use podway_store::{
    DelegationReceiptV1, GraphMutationErrorV2, GraphWorkspaceViewV2, IdempotencyKeyV1,
    direct_store::{DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1},
    v2_delegation::{
        DelegationBindingMismatchReasonV1, DelegationReadinessV1, acceptance_binding_mismatch_v1,
        accepted_items_digest_v1,
    },
};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::{
    direct_execution::{
        DirectDelegationFailureV1 as Failure, DirectExecutionErrorV1, DirectIdSourceV1,
    },
    direct_mutation::{check_attempt_fence, check_session_fences, graph_success},
    execution::ExecutionClockV1,
    v2_read_service::{GraphViewErrorV2, delegation_work_package_from_view_v1},
    workspace::DirectWorkspaceResolverV1,
};

#[derive(Clone, Debug)]
pub struct DirectDelegationRequestV1 {
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub request_id: RequestIdV1,
    pub key: IdempotencyKeyV1,
    pub input: DelegationAcceptV1,
    pub deadline: Instant,
}

pub fn accept_delegation_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectDelegationRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let (resolved, store) = resolver.resolve_existing(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    let command = SliceCommandV1::DelegationAccept(request.input.clone());
    let canonical = canonical_direct_slice_mutation_identity_v1(
        &command,
        resolved.store_identity().workspace_uuid(),
    )
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "invalid delegation semantic identity",
    })?;
    let digest = Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
        .expect("SHA-256 produces a canonical digest");
    let fences = &request.input.preconditions;
    let mutation = DirectMutationRequestV1::new(
        request.key.clone(),
        digest,
        command.command_name(),
        request.request_id.as_str(),
        DirectOutcomeScopeV1::Session(fences.expected_session_id.clone()),
    )?;
    let prepared = store.prepare_graph(request.deadline)?;
    let result = store.mutate_typed(&mutation, request.deadline, || clock.now(), |transaction| -> Result<_, DirectExecutionErrorV1> {
        let current = transaction.use_prepared_graph(prepared.as_ref())?;
        let current = current.as_ref().ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        let prior = current.as_ref();
        check_session_fences(prior, &fences.expected_session_id, fences.expected_session_revision)?;
        check_attempt_fence(prior, &fences.expected_attempt_id)?;
        if prior.trace().lifecycle() != SessionLifecycle::Running { return Err(GraphMutationErrorV2::SessionNotRunning.into()); }
        resolver.revalidate(&resolved, transaction.binding()).map_err(DirectExecutionErrorV1::from)?;
        let input = &request.input.input;
        if !validate_delegation_document_v1("podway.delegation-accept-input/v1", input)
            || input["workspace_uuid"].as_str() != Some(transaction.binding().identity().workspace_uuid().as_str())
            || input["session_id"].as_str() != Some(fences.expected_session_id.as_str())
            || input["session_revision"].as_u64() != Some(fences.expected_session_revision.get())
            || input["attempt_id"].as_str() != Some(fences.expected_attempt_id.as_str())
            || input["idempotency_key"].as_str() != Some(request.key.as_str()) {
            return Err(DirectStoreErrorV1::InvalidInput { reason: "invalid delegation acceptance input or fences" }.into());
        }
        let receipts = transaction.delegation_receipts()?;
        let view = GraphWorkspaceViewV2::new(transaction.binding().identity().clone(), Some(prior.clone()),
            transaction.next_workspace_sequence()? - 1, prior.created_at())
            .with_delegation_receipts(receipts.clone());
        let package = delegation_work_package_from_view_v1(&view).map_err(|error| match error {
                GraphViewErrorV2::DelegationNotApplicable => DirectExecutionErrorV1::Delegation(Failure::NotApplicable),
                other => DirectExecutionErrorV1::View(other),
            })?;
        let failure = if receipts.readiness() != DelegationReadinessV1::ReceiptMissing {
            Some(Failure::AlreadyAccepted)
        } else if input["package_digest"].as_str() != Some(package.digest().as_str()) {
            Some(Failure::BindingMismatch(DelegationBindingMismatchReasonV1::PackageMismatch))
        } else {
            acceptance_binding_mismatch_v1(&package.package()["delegation"], input).map(Failure::BindingMismatch)
        };
        if let Some(failure) = failure {
            return Err(DirectExecutionErrorV1::Delegation(failure));
        }
        // Readiness and immutable input checks precede both identity generation and the clock.
        let next = prior.accept_delegation_successor_v1(&fences.expected_attempt_id)?;
        let items_digest = accepted_items_digest_v1(prior)
            .map_err(|_| DirectStoreErrorV1::Unreadable)?;
        let receipt_id = ids.next_delegation_receipt_id();
        let now = transaction.now();
        let receipt = DelegationReceiptV1::new(json!({"schema":"podway.delegation-receipt/v1",
            "receipt_id":receipt_id, "workspace_uuid":transaction.binding().identity().workspace_uuid(),
            "procedure_digest":prior.snapshot().digest(), "session_id":fences.expected_session_id,
            "placement_id":prior.trace().active_attempt().expect("attempt fence validated").graph_node_id(),
            "attempt_id":fences.expected_attempt_id, "delegation":package.package()["delegation"],
            "package_digest":package.digest(), "execution":input["execution"], "rounds_used":input["rounds_used"],
            "result_digest":input["result_digest"], "summary":input["summary"], "worker_disposition":input["worker_disposition"],
            "accepted":true, "accepted_items_digest":items_digest, "accepted_at":Rfc3339MillisV1::from_unix_millis(now.get())
                .map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "clock exceeds the public timestamp range" })? }))?;
        let body = json!({"schema":"podway.delegation-accept-result/v2", "receipt_id":receipt_id,
            "workspace_uuid":transaction.binding().identity().workspace_uuid(), "session_id":fences.expected_session_id,
            "session_revision":next.trace().revision(), "attempt_id":fences.expected_attempt_id,
            "package_digest":package.digest(), "result_digest":input["result_digest"], "acceptance_state":"accepted"});
        let response = graph_success(transaction, &resolved, request.request_id, command.command_name(), prior, &next,
            body.as_object().expect("delegation result is an object").clone())?;
        transaction.accept_delegation(prior.workspace_revision(), prior.trace().revision(), &next, package.digest(), &receipt)?;
        Ok(response)
    });
    let result = result.map_err(podway_store::direct_store::DirectMutationFailureV1::into_error)?;
    serde_json::from_str(result.outcome().response().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable.into())
}
