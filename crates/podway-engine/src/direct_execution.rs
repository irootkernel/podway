//! In-process command boundaries. No job, IPC, registry or runtime-mode authority.

use sha2::{Digest as _, Sha256};
use std::time::Instant;

use podway_core::{
    ActorAttributionV2, AttemptId, BlockerId, DelegationReceiptIdV1, DomainError, GoalStatementV2,
    GraphNodeId, ItemId, ProcedureSnapshotId, ReasonV2, SessionId, SessionLifecycle, Sha256Digest,
    TraceSequenceV2, WorkspaceId, canonicalize_json_v1,
};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{
    CommandNameV1, DirectSessionOutputV1, EvidencePageTokenV1, OutputEnvelopeInputV4,
    OutputEnvelopeV4, ProcedureV2MutationCommandV1, ProtocolError, RequestIdV1, Rfc3339MillisV1,
    SessionBeginV1, SessionBlockV1, SessionCancelV1, SessionCompleteV1,
    SessionMutationPreconditionsWireV1, SessionRetryV1, SessionSkipV1, SessionUnblockV1,
    SliceCommandV1, WorkspaceOutputV1, canonical_direct_slice_mutation_identity_v1,
    canonical_procedure_v2_command_identity_v1,
};
use podway_store::{
    CanonicalExecutionJsonV1, GraphInitialGoalV2, GraphMutationErrorV2, IdempotencyKeyV1,
    direct_store::{DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1},
};

use crate::{
    direct_read_service::{
        project_direct_delegation_package_v1, project_direct_evidence_v1, project_direct_next_v1,
        project_direct_observation_v1, project_direct_status_v1,
    },
    execution::{
        ExecutionBoundaryErrorV1, ExecutionClockV1, ExecutionErrorV1, goal_definition_from_wire_v2,
    },
    v2_read_service::{EvidenceReadErrorV2, GraphStatusTierV2, GraphViewErrorV2},
    workspace::{DirectWorkspaceResolutionErrorV1, DirectWorkspaceResolverV1},
};

/// Generated domain identities do not depend on a removed job identity.
pub trait DirectIdSourceV1: Send + Sync {
    fn next_workspace_id(&self) -> WorkspaceId;
    fn next_session_id(&self) -> SessionId;
    fn next_attempt_id(&self) -> AttemptId;
    fn next_blocker_id(&self) -> BlockerId;
    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId;
    fn next_delegation_receipt_id(&self) -> DelegationReceiptIdV1;
}

#[derive(Clone, Debug)]
pub enum DirectCursorCommandV1 {
    Complete(SessionCompleteV1),
    Skip(SessionSkipV1),
    Block(SessionBlockV1),
    Unblock(SessionUnblockV1),
    Cancel(SessionCancelV1),
    Retry(SessionRetryV1),
}

impl DirectCursorCommandV1 {
    pub(crate) fn slice_command(&self) -> SliceCommandV1 {
        match self {
            Self::Complete(input) => SliceCommandV1::SessionComplete(input.clone()),
            Self::Skip(input) => SliceCommandV1::SessionSkip(input.clone()),
            Self::Block(input) => SliceCommandV1::SessionBlock(input.clone()),
            Self::Unblock(input) => SliceCommandV1::SessionUnblock(input.clone()),
            Self::Cancel(input) => SliceCommandV1::SessionCancel(input.clone()),
            Self::Retry(input) => SliceCommandV1::SessionRetry(input.clone()),
        }
    }

