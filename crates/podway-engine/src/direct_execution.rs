//! In-process command boundaries. No job, IPC, registry or runtime-mode authority.

use sha2::{Digest as _, Sha256};
use std::time::Instant;

use podway_core::{
    ActorAttributionV2, AttemptId, BlockerId, DelegationReceiptIdV1, DomainError,
    GoalRevisionNumberV2, GoalStatementV2, GraphNodeId, ItemId, ProcedureSnapshotId, ReasonV2,
    Revision, SessionId, SessionLifecycle, Sha256Digest, TraceSequenceV2, WorkspaceId,
};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{
    CommandNameV1, ErrorEnvelopeV2, EvidencePageTokenV1, MAX_DIRECT_OUTPUT_BYTES_V1,
    MAX_JSON_DEPTH_V1, OutputEnvelopeInputV4, OutputEnvelopeV4, ProcedureV2MutationCommandV1,
    ProtocolError, RequestIdV1, Rfc3339MillisV1, SessionBeginV1, SessionBlockV1, SessionCancelV1,
    SessionCompleteV1, SessionMutationPreconditionsWireV1, SessionRetryV1, SessionSkipV1,
    SessionUnblockV1, SliceCommandV1, WorkspaceOutputV1,
    canonical_direct_slice_mutation_identity_v1, canonical_procedure_v2_command_identity_v1,
};
use podway_store::{
    GraphInitialGoalV2, GraphMutationErrorV2, IdempotencyKeyV1,
    direct_store::{DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1},
};

use crate::{
    direct_mutation::{check_attempt_fence, check_session_fences, graph_success},
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
    let result = store.mutate_typed(&mutation, request.deadline, || clock.now(), |transaction| -> Result<_, DirectExecutionErrorV1> {
        let current = transaction.use_prepared_graph(prepared.as_ref())?;
        let current = current.as_ref().ok_or_else(|| DirectStoreErrorV1::Domain(
            GraphMutationErrorV2::Domain(DomainError::SessionIdentityMismatch {
                expected: fences.expected_session_id.clone(), actual: None,
            })))?;
        let prior = current.as_ref();
        check_session_fences(prior, &fences.expected_session_id, fences.expected_session_revision)?;
        if prior.trace().lifecycle() != SessionLifecycle::Running {
            return Err(GraphMutationErrorV2::SessionNotRunning.into());
        }
        let active = prior.trace().active_attempt().ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        check_attempt_fence(prior, &fences.expected_attempt_id)?;
        resolver.revalidate(&resolved, transaction.binding()).map_err(DirectExecutionErrorV1::from)?;
        if matches!(request.command, DirectCursorCommandV1::Complete(_)) {
            use podway_store::v2_delegation::DelegationReadinessV1;
            let failure = match transaction.delegation_receipts()?.readiness() {
                DelegationReadinessV1::Direct | DelegationReadinessV1::ReceiptCurrent => None,
                DelegationReadinessV1::ReceiptMissing => Some(DirectDelegationFailureV1::ReceiptRequired),
                DelegationReadinessV1::PackageChanged => Some(DirectDelegationFailureV1::PackageChanged),
                DelegationReadinessV1::ItemsChanged => Some(DirectDelegationFailureV1::ItemsChanged),
            };
            if let Some(failure) = failure {
                return Err(DirectExecutionErrorV1::Delegation(failure));
            }
            let artifacts = artifacts?;
            native.verify_required_artifacts_v2(transaction.binding(), prior, &artifacts)?;
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
                    return Err(DirectStoreErrorV1::InvalidInput { reason: "invalid block reason" }.into());
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
                    return Err(DirectStoreErrorV1::InvalidInput { reason: "unblock requires a blocker or all" }.into());
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
        let response = graph_success(transaction, &resolved, request.request_id, command.command_name(), prior, &next,
            body.as_object().expect("cursor result is an object").clone())?;
        transaction.replace_graph(prior.workspace_revision(), prior.trace().revision(), &next)?;
        Ok(response)
    });
    let result = result.map_err(podway_store::direct_store::DirectMutationFailureV1::into_error)?;
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
    let result = store.mutate_typed(&mutation, request.deadline, || clock.now(), |transaction| -> Result<_, DirectExecutionErrorV1> {
        let current = transaction.use_prepared_graph(prepared.as_ref())?;
        let current = current.as_ref().ok_or_else(|| DirectStoreErrorV1::Domain(
            GraphMutationErrorV2::Domain(DomainError::SessionIdentityMismatch {
                expected: request.input.preconditions.expected_session_id.clone(), actual: None,
            })))?;
        let prior = current.as_ref();
        let fences = &request.input.preconditions;
        check_session_fences(prior, &fences.expected_session_id, fences.expected_session_revision)?;
        if prior.trace().lifecycle() != SessionLifecycle::Prepared {
            return Err(GraphMutationErrorV2::SessionNotPrepared.into());
        }
        if initial_goal.is_some() && !prior.snapshot().goal_tracking() {
            return Err(GraphMutationErrorV2::GoalTrackingNotEnabled.into());
        }
        resolver.revalidate(&resolved, transaction.binding()).map_err(DirectExecutionErrorV1::from)?;
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
        let response = graph_success(transaction, &resolved, request.request_id, "session.begin", prior, next, body)?;
        transaction.replace_graph(prior.workspace_revision(), prior.trace().revision(), next)?;
        Ok(response)
    });
    let result = result.map_err(podway_store::direct_store::DirectMutationFailureV1::into_error)?;
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
                DirectReadCommandV1::ArchiveShow { session_id, .. } => {
                    snapshot.archived_view(session_id, now)?.ok_or_else(|| {
                        DirectStoreErrorV1::State(
                            podway_store::StoreErrorV1::SessionArchiveNotFoundV1 {
                                session_id: session_id.clone(),
                            },
                        )
                    })?
                }
                DirectReadCommandV1::DelegationReceipt { session_id, .. }
                    if !current
                        .graph()
                        .is_some_and(|graph| graph.trace().session_id() == session_id) =>
                {
                    snapshot
                        .archived_view(session_id, now)?
                        .ok_or(GraphViewErrorV2::MissingGraphSession)?
                }
                _ => current,
            };
            if let DirectReadCommandV1::Evidence { item_id, token, .. } = &request.command {
                crate::v2_read_service::check_evidence_token_binding_v1(
                    view.graph().map(|graph| graph.trace().session_id()),
                    item_id,
                    token.as_ref(),
                )?;
            }
            if token_read
                && let Some(expected) = &request.expected_workspace_id
                && expected != snapshot.binding().identity().workspace_uuid()
            {
                return Err(
                    crate::workspace::WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch {
                        expected: expected.clone(),
                        actual: snapshot.binding().identity().workspace_uuid().clone(),
                    }
                    .into(),
                );
            }
            if let Some(expected) = request.expected_session_id
                && !view
                    .graph()
                    .is_some_and(|graph| graph.trace().session_id() == &expected)
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
                    let page =
                        snapshot.archives(podway_protocol::SESSION_ARCHIVE_LIST_WINDOW_V2)?;
                    // The summaries are built once and move into the result: the `json!` macro
                    // finalizes owned expressions through `to_value`, which would build a second
                    // copy of every summary before the extraction cloned the finished map again.
                    let mut result = serde_json::Map::with_capacity(5);
                    result.insert(
                        "schema".to_owned(),
                        serde_json::json!("podway.session-archive-list-result/v2"),
                    );
                    result.insert("count".to_owned(), serde_json::json!(page.count()));
                    result.insert(
                        "truncated".to_owned(),
                        serde_json::json!(
                            page.count() > podway_protocol::SESSION_ARCHIVE_LIST_WINDOW_V2 as u64
                        ),
                    );
                    result.insert(
                        "oldest".to_owned(),
                        page.oldest()
                            .map(archive_summary)
                            .unwrap_or(serde_json::Value::Null),
                    );
                    result.insert(
                        "sessions".to_owned(),
                        serde_json::Value::Array(
                            page.sessions().iter().map(archive_summary).collect(),
                        ),
                    );
                    result
                }
                DirectReadCommandV1::ArchiveShow {
                    session_id,
                    tier,
                    history_before,
                } => {
                    let summary = snapshot
                        .archive_summary(session_id)?
                        .ok_or(DirectStoreErrorV1::Unreadable)?;
                    crate::direct_read_service::project_direct_archive_show_v1(
                        &view,
                        &summary,
                        *tier,
                        *history_before,
                    )?
                }
                DirectReadCommandV1::DelegationReceipt {
                    session_id,
                    attempt_id,
                } => {
                    let receipt = snapshot.delegation_receipt(session_id, attempt_id)?;
                    crate::direct_read_service::project_direct_receipt_v1(
                        &view,
                        attempt_id,
                        receipt.as_ref(),
                    )?
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
                        let actual = view
                            .graph()
                            .and_then(|graph| graph.trace().active_attempt())
                            .map(|attempt| attempt.attempt_id().clone());
                        if actual.as_ref() != Some(expected) {
                            return Err(DirectExecutionErrorV1::Domain(
                                DomainError::AttemptNotCurrent {
                                    expected: expected.clone(),
                                    actual,
                                },
                            ));
                        }
                    }
                    project_direct_evidence_v1(&view, source, item_id, token.as_ref())?
                }
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