    pub(crate) fn preconditions(&self) -> &SessionMutationPreconditionsWireV1 {
        match self {
            Self::Complete(input) => &input.preconditions,
            Self::Skip(input) => &input.preconditions,
            Self::Block(input) => &input.preconditions,
            Self::Unblock(input) => &input.preconditions,
            Self::Cancel(input) => &input.preconditions,
            Self::Retry(input) => &input.preconditions,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirectCursorRequestV1 {
    pub request_id: RequestIdV1,
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub key: IdempotencyKeyV1,
    pub command: DirectCursorCommandV1,
    pub deadline: Instant,
}

pub fn execute_cursor_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectCursorRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let (resolved, store, binding) = resolver.resolve_existing_with_binding(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    let command = request.command.slice_command();
    let canonical = canonical_direct_slice_mutation_identity_v1(
        &command,
        resolved.store_identity().workspace_uuid(),
    )
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "invalid cursor semantic identity",
    })?;
    let digest = Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
        .expect("SHA-256 produces a canonical digest");
    let fences = request.command.preconditions();
    let mutation = DirectMutationRequestV1::new(
        request.key,
        digest,
        command.command_name(),
        request.request_id.as_str(),
        DirectOutcomeScopeV1::Session(fences.expected_session_id.clone()),
    )?;
    let prepared = store.prepare_graph(request.deadline)?;
    let native = resolver.native_workspace(resolved.clone());
    let artifacts = if matches!(request.command, DirectCursorCommandV1::Complete(_)) {
        prepared
            .as_ref()
            .map(|prior| native.prepare_required_artifacts_v2(&binding, prior.graph()))
            .transpose()
            .map(|artifacts| artifacts.unwrap_or_default())
    } else {
        Ok(Vec::new())
    };
    let mut execution_error = None;
    let result = store.mutate(&mutation, request.deadline, || clock.now(), |transaction| {
        let current = transaction.use_prepared_graph(prepared.as_ref())?;
        let current = current.as_ref().ok_or_else(|| DirectStoreErrorV1::Domain(
            GraphMutationErrorV2::Domain(DomainError::SessionIdentityMismatch {
                expected: fences.expected_session_id.clone(), actual: None,
            })))?;
        let prior = current.as_ref();
        if prior.trace().session_id() != &fences.expected_session_id {
            return Err(GraphMutationErrorV2::Domain(DomainError::SessionIdentityMismatch {
                expected: fences.expected_session_id.clone(), actual: Some(prior.trace().session_id().clone()),
            }).into());
        }
        if prior.trace().revision() != fences.expected_session_revision {
            return Err(GraphMutationErrorV2::SessionRevisionConflict {
                expected: fences.expected_session_revision, actual: prior.trace().revision(),
            }.into());
        }
        if prior.trace().lifecycle() != SessionLifecycle::Running {
            return Err(GraphMutationErrorV2::SessionNotRunning.into());
        }
        let active = prior.trace().active_attempt().ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        if active.attempt_id() != &fences.expected_attempt_id {
            return Err(GraphMutationErrorV2::AttemptNotCurrent {
                expected: fences.expected_attempt_id.clone(), actual: Some(active.attempt_id().clone()),
            }.into());
        }
        resolver.revalidate(&resolved, transaction.binding()).map_err(|error| {
            execution_error = Some(DirectExecutionErrorV1::from(error));
            DirectStoreErrorV1::InvalidInput { reason: "workspace revalidation failed" }
        })?;
        if matches!(request.command, DirectCursorCommandV1::Complete(_)) {
            use podway_store::v2_delegation::DelegationReadinessV1;
            let failure = match transaction.delegation_receipts()?.readiness() {
                DelegationReadinessV1::Direct | DelegationReadinessV1::ReceiptCurrent => None,
                DelegationReadinessV1::ReceiptMissing => Some(DirectDelegationFailureV1::ReceiptRequired),
                DelegationReadinessV1::PackageChanged => Some(DirectDelegationFailureV1::PackageChanged),
                DelegationReadinessV1::ItemsChanged => Some(DirectDelegationFailureV1::ItemsChanged),
            };
            if let Some(failure) = failure {
                execution_error = Some(DirectExecutionErrorV1::Delegation(failure));
                return Err(DirectStoreErrorV1::InvalidInput { reason: "delegation is not ready for completion" });
            }
            let artifacts = artifacts.map_err(|error| {
                execution_error = Some(error);
                DirectStoreErrorV1::InvalidInput { reason: "required artifact preparation failed" }
            })?;
            if let Err(error) = native.verify_required_artifacts_v2(transaction.binding(), prior, &artifacts) {
                execution_error = Some(error);
                return Err(DirectStoreErrorV1::InvalidInput { reason: "required artifact changed" });
            }
        }
        let (next, mut body) = match &request.command {
            DirectCursorCommandV1::Complete(_) => {
                let target = action_successor(prior, active.graph_node_id())?;
                let fresh = target.as_ref().map(|_| ids.next_attempt_id());
                let outcome = prior.complete_active_action_v2(fences.expected_session_revision,
                    &fences.expected_attempt_id, fresh.clone(), transaction.now())?;
                let mut body = serde_json::json!({"transition":"complete", "session_state":
                    if target.is_some() { "running" } else { "completed" }});
                if let Some(target) = target { body["to_graph_node_id"] = serde_json::json!(target); body["to_attempt_id"] = serde_json::json!(fresh); }
                (outcome.into_state(), body)
            }
            DirectCursorCommandV1::Skip(input) => {
                let reason = input.reason.clone().map(ReasonV2::new).transpose().map_err(GraphMutationErrorV2::Domain)?;
                let target = action_successor(prior, active.graph_node_id())?;
                let fresh = target.as_ref().map(|_| ids.next_attempt_id());
                let outcome = prior.skip_active_action_v2(fences.expected_session_revision,
                    &fences.expected_attempt_id, fresh.clone(), reason, transaction.now())?;
                let mut body = serde_json::json!({"transition":"skip", "session_state":
                    if target.is_some() { "running" } else { "completed" }});
                if let Some(target) = target { body["to_graph_node_id"] = serde_json::json!(target); body["to_attempt_id"] = serde_json::json!(fresh); }
                if let Some(reason) = &input.reason { body["reason"] = serde_json::json!(reason); }
                (outcome.into_state(), body)
            }
            DirectCursorCommandV1::Cancel(input) => {
                let reason = ReasonV2::new(input.reason.clone()).map_err(GraphMutationErrorV2::Domain)?;
                let outcome = prior.cancel_active_session_v2(fences.expected_session_revision,
                    &fences.expected_attempt_id, reason, transaction.now())?;
                let body = serde_json::json!({"transition":"cancel", "session_state":"cancelled"});
                (outcome.into_state(), body)
            }
            DirectCursorCommandV1::Block(input) => {
                // The block-specific bound is tighter than a general transition reason.
                if input.reason.is_empty() || input.reason.chars().count() > 1000 {
                    return Err(DirectStoreErrorV1::InvalidInput { reason: "invalid block reason" });
                }
                let blocker = ids.next_blocker_id();
                let outcome = prior.block_active_attempt_v2(fences.expected_session_revision,
                    &fences.expected_attempt_id, blocker.clone(), input.reason.clone(), transaction.now())?;
                let body = serde_json::json!({"transition":"block", "session_state":"running",
                    "blocker_id":blocker, "reason":input.reason});
                (outcome.into_state(), body)
            }
            DirectCursorCommandV1::Unblock(input) => {
                if input.all == input.blocker_id.is_some() {
                    return Err(DirectStoreErrorV1::InvalidInput { reason: "unblock requires a blocker or all" });
                }
                let outcome = prior.unblock_active_attempt_v2(fences.expected_session_revision,
                    &fences.expected_attempt_id, input.blocker_id.as_ref(), input.all, transaction.now())?;
                let mut body = serde_json::json!({"transition":"unblock", "session_state":"running"});
                if input.all { body["all"] = serde_json::json!(true); }
                else { body["blocker_id"] = serde_json::json!(input.blocker_id); }
                (outcome.into_state(), body)
            }
            DirectCursorCommandV1::Retry(input) => {
                let reason = ReasonV2::new(input.reason.clone()).map_err(GraphMutationErrorV2::Domain)?;
                let attempt = ids.next_attempt_id();
                let outcome = prior.retry_active_attempt_v2(fences.expected_session_revision,
                    &fences.expected_attempt_id, attempt.clone(), reason, transaction.now())?;
                let body = serde_json::json!({"transition":"retry", "session_state":"running",
                    "to_attempt_id":attempt, "to_graph_node_id":active.graph_node_id(), "reason":input.reason});
                (outcome.into_state(), body)
            }
        };
        body["schema"] = serde_json::json!("podway.stage-transition-result/v3");
        body["from_graph_node_id"] = serde_json::json!(active.graph_node_id());
        body["from_attempt_id"] = serde_json::json!(active.attempt_id());
        body["revision"] = serde_json::json!(next.trace().revision());
        let response = OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
            request_id: request.request_id, command: CommandNameV1::new(command.command_name()).expect("static command"),
            generated_at: Rfc3339MillisV1::from_unix_millis(transaction.now().get())
                .map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "clock exceeds the public timestamp range" })?,
            workspace: Some(WorkspaceOutputV1::new(transaction.binding().identity().workspace_uuid().clone(),
                resolved.worktree().roots().worktree_root().display().as_str(), transaction.next_workspace_sequence()?)
                .map_err(|_| DirectStoreErrorV1::BindingMismatch)?),
            session: Some(DirectSessionOutputV1 { id: next.trace().session_id().clone(), title: next.task_title().into(),
                lifecycle: next.trace().lifecycle(), revision_before: prior.trace().revision(), revision_after: next.trace().revision() }),
            result: body.as_object().expect("cursor result is an object").clone(), warnings: Vec::new(),
        }).map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "cursor success violates the canonical envelope" })?;
        let response = CanonicalExecutionJsonV1::new(canonicalize_json_v1(response.as_value())
            .map_err(|_| DirectStoreErrorV1::Unreadable)?)
            .map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "cursor success exceeds the outcome bound" })?;
        transaction.replace_graph(prior.workspace_revision(), prior.trace().revision(), &next)?;
        Ok(response)
    });
    if let Some(error) = execution_error {
        return Err(error);
    }
    let result = result?;
    serde_json::from_str(result.outcome().response().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable.into())
}