/// Request context for the READS-004 typed fence-preflight facts operation.
///
/// The operation is fixed to the Standard `session.status` projection the CLI
/// fence preflight consumed: it accepts no command or idempotency-key surface
/// and carries exactly the rendering context the shared error finisher needs,
/// preserving the original single request identity, observation and deadline.
#[derive(Clone, Debug)]
pub struct DirectPreflightFactsRequestV1 {
    pub request_id: RequestIdV1,
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub expected_session_id: Option<SessionId>,
    pub deadline: Instant,
    pub observed_at: Rfc3339MillisV1,
    pub timeout_ms: u64,
}

/// Owned fence-preflight facts assembled from validated typed state.
#[derive(Clone, Debug)]
pub struct DirectPreflightFactsV1 {
    pub workspace_id: WorkspaceId,
    pub session_id: SessionId,
    pub session_revision: Revision,
    pub attempt_id: Option<AttemptId>,
    pub item_revisions: Vec<(String, Revision)>,
    pub goal_revision: Option<GoalRevisionNumberV2>,
    pub lifecycle: SessionLifecycle,
}

/// The facts-operation response, mirroring [`DirectResponseV1`](crate::direct_api::DirectResponseV1):
/// owned typed facts on success, or the same rendered failure envelope the
/// original status read produced.
#[derive(Clone, Debug)]
pub enum DirectPreflightFactsResponseV1 {
    Facts(DirectPreflightFactsV1),
    Failure(ErrorEnvelopeV2),
}

/// The exact serialized framing of a Standard status output envelope beyond its
/// variable members: envelope and workspace braces, member separators, and the
/// fixed schema, command and warnings values. `session` is skipped (always
/// `None` for reads) and every variable member is counted exactly once by its
/// own serialization in [`envelope_equivalent_status_bytes_v1`].
const STATUS_ENVELOPE_FRAMING_BYTES_V1: usize = 2 // envelope braces
    + 6 // separators between the seven envelope members
    + "\"schema\":\"podway.output/v4\"".len()
    + "\"request_id\":".len()
    + "\"command\":\"session.status\"".len()
    + "\"generated_at\":".len()
    + "\"workspace\":".len()
    + 2 // workspace braces
    + 2 // separators between the three workspace members
    + "\"uuid\":".len()
    + "\"root\":".len()
    + "\"latest_workspace_sequence\":".len()
    + "\"result\":".len()
    + "\"warnings\":[]".len();

/// Exact envelope-equivalent byte accounting without constructing an envelope:
/// each variable member is serialized individually (the same compact JSON the
/// member would occupy inside the document) and the fixed framing constant
/// supplies every remaining byte. The sum equals the serialized length of the
/// equivalent `podway.output/v4` status document, so comparing it against
/// [`MAX_DIRECT_OUTPUT_BYTES_V1`] preserves the original envelope byte bound.
#[doc(hidden)]
pub fn envelope_equivalent_status_bytes_v1(
    result: &serde_json::Value,
    root: &str,
    request_id: &str,
    workspace_uuid: &str,
    generated_at: &str,
    workspace_sequence: u64,
) -> usize {
    let string_bytes =
        |value: &str| serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len());
    // Serializing a string or a `Map<String, Value>` cannot fail; the fallback
    // only keeps this total function fail-closed.
    serde_json::to_vec(result).map_or(usize::MAX, |bytes| {
        bytes.len()
            + string_bytes(root)
            + string_bytes(request_id)
            + string_bytes(workspace_uuid)
            + string_bytes(generated_at)
            + workspace_sequence.to_string().len()
            + STATUS_ENVELOPE_FRAMING_BYTES_V1
    })
}

/// The deepest nesting level below `value`, counting `value` itself as level
/// zero. This is the accounting rule `podway_protocol` enforces with
/// [`MAX_JSON_DEPTH_V1`]: a document is refused when a level exceeds the
/// maximum, and a result nested inside an envelope sits exactly one level
/// deeper than it did as a standalone document.
#[doc(hidden)]
pub fn json_nesting_levels_v1(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(map) => map
            .values()
            .map(json_nesting_levels_v1)
            .max()
            .map_or(0, |deepest| deepest + 1),
        serde_json::Value::Array(values) => values
            .iter()
            .map(json_nesting_levels_v1)
            .max()
            .map_or(0, |deepest| deepest + 1),
        _ => 0,
    }
}

/// The exact Stage 5 envelope-equivalent checks the Standard facts operation
/// runs after Stage 4, extracted as the one C1 test subject: the generated-at
/// clock formatting and the workspace construction the original envelope input
/// evaluated first, then the depth, byte and root-pattern conditions its
/// document validation enforced — the same fallible operations in the original
/// order, without constructing or parsing a public output envelope.
#[doc(hidden)]
pub fn stage5_status_equivalent_checks_v1(
    now_ms: u64,
    request_id: &RequestIdV1,
    workspace_uuid: &WorkspaceId,
    root: &str,
    workspace_sequence: u64,
    result: &serde_json::Value,
) -> Result<(), ProtocolError> {
    // The envelope input's field order: generated-at clock, then workspace.
    let generated_at = Rfc3339MillisV1::from_unix_millis(now_ms)?;
    drop(WorkspaceOutputV1::new(
        workspace_uuid.clone(),
        root.to_owned(),
        workspace_sequence,
    )?);
    // The document validation trio order: depth, then bytes, then schema —
    // the root-pattern condition below is the schema-stage slice.
    if json_nesting_levels_v1(result) + 1 > MAX_JSON_DEPTH_V1 {
        return Err(ProtocolError::InvalidCommandResult {
            command: "session.status".to_owned(),
        });
    }
    if envelope_equivalent_status_bytes_v1(
        result,
        root,
        request_id.as_str(),
        workspace_uuid.as_str(),
        generated_at.as_str(),
        workspace_sequence,
    ) > MAX_DIRECT_OUTPUT_BYTES_V1
    {
        return Err(ProtocolError::InvalidCommandResult {
            command: "session.status".to_owned(),
        });
    }
    // The canonical workspace schema requires an absolute root beyond the
    // constructor's scalar bound; the generic resolver contract admits
    // relative diagnostic displays.
    if !root.starts_with('/') {
        return Err(ProtocolError::InvalidCommandResult {
            command: "session.status".to_owned(),
        });
    }
    Ok(())
}