fn action_successor(
    graph: &podway_store::GraphSessionStateV2,
    node: &GraphNodeId,
) -> Result<Option<GraphNodeId>, DirectStoreErrorV1> {
    let node = graph
        .snapshot()
        .graph_nodes()
        .iter()
        .find(|entry| entry.graph_node_id() == node)
        .ok_or(DirectStoreErrorV1::Unreadable)?;
    if node.node_kind() != podway_core::NodeKindV2::Action {
        return Err(GraphMutationErrorV2::GraphNodeTypeMismatch {
            graph_node_id: node.graph_node_id().clone(),
            actual: node.node_kind(),
        }
        .into());
    }
    node.next_target()
        .map(|target| target.cloned())
        .map_err(|_| DirectStoreErrorV1::Unreadable)
}

#[derive(Clone, Debug)]
pub struct DirectBeginRequestV1 {
    pub request_id: RequestIdV1,
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub key: IdempotencyKeyV1,
    pub input: SessionBeginV1,
    pub deadline: Instant,
}

pub fn begin_session_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectBeginRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let (resolved, store) = resolver.resolve_existing(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    let canonical = canonical_procedure_v2_command_identity_v1(
        &ProcedureV2MutationCommandV1::SessionBegin(request.input.clone()),
        resolved.store_identity().workspace_uuid(),
    )
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "invalid begin semantic identity",
    })?;
    let digest = Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
        .expect("SHA-256 produces a canonical digest");
    let mutation = DirectMutationRequestV1::new(
        request.key,
        digest,
        "session.begin",
        request.request_id.as_str(),
        DirectOutcomeScopeV1::Session(request.input.preconditions.expected_session_id.clone()),
    )?;
    let initial_goal = request
        .input
        .initial_goal
        .as_ref()
        .map(|goal| {
            Ok::<_, DomainError>(GraphInitialGoalV2::new(
                GoalStatementV2::new(goal.goal.clone())?,
                goal_definition_from_wire_v2(&goal.criteria)?,
                goal.actor
                    .clone()
                    .map(ActorAttributionV2::new)
                    .transpose()?,
            ))
        })
        .transpose()
        .map_err(DirectExecutionErrorV1::Domain)?;
    let prepared = store.prepare_graph(request.deadline)?;
    let mut execution_error = None;
    let result = store.mutate(&mutation, request.deadline, || clock.now(), |transaction| {
        let current = transaction.use_prepared_graph(prepared.as_ref())?;
        let current = current.as_ref().ok_or_else(|| DirectStoreErrorV1::Domain(
            GraphMutationErrorV2::Domain(DomainError::SessionIdentityMismatch {
                expected: request.input.preconditions.expected_session_id.clone(), actual: None,
            })))?;
        let prior = current.as_ref();
        let fences = &request.input.preconditions;
        if prior.trace().session_id() != &fences.expected_session_id {
            return Err(GraphMutationErrorV2::Domain(DomainError::SessionIdentityMismatch {
                expected: fences.expected_session_id.clone(), actual: Some(prior.trace().session_id().clone()),
            }).into());
        }
        if prior.trace().revision() != fences.expected_session_revision {
            return Err(GraphMutationErrorV2::SessionRevisionConflict {
                expected: fences.expected_session_revision, actual: prior.trace().revision(),
            }.into());
        }
        if prior.trace().lifecycle() != SessionLifecycle::Prepared {
            return Err(GraphMutationErrorV2::SessionNotPrepared.into());
        }
        if initial_goal.is_some() && !prior.snapshot().goal_tracking() {
            return Err(GraphMutationErrorV2::GoalTrackingNotEnabled.into());
        }
        resolver.revalidate(&resolved, transaction.binding()).map_err(|error| {
            execution_error = Some(DirectExecutionErrorV1::from(error));
            DirectStoreErrorV1::InvalidInput { reason: "workspace revalidation failed" }
        })?;
        let attempt_id = ids.next_attempt_id();
        let now = transaction.now();
        let outcome = prior.begin_v2(fences.expected_session_revision, attempt_id.clone(), initial_goal, now)?;
        let next = outcome.state();
        let body = serde_json::json!({
            "schema":"podway.session-begin-result/v2", "procedure_schema":"podway.procedure/v2",
            "procedure_digest":next.snapshot().digest(), "session_id":next.trace().session_id(),
            "revision":next.trace().revision(), "session_state":"running",
            "entry_graph_node_id":next.snapshot().entry_graph_node_id(),
            "attempt":{"attempt_id":attempt_id,"attempt_number":1},
            "goal_tracking":next.snapshot().goal_tracking(), "goal_defined":outcome.goal_revision().is_some(),
            "goal_revision":outcome.goal_revision().map(|revision| revision.get()),
        }).as_object().expect("begin result is an object").clone();
        let response = OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
            request_id: request.request_id, command: CommandNameV1::new("session.begin").expect("static command"),
            generated_at: Rfc3339MillisV1::from_unix_millis(now.get())
                .map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "clock exceeds the public timestamp range" })?,
            workspace: Some(WorkspaceOutputV1::new(transaction.binding().identity().workspace_uuid().clone(),
                resolved.worktree().roots().worktree_root().display().as_str(), transaction.next_workspace_sequence()?)
                .map_err(|_| DirectStoreErrorV1::BindingMismatch)?),
            session: Some(DirectSessionOutputV1 { id: next.trace().session_id().clone(),
                title: next.task_title().into(), lifecycle: next.trace().lifecycle(),
                revision_before: prior.trace().revision(), revision_after: next.trace().revision() }),
            result: body, warnings: Vec::new(),
        }).map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "begin success violates the canonical envelope" })?;
        let response = canonicalize_json_v1(response.as_value())
            .map_err(|_| DirectStoreErrorV1::Unreadable)?;
        let response = CanonicalExecutionJsonV1::new(response)
            .map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "begin success exceeds the outcome bound" })?;
        transaction.replace_graph(prior.workspace_revision(), prior.trace().revision(), next)?;
        Ok(response)
    });
    if let Some(error) = execution_error {
        return Err(error);
    }
    let result = result?;
    serde_json::from_str(result.outcome().response().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable.into())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeDirectIdSourceV1;

macro_rules! native_id {
    ($method:ident, $kind:ident) => {
        fn $method(&self) -> $kind {
            $kind::new(uuid::Uuid::new_v4().to_string()).expect("generated UUID is canonical")
        }
    };
}

impl DirectIdSourceV1 for NativeDirectIdSourceV1 {
    native_id!(next_workspace_id, WorkspaceId);
    native_id!(next_session_id, SessionId);
    native_id!(next_attempt_id, AttemptId);
    native_id!(next_blocker_id, BlockerId);
    native_id!(next_procedure_snapshot_id, ProcedureSnapshotId);
    native_id!(next_delegation_receipt_id, DelegationReceiptIdV1);
}

#[derive(Debug)]
pub enum DirectExecutionErrorV1 {
    InputTooLarge,
    SessionNotTerminal,
    Workspace(DirectWorkspaceResolutionErrorV1),
    Layout(podway_git::WorkspaceLayoutErrorV1),
    Maintenance(crate::direct_maintenance::DirectMaintenanceErrorV1),
    Store(DirectStoreErrorV1),
    Protocol(ProtocolError),
    View(GraphViewErrorV2),
    Evidence(EvidenceReadErrorV2),
    Domain(DomainError),
    Boundary(ExecutionBoundaryErrorV1),
    Preparation(ExecutionErrorV1),
    Delegation(DirectDelegationFailureV1),
    Start(crate::execution::ProcedureV2StartPreparationErrorV1),
    StartDecisionRequired {
        session_id: SessionId,
        revision: podway_core::Revision,
        lifecycle: SessionLifecycle,
        current_terminal_disposition: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectDelegationFailureV1 {
    ReceiptRequired,
    PackageChanged,
    ItemsChanged,
    NotApplicable,
    AlreadyAccepted,
    BindingMismatch(podway_store::v2_delegation::DelegationBindingMismatchReasonV1),
}

impl From<GraphViewErrorV2> for DirectExecutionErrorV1 {
    fn from(error: GraphViewErrorV2) -> Self {
        Self::View(error)
    }
}

impl From<EvidenceReadErrorV2> for DirectExecutionErrorV1 {
    fn from(error: EvidenceReadErrorV2) -> Self {
        Self::Evidence(error)
    }
}

/// Query inputs contain no queue barrier or removed transport flag.
#[derive(Clone, Debug)]
pub enum DirectReadCommandV1 {
    ArchiveList,
    ArchiveShow {
        session_id: SessionId,
        tier: GraphStatusTierV2,
        history_before: Option<TraceSequenceV2>,
    },
    DelegationReceipt {
        session_id: SessionId,
        attempt_id: AttemptId,
    },
    Status {
        tier: GraphStatusTierV2,
        history_before: Option<TraceSequenceV2>,
    },
    Next,
    Observe,
    Evidence {
        source: GraphNodeId,
        item_id: ItemId,
        token: Option<EvidencePageTokenV1>,
        expected_consumer_attempt_id: Option<AttemptId>,
    },
    DelegationPackage,
}

impl DirectReadCommandV1 {
    pub const fn command_name(&self) -> &'static str {
        match self {
            Self::ArchiveList => "session.archive_list",
            Self::ArchiveShow { .. } => "session.archive_show",
            Self::DelegationReceipt { .. } => "delegation.receipt",
            Self::Status { .. } => "session.status",
            Self::Next => "session.next",
            Self::Observe => "session.observe",
            Self::Evidence { .. } => "evidence.read",
            Self::DelegationPackage => "delegation.package",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirectReadRequestV1 {
    pub request_id: RequestIdV1,
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub expected_session_id: Option<SessionId>,
    pub command: DirectReadCommandV1,
    pub deadline: Instant,
}

pub fn execute_read_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectReadRequestV1,
    clock: &impl ExecutionClockV1,
) -> Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let token_read = matches!(
        &request.command,
        DirectReadCommandV1::Evidence { token: Some(_), .. }
    );
    let (resolved, store) = resolver.resolve_existing(
        request.selector,
        if token_read {
            None
        } else {
            request.expected_workspace_id.as_ref()
        },
        request.deadline,
    )?;
    store.read(request.deadline, |snapshot| {
        Ok((|| {
            resolver
                .revalidate(&resolved, snapshot.binding())
                .map_err(DirectWorkspaceResolutionErrorV1::from)?;
            let now = clock.now();
            let current = snapshot.view(now)?;
            let view = match &request.command {
                DirectReadCommandV1::ArchiveShow { session_id, .. } => snapshot.archived_view(session_id, now)?
                    .ok_or_else(|| DirectStoreErrorV1::State(podway_store::StoreErrorV1::SessionArchiveNotFoundV1 { session_id: session_id.clone() }))?,
                DirectReadCommandV1::DelegationReceipt { session_id, .. }
                    if !current.graph().is_some_and(|graph| graph.trace().session_id() == session_id) =>
                    snapshot.archived_view(session_id, now)?.ok_or(GraphViewErrorV2::MissingGraphSession)?,
                _ => current,
            };
            if let DirectReadCommandV1::Evidence { item_id, token, .. } = &request.command {
                crate::v2_read_service::check_evidence_token_binding_v1(
                    view.graph().map(|graph| graph.trace().session_id()), item_id, token.as_ref(),
                )?;
            }
            if token_read && let Some(expected) = &request.expected_workspace_id
                && expected != snapshot.binding().identity().workspace_uuid()
            {
                return Err(crate::workspace::WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch {
                    expected: expected.clone(),
                    actual: snapshot.binding().identity().workspace_uuid().clone(),
                }.into());
            }
            if let Some(expected) = request.expected_session_id
                && !view.graph().is_some_and(|graph| graph.trace().session_id() == &expected)
            {
                return Err(DirectExecutionErrorV1::Domain(
                    DomainError::SessionIdentityMismatch {
                        expected,
                        actual: view.graph().map(|graph| graph.trace().session_id().clone()),
                    },
                ));
            }
            let result = match &request.command {
                DirectReadCommandV1::ArchiveList => {
                    use crate::direct_read_service::archive_summary;
                    let page = snapshot.archives(podway_protocol::SESSION_ARCHIVE_LIST_WINDOW_V2)?;
                    serde_json::json!({"schema":"podway.session-archive-list-result/v2", "count":page.count(),
                        "truncated":page.count() > podway_protocol::SESSION_ARCHIVE_LIST_WINDOW_V2 as u64,
                        "oldest":page.oldest().map(archive_summary), "sessions":page.sessions().iter().map(archive_summary).collect::<Vec<_>>()})
                        .as_object().expect("archive list is an object").clone()
                }
                DirectReadCommandV1::ArchiveShow { session_id, tier, history_before } => {
                    let summary = snapshot.archive_summary(session_id)?.ok_or(DirectStoreErrorV1::Unreadable)?;
                    crate::direct_read_service::project_direct_archive_show_v1(&view, &summary, *tier, *history_before)?
                }
                DirectReadCommandV1::DelegationReceipt { session_id, attempt_id } => {
                    let receipt = snapshot.delegation_receipt(session_id, attempt_id)?;
                    crate::direct_read_service::project_direct_receipt_v1(&view, attempt_id, receipt.as_ref())?
                }
                DirectReadCommandV1::Status {
                    tier,
                    history_before,
                } => project_direct_status_v1(&view, *tier, *history_before)?,
                DirectReadCommandV1::Next => project_direct_next_v1(&view)?,
                DirectReadCommandV1::Observe => project_direct_observation_v1(&view)?,
                DirectReadCommandV1::Evidence {
                    source,
                    item_id,
                    token,
                    expected_consumer_attempt_id,
                } => {
                    if let Some(expected) = expected_consumer_attempt_id {
                        let actual = view.graph().and_then(|graph| graph.trace().active_attempt())
                            .map(|attempt| attempt.attempt_id().clone());
                        if actual.as_ref() != Some(expected) {
                            return Err(DirectExecutionErrorV1::Domain(DomainError::AttemptNotCurrent {
                                expected: expected.clone(), actual,
                            }));
                        }
                    }
                    project_direct_evidence_v1(&view, source, item_id, token.as_ref())?
                },
                DirectReadCommandV1::DelegationPackage => {
                    project_direct_delegation_package_v1(&view)?
                }
            };
            Ok(OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
                request_id: request.request_id,
                command: CommandNameV1::new(request.command.command_name())?,
                generated_at: Rfc3339MillisV1::from_unix_millis(now.get())?,
                workspace: Some(WorkspaceOutputV1::new(
                    snapshot.binding().identity().workspace_uuid().clone(),
                    resolved
                        .worktree()
                        .roots()
                        .worktree_root()
                        .display()
                        .as_str(),
                    view.sequence(),
                )?),
                session: None,
                result,
                warnings: Vec::new(),
            })?)
        })())
    })?
}