/// READS-004 typed fence-preflight facts over the shared Standard status
/// validation core.
///
/// Ordering mirrors [`execute_read_v1`]: workspace resolution, Store read
/// admission (lease, process gate, transaction), revalidation, one clock read,
/// the eager current view and the expected-session fence all precede the Stage
/// 4 Standard projection and its schema validation. The retained result then
/// feeds the exact envelope-equivalent Stage 5 checks — the same fallible
/// operations, in the original order, without constructing or parsing a public
/// output envelope — and owned facts are assembled from typed state. The
/// validation map is dropped before the facts are returned.
pub fn preflight_facts_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectPreflightFactsRequestV1,
    clock: &impl ExecutionClockV1,
) -> Result<DirectPreflightFactsV1, DirectExecutionErrorV1> {
    let (resolved, store) = resolver.resolve_existing(
        request.selector.clone(),
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    store.read(request.deadline, |snapshot| {
        Ok((|| {
            resolver
                .revalidate(&resolved, snapshot.binding())
                .map_err(DirectWorkspaceResolutionErrorV1::from)?;
            let now = clock.now();
            let view = snapshot.view(now)?;
            if let Some(expected) = &request.expected_session_id
                && !view
                    .graph()
                    .is_some_and(|graph| graph.trace().session_id() == expected)
            {
                return Err(DirectExecutionErrorV1::Domain(
                    DomainError::SessionIdentityMismatch {
                        expected: expected.clone(),
                        actual: view.graph().map(|graph| graph.trace().session_id().clone()),
                    },
                ));
            }
            // Stage 4: the shared Standard projection and its schema validation.
            let result = crate::direct_read_service::project_direct_status_v1(
                &view,
                GraphStatusTierV2::Standard,
                None,
            )?;
            let workspace_uuid = snapshot.binding().identity().workspace_uuid().clone();
            let root = resolved
                .worktree()
                .roots()
                .worktree_root()
                .display()
                .as_str()
                .to_owned();
            let workspace_sequence = view.sequence();
            let result = serde_json::Value::Object(result);
            stage5_status_equivalent_checks_v1(
                now.get(),
                &request.request_id,
                &workspace_uuid,
                &root,
                workspace_sequence,
                &result,
            )?;
            drop(result);
            // Facts derive from typed state, never from the validation map.
            let graph = view.graph().ok_or(GraphViewErrorV2::MissingGraphSession)?;
            let trace = graph.trace();
            let lifecycle = trace.lifecycle();
            let attempt = trace.active_attempt();
            let item_revisions = attempt
                .and_then(|attempt| {
                    graph
                        .workflow_memory()
                        .attempts()
                        .iter()
                        .find(|memory| memory.attempt_id() == attempt.attempt_id())
                })
                .map(|memory| {
                    memory
                        .item_slots()
                        .iter()
                        .map(|slot| (slot.item_id().as_str().to_owned(), slot.revision()))
                        .collect()
                })
                .unwrap_or_default();
            Ok(DirectPreflightFactsV1 {
                workspace_id: workspace_uuid,
                session_id: trace.session_id().clone(),
                session_revision: trace.revision(),
                attempt_id: attempt.map(|attempt| attempt.attempt_id().clone()),
                item_revisions,
                goal_revision: graph.goal_state().current_revision(),
                lifecycle,
            })
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

// Callback transport retains the prior Store classification of pure graph
// failures and persistence failures; it does not reclassify them as input errors.
impl From<GraphMutationErrorV2> for DirectExecutionErrorV1 {
    fn from(error: GraphMutationErrorV2) -> Self {
        Self::Store(error.into())
    }
}

impl From<podway_store::StoreErrorV1> for DirectExecutionErrorV1 {
    fn from(error: podway_store::StoreErrorV1) -> Self {
        Self::Store(error.into())
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

#[cfg(test)]
mod reads004_stage5_tests {
    //! READS-004 C1 component evidence. The framing constant and depth rule
    //! are pinned against real `serde_json` serialization and the real
    //! protocol document validator, and the extracted production Stage 5
    //! sequence is compared against the original envelope sequence it
    //! replaces. Synthetic oracle inputs are labeled as such: schema-valid
    //! Standard results inflated beyond what the Core projection window can
    //! emit are Stage 5 oracle inputs only, not Core-admitted or
    //! production-window-reachable status shapes.
    use super::*;
    use podway_protocol::validate_daemon_free_document_v1;

    const UUID: &str = "00000000-0000-4000-8000-000000000001";
    const GENERATED_AT: &str = "2023-11-14T22:13:20.000Z";
    const NOW_MS: u64 = 1_700_000_000_000;
    const OUT_OF_RANGE_MS: u64 = 253_402_300_800_000;

    fn envelope_value(
        result: serde_json::Value,
        root: &str,
        workspace_sequence: u64,
    ) -> serde_json::Value {
        serde_json::json!({
            "schema": "podway.output/v4",
            "request_id": UUID,
            "command": "session.status",
            "generated_at": GENERATED_AT,
            "workspace": {
                "uuid": UUID,
                "root": root,
                "latest_workspace_sequence": workspace_sequence,
            },
            "result": result,
            "warnings": [],
        })
    }

    #[test]
    fn framing_constant_matches_real_serialization_exactly() {
        let results = [
            serde_json::json!({}),
            serde_json::json!({"schema":"podway.status-result/v5","tier":"standard"}),
            serde_json::json!({"nested":{"deep":[{"id":"x","value":"雪"}],"flag":true}}),
            serde_json::json!({
                "text": "quote\" backslash\\ tab\t control\u{1} unicode-ü-雪",
                "array": ["a", 1, -2, true, null],
            }),
        ];
        let roots = [
            "/",
            "/plain-root",
            "/with\"quote",
            "/back\\slash",
            "/tab\tand\u{7}control",
            "/unicode-ü-雪",
            &format!("/{}", "a".repeat(4_095)),
        ];
        let sequences = [0u64, 1, 42, 9_999_999_999_999_999_999, u64::MAX];
        for result in results {
            for root in roots {
                for sequence in sequences {
                    let value = envelope_value(result.clone(), root, sequence);
                    let actual = serde_json::to_vec(&value).unwrap().len();
                    let equivalent = envelope_equivalent_status_bytes_v1(
                        &result,
                        root,
                        UUID,
                        UUID,
                        GENERATED_AT,
                        sequence,
                    );
                    assert_eq!(
                        actual, equivalent,
                        "framing drifted for root {root:?} sequence {sequence}"
                    );
                }
            }
        }
    }

    /// Builds the oracle map shape without any validity claim. Pure
    /// construction/length probes for the size search are not equivalence
    /// evidence and may transiently exceed the standalone bound; only the
    /// final tuned map is validated as an oracle input below.
    fn raw_oracle_result(item_values: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({
            "schema": "podway.status-result/v5",
            "tier": "standard",
            "procedure": {
                "schema": "podway.procedure/v2",
                "id": "oracle",
                "version": "1",
                "digest": format!("sha256:{}", "0".repeat(64)),
            },
            "session": {"id": UUID, "lifecycle": "running", "revision": 1},
            "current": {
                "node": {
                    "node_definition_id": "decide",
                    "graph_node_id": "work",
                    "node_type": "decision",
                },
                "attempt": {"attempt_id": UUID, "attempt_number": 1},
                "readiness": {
                    "items_satisfied": false,
                    "unblocked": true,
                    "goal_ready": true,
                    "can_advance": false,
                },
                "missing_required_item_count": 0,
                "blockers_total": 0,
            },
            "purpose": "Stage 5 oracle input.",
            "goal_tracking": false,
            "goal_defined": false,
            "trace_length": 1,
            "counters": [],
            "items": [],
            "missing_required_item_ids": [],
            "blocker_window": [],
            "blockers_truncated": false,
            "item_values": item_values,
            "items_total": item_values.len(),
            "items_truncated": false,
            "references": [],
            "allowed_option_ids": [],
            "allowed_manual_rework_targets": [],
        })
    }

    /// The validated final oracle input: a schema-valid Standard status
    /// document within Stage 4's standalone byte and depth limits. Validity
    /// is asserted, never assumed; `item_values` is caller-supplied because
    /// the boundary tests deliberately inflate it beyond what the Core
    /// projection window can emit. The schema ties non-empty item windows to
    /// a running session with a full current projection, so the shape
    /// mirrors that.
    fn status_oracle_result(item_values: Vec<serde_json::Value>) -> serde_json::Value {
        assert!(
            item_values.len() <= 128,
            "oracle item_values exceed the schema bound"
        );
        let result = raw_oracle_result(item_values);
        assert!(
            serde_json::to_vec(&result).unwrap().len() <= MAX_DIRECT_OUTPUT_BYTES_V1,
            "oracle result exceeds Stage 4's standalone byte limit"
        );
        assert!(json_nesting_levels_v1(&result) < MAX_JSON_DEPTH_V1);
        assert!(
            validate_daemon_free_document_v1("podway.status-result/v5", &result),
            "oracle result is not a schema-valid Standard status document"
        );
        result
    }

    /// The original Stage 5 sequence the facts operation replaced: the
    /// envelope input's clock and workspace construction, then
    /// `OutputEnvelopeV4::new` with its depth/byte/schema document trio.
    /// Test-side only; production never builds this envelope.
    fn original_stage5_sequence(
        now_ms: u64,
        root: &str,
        workspace_sequence: u64,
        result: &serde_json::Value,
    ) -> Result<(), ProtocolError> {
        let generated_at = Rfc3339MillisV1::from_unix_millis(now_ms)?;
        let workspace = WorkspaceOutputV1::new(
            WorkspaceId::new(UUID).unwrap(),
            root.to_owned(),
            workspace_sequence,
        )?;
        OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
            request_id: RequestIdV1::new(UUID).unwrap(),
            command: CommandNameV1::new("session.status")?,
            generated_at,
            workspace: Some(workspace),
            session: None,
            result: result.as_object().unwrap().clone(),
            warnings: Vec::new(),
        })?;
        Ok(())
    }

    fn stage5(now_ms: u64, root: &str, result: &serde_json::Value) -> Result<(), ProtocolError> {
        stage5_status_equivalent_checks_v1(
            now_ms,
            &RequestIdV1::new(UUID).unwrap(),
            &WorkspaceId::new(UUID).unwrap(),
            root,
            42,
            result,
        )
    }

    fn variant(error: &ProtocolError) -> &'static str {
        match error {
            ProtocolError::InvalidTimestamp => "InvalidTimestamp",
            ProtocolError::EmptyValue { .. } => "EmptyValue",
            ProtocolError::ValueTooLong { .. } => "ValueTooLong",
            ProtocolError::InvalidCommandResult { .. } => "InvalidCommandResult",
            _ => "other",
        }
    }

    fn assert_same_failure(
        root: &str,
        now_ms: u64,
        result: &serde_json::Value,
        expected_variant: &'static str,
    ) {
        let new = stage5(now_ms, root, result).expect_err("Stage 5 check unexpectedly accepted");
        let old = original_stage5_sequence(now_ms, root, 42, result)
            .expect_err("original envelope sequence unexpectedly accepted");
        assert_eq!(variant(&new), expected_variant);
        assert_eq!(
            variant(&old),
            expected_variant,
            "Stage 5 diverged from the original sequence on root {root:?}"
        );
        match (&old, &new) {
            (ProtocolError::InvalidTimestamp, ProtocolError::InvalidTimestamp) => {}
            (
                ProtocolError::EmptyValue { field: old_field },
                ProtocolError::EmptyValue { field },
            ) => {
                assert_eq!((old_field, field), (&"workspace.root", &"workspace.root"));
            }
            (
                ProtocolError::ValueTooLong {
                    field: old_field,
                    maximum: old_maximum,
                    actual: old_actual,
                },
                ProtocolError::ValueTooLong {
                    field,
                    maximum,
                    actual,
                },
            ) => {
                assert_eq!(old_field, field);
                assert_eq!((old_maximum, maximum), (&4_096, &4_096));
                assert_eq!(old_actual, actual);
            }
            (
                ProtocolError::InvalidCommandResult {
                    command: old_command,
                },
                ProtocolError::InvalidCommandResult { command },
            ) => {
                assert_eq!(old_command, command);
                assert_eq!(command, "session.status");
            }
            _ => panic!("unmatched variant pair for root {root:?}"),
        }
    }

    #[test]
    fn stage5_sequence_matches_the_original_envelope_sequence() {
        let result = status_oracle_result(vec![serde_json::json!({
            "item_id": "i00",
            "value": "oracle value",
            "value_truncated": false,
        })]);
        // Both sequences accept a bounded absolute root.
        assert!(stage5(NOW_MS, "/plain", &result).is_ok());
        assert!(original_stage5_sequence(NOW_MS, "/plain", 42, &result).is_ok());
        assert!(stage5(NOW_MS, &format!("/{}", "a".repeat(4_095)), &result).is_ok());
        // A 4,096-scalar multibyte root stays within the scalar bound even
        // though its UTF-8 encoding exceeds 4,096 bytes.
        let multibyte = format!("/{}", "雪".repeat(4_095));
        assert_eq!(multibyte.chars().count(), 4_096);
        assert!(stage5(NOW_MS, &multibyte, &result).is_ok());
        assert!(original_stage5_sequence(NOW_MS, &multibyte, 42, &result).is_ok());
        assert!(stage5(NOW_MS, "/tab\tand\u{7}control", &result).is_ok());
        assert!(original_stage5_sequence(NOW_MS, "/tab\tand\u{7}control", 42, &result).is_ok());
        // Rejections keep the exact original variants, fields and priority.
        assert_same_failure("", NOW_MS, &result, "EmptyValue");
        assert_same_failure(
            &format!("/{}", "a".repeat(4_096)),
            NOW_MS,
            &result,
            "ValueTooLong",
        );
        assert_same_failure("fixture", NOW_MS, &result, "InvalidCommandResult");
        // Stage 5 #3 (workspace construction) precedes the #4 root pattern.
        assert_same_failure(&"a".repeat(4_097), NOW_MS, &result, "ValueTooLong");
        // Stage 5 #2 (generated-at clock) precedes #3 (workspace construction).
        assert_same_failure("", OUT_OF_RANGE_MS, &result, "InvalidTimestamp");
        assert_same_failure("/plain", OUT_OF_RANGE_MS, &result, "InvalidTimestamp");
    }

    /// Builds one synthetic oracle entry. These values deliberately escape at
    /// six bytes per scalar so a schema-valid map (128 entries of at most
    /// 2,048 scalars) can inflate past one mebibyte — far beyond anything the
    /// Core projection window emits. Synthetic Stage 5 oracle input only: no
    /// Core admission, no production-window reachability.
    fn oracle_entry(id: &str, control_scalars: usize, ascii_scalars: usize) -> serde_json::Value {
        let mut value = "\u{1}".repeat(control_scalars);
        value.push_str(&"a".repeat(ascii_scalars));
        assert!(value.chars().count() <= 2_048);
        serde_json::json!({
            "item_id": id,
            "value": value,
            "value_truncated": false,
        })
    }

    fn envelope_equivalent_len(result: &serde_json::Value, root: &str) -> usize {
        envelope_equivalent_status_bytes_v1(result, root, UUID, UUID, GENERATED_AT, 42)
    }

    /// Tunes a schema-valid oracle map so the equivalent envelope occupies
    /// exactly `target` serialized bytes. The search uses raw length probes
    /// that make no validity claim; only the final tuned map is validated,
    /// and its actual standalone size — not any hypothetical filler maximum —
    /// must stay inside Stage 4's byte and depth limits so no earlier
    /// rejection confounds the envelope-boundary comparison.
    fn tuned_oracle_result(target: usize, root: &str) -> serde_json::Value {
        for entries in (8..=84).rev() {
            let mut probe: Vec<_> = (0..entries)
                .map(|index| oracle_entry(&format!("i{index:02}"), 2_048, 0))
                .collect();
            probe.push(oracle_entry("ifill", 2_048, 0));
            probe.push(oracle_entry("jfill", 0, 0));
            let anchor = envelope_equivalent_len(&raw_oracle_result(probe), root);
            if anchor > target {
                continue;
            }
            let delta = target - anchor;
            if delta > 6 * 2_047 {
                continue;
            }
            // Growing the jfill value adds exactly `delta` serialized bytes;
            // constrain the actual final map against Stage 4's own bound.
            let mut final_probe: Vec<_> = (0..entries)
                .map(|index| oracle_entry(&format!("i{index:02}"), 2_048, 0))
                .collect();
            final_probe.push(oracle_entry("ifill", 2_048, 0));
            final_probe.push(oracle_entry("jfill", delta / 6, delta % 6));
            let final_raw = raw_oracle_result(final_probe.clone());
            if serde_json::to_vec(&final_raw).unwrap().len() > MAX_DIRECT_OUTPUT_BYTES_V1 {
                continue;
            }
            let result = status_oracle_result(final_probe);
            let len = envelope_equivalent_len(&result, root);
            assert_eq!(len, target, "oracle tuning missed the exact envelope size");
            return result;
        }
        panic!("no oracle entry count reached envelope target {target}");
    }

    #[test]
    fn stage5_byte_cut_matches_the_original_envelope_byte_check() {
        for target in [
            MAX_DIRECT_OUTPUT_BYTES_V1 - 1,
            MAX_DIRECT_OUTPUT_BYTES_V1,
            MAX_DIRECT_OUTPUT_BYTES_V1 + 1,
        ] {
            let result = tuned_oracle_result(target, "/oracle");
            // The component accounting equals the real serialized envelope.
            let value = envelope_value(result.clone(), "/oracle", 42);
            assert_eq!(
                serde_json::to_vec(&value).unwrap().len(),
                envelope_equivalent_len(&result, "/oracle")
            );
            let new = stage5(NOW_MS, "/oracle", &result);
            let old = original_stage5_sequence(NOW_MS, "/oracle", 42, &result);
            assert_eq!(
                new.is_ok(),
                old.is_ok(),
                "Stage 5 disagreed with the original sequence at envelope size {target}"
            );
            match (old, new) {
                (Ok(()), Ok(())) => {
                    assert!(target <= MAX_DIRECT_OUTPUT_BYTES_V1);
                }
                (Err(old_error), Err(new_error)) => {
                    assert!(target > MAX_DIRECT_OUTPUT_BYTES_V1);
                    let expected = ProtocolError::InvalidCommandResult {
                        command: "session.status".to_owned(),
                    };
                    assert_eq!(
                        (variant(&old_error), variant(&new_error)),
                        ("InvalidCommandResult", "InvalidCommandResult")
                    );
                    assert_eq!(old_error, expected);
                    assert_eq!(new_error, expected);
                }
                (old, new) => panic!("mismatched outcomes at {target}: {old:?} vs {new:?}"),
            }
        }
    }

    fn nested(depth: usize) -> serde_json::Value {
        if depth == 0 {
            return serde_json::json!(0);
        }
        serde_json::json!({ "a": nested(depth - 1) })
    }

    #[test]
    fn nesting_accounting_matches_the_protocol_document_boundary() {
        for (depth, expected) in [(0, 0), (1, 1), (5, 5), (63, 63), (64, 64), (65, 65)] {
            assert_eq!(json_nesting_levels_v1(&nested(depth)), expected);
        }
        // The Standard status schema has fixed non-recursive shapes, so deep
        // nesting is isolated against the protocol validator's generic/help
        // branch (genericObject), which runs the identical depth gate. The two
        // sides attribute their shared InvalidCommandResult variant to their
        // own commands — "help" for the oracle envelope, "session.status" for
        // the facts operation — so variant kind and boundary are compared,
        // with the attribution difference verified rather than claimed
        // byte-identical.
        for (result_depth, accepted) in [(62, true), (63, true), (64, false), (65, false)] {
            let deep = nested(result_depth);
            let oracle = serde_json::json!({
                "schema": "podway.output/v4",
                "request_id": UUID,
                "command": "help",
                "generated_at": GENERATED_AT,
                "result": deep.clone(),
                "warnings": [],
            });
            let oracle_outcome = match OutputEnvelopeV4::from_value(oracle) {
                Ok(_) => Ok::<(), ProtocolError>(()),
                Err(error) => {
                    assert_eq!(variant(&error), "InvalidCommandResult");
                    if let ProtocolError::InvalidCommandResult { command } = &error {
                        assert_eq!(command, "help");
                    } else {
                        unreachable!();
                    }
                    Err(error)
                }
            };
            assert_eq!(
                oracle_outcome.is_ok(),
                accepted,
                "protocol depth boundary changed at result depth {result_depth}"
            );
            let equivalent = stage5(NOW_MS, "/oracle", &deep);
            assert_eq!(
                equivalent.is_ok(),
                accepted,
                "Stage 5 depth check disagrees at result depth {result_depth}"
            );
            assert_eq!(json_nesting_levels_v1(&deep) < MAX_JSON_DEPTH_V1, accepted);
            if let Err(error) = equivalent {
                match error {
                    ProtocolError::InvalidCommandResult { command } => {
                        assert_eq!(command, "session.status");
                    }
                    _ => panic!("unexpected depth failure variant at {result_depth}"),
                }
            }
        }
    }
}