impl From<DirectWorkspaceResolutionErrorV1> for DirectExecutionErrorV1 {
    fn from(error: DirectWorkspaceResolutionErrorV1) -> Self {
        Self::Workspace(error)
    }
}

impl From<crate::workspace::WorkspaceResolutionErrorV1> for DirectExecutionErrorV1 {
    fn from(error: crate::workspace::WorkspaceResolutionErrorV1) -> Self {
        Self::Workspace(error.into())
    }
}

impl From<DirectStoreErrorV1> for DirectExecutionErrorV1 {
    fn from(error: DirectStoreErrorV1) -> Self {
        Self::Store(error)
    }
}

impl From<ProtocolError> for DirectExecutionErrorV1 {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

#[derive(Clone, Debug)]
pub struct OutcomeLookupRequestV1 {
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub request_id: RequestIdV1,
    pub key: IdempotencyKeyV1,
    pub deadline: Instant,
}

/// Read-only outcome lookup observes binding, sequence and response in one snapshot.
pub fn lookup_outcome_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: OutcomeLookupRequestV1,
    clock: &impl ExecutionClockV1,
) -> Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let (resolved, store) = resolver.resolve_existing(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    store.read(request.deadline, |snapshot| {
        Ok((|| {
            resolver.revalidate(&resolved, snapshot.binding())
                .map_err(DirectWorkspaceResolutionErrorV1::from)?;
            let outcome = snapshot.outcome(&request.key)?;
            let outcome = outcome.map(|outcome| {
                let response = serde_json::from_str(outcome.response().as_str())
                    .map_err(|_| DirectStoreErrorV1::Unreadable)?;
                let response = OutputEnvelopeV4::from_value(response)
                    .map_err(|_| DirectStoreErrorV1::Unreadable)?;
                let recorded_at = Rfc3339MillisV1::from_unix_millis(outcome.recorded_at().get())?;
                Ok::<_, DirectExecutionErrorV1>(serde_json::json!({
                    "request_digest": outcome.digest(), "recorded_at": recorded_at,
                    "scope": match outcome.scope() {
                        DirectOutcomeScopeV1::Workspace => "workspace",
                        DirectOutcomeScopeV1::Session(_) => "session",
                    }, "response": response,
                }))
            }).transpose()?;
            let result = serde_json::json!({
                "schema": "podway.outcome-lookup-result/v1", "idempotency_key": request.key.as_str(),
                "found": outcome.is_some(), "outcome": outcome,
            }).as_object().expect("result is an object").clone();
            Ok(OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
                request_id: request.request_id, command: CommandNameV1::new("outcome.lookup")?,
                generated_at: Rfc3339MillisV1::from_unix_millis(clock.now().get())?,
                workspace: Some(WorkspaceOutputV1::new(
                    snapshot.binding().identity().workspace_uuid().clone(),
                    resolved.worktree().roots().worktree_root().display().as_str(),
                    snapshot.committed_sequence()?,
                )?), session: None, result, warnings: Vec::new(),
            })?)
        })())
    })?
}

impl From<crate::direct_workspace::DirectWorkspaceErrorV1> for DirectExecutionErrorV1 {
    fn from(error: crate::direct_workspace::DirectWorkspaceErrorV1) -> Self {
        use crate::direct_workspace::DirectWorkspaceErrorV1 as E;
        match error {
            E::Resolution(error) => Self::Workspace(error),
            E::Store(error) => Self::Store(error),
            E::Protocol(error) => Self::Protocol(error),
            E::Layout(error) => Self::Layout(error),
            E::Boundary(error) => Self::Boundary(error),
        }
    }
}
