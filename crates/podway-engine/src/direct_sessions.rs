//! Reset, archive and purge retain the direct success outcome and their domain policies.

use std::time::Instant;

use podway_core::{DomainError, SessionLifecycle, Sha256Digest, WorkspaceId};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{
    CommandNameV1, OutputEnvelopeInputV4, OutputEnvelopeV4, RequestIdV1, Rfc3339MillisV1,
    SessionArchivePurgeV1, SessionArchiveV1, SessionDeletionModeV1, SessionResetV1, SliceCommandV1,
    WorkspaceOutputV1, canonical_direct_slice_mutation_identity_v1,
};
use podway_store::{
    CanonicalExecutionJsonV1, GraphMutationErrorV2, IdempotencyKeyV1, StoreErrorV1,
    direct_store::{DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1},
};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::{
    direct_execution::DirectExecutionErrorV1, direct_mutation::check_session_fences,
    direct_read_service::archive_summary, execution::ExecutionClockV1,
    workspace::DirectWorkspaceResolverV1,
};

#[derive(Clone, Debug)]
pub enum DirectSessionCommandV1 {
    Reset(SessionResetV1),
    Archive(SessionArchiveV1),
    Purge(SessionArchivePurgeV1),
}

impl DirectSessionCommandV1 {
    pub(crate) fn slice(&self) -> SliceCommandV1 {
        match self {
            Self::Reset(input) => SliceCommandV1::SessionReset(input.clone()),
            Self::Archive(input) => SliceCommandV1::SessionArchive(input.clone()),
            Self::Purge(input) => SliceCommandV1::SessionArchivePurge(input.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirectSessionRequestV1 {
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub request_id: RequestIdV1,
    pub key: Option<IdempotencyKeyV1>,
    pub command: DirectSessionCommandV1,
    pub deadline: Instant,
}

pub fn execute_session_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectSessionRequestV1,
    clock: &impl ExecutionClockV1,
) -> Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let (resolved, store) = resolver.resolve_existing(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    let command = request.command.slice();
    if let DirectSessionCommandV1::Reset(input) = &request.command
        && input.dry_run
    {
        return store.read(request.deadline, |snapshot| {
            Ok((|| {
                resolver
                    .revalidate(&resolved, snapshot.binding())
                    .map_err(crate::workspace::DirectWorkspaceResolutionErrorV1::from)?;
                let current = snapshot.current_graph()?.ok_or_else(|| {
                    DirectStoreErrorV1::Domain(GraphMutationErrorV2::Domain(
                        podway_core::DomainError::SessionIdentityMismatch {
                            expected: input.preconditions.expected_session_id.clone(),
                            actual: None,
                        },
                    ))
                })?;
                check_session_fences(
                    &current,
                    &input.preconditions.expected_session_id,
                    input.preconditions.expected_session_revision,
                )?;
                let now = clock.now();
                let view = snapshot.view(now)?;
                let graph = view
                    .graph()
                    .ok_or(crate::v2_read_service::GraphViewErrorV2::MissingGraphSession)?;
                check_session_fences(
                    graph,
                    &input.preconditions.expected_session_id,
                    input.preconditions.expected_session_revision,
                )?;
                let disposed = view
                    .terminal_dispositions()
                    .iter()
                    .any(|record| record.terminal_session_revision() == graph.trace().revision());
                let body = reset_result(graph, &input.mode, disposed, true)?;
                Ok(OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
                    request_id: request.request_id,
                    command: CommandNameV1::new(command.command_name())?,
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
                    result: body.as_object().expect("reset result is an object").clone(),
                    warnings: Vec::new(),
                })?)
            })())
        })?;
    }
    let key = request.key.ok_or(DirectStoreErrorV1::InvalidInput {
        reason: "mutation requires an idempotency key",
    })?;
    let canonical = canonical_direct_slice_mutation_identity_v1(
        &command,
        resolved.store_identity().workspace_uuid(),
    )
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "invalid session semantic identity",
    })?;
    let digest = Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
        .expect("SHA-256 produces a canonical digest");
    let scope = match &request.command {
        DirectSessionCommandV1::Reset(_) => DirectOutcomeScopeV1::Workspace,
        DirectSessionCommandV1::Archive(input) => {
            DirectOutcomeScopeV1::Session(input.preconditions.expected_session_id.clone())
        }
        DirectSessionCommandV1::Purge(_) => DirectOutcomeScopeV1::Workspace,
    };
    let mutation = DirectMutationRequestV1::new(
        key,
        digest,
        command.command_name(),
        request.request_id.as_str(),
        scope,
    )?;
    let prepared = if matches!(request.command, DirectSessionCommandV1::Purge(_)) {
        None
    } else {
        store.prepare_graph(request.deadline)?
    };
    let result = store.mutate_typed(&mutation, request.deadline, || clock.now(), |transaction| -> Result<_, DirectExecutionErrorV1> {
        resolver.revalidate(&resolved, transaction.binding()).map_err(DirectExecutionErrorV1::from)?;
        let body = match &request.command {
            DirectSessionCommandV1::Purge(input) => {
                if !input.confirmed { return Err(DirectStoreErrorV1::InvalidInput { reason: "archive purge requires confirmation" }.into()); }
                // Store checks the archived session revision atomically before deleting any row.
                let archived = transaction.archive_summary(&input.session_id)?
                    .ok_or_else(|| StoreErrorV1::SessionArchiveNotFoundV1 { session_id: input.session_id.clone() })?;
                if archived.session_revision() != input.expected_session_revision {
                    return Err(GraphMutationErrorV2::SessionRevisionConflict {
                        expected: input.expected_session_revision, actual: archived.session_revision(),
                    }.into());
                }
                transaction.now();
                let purged = transaction.purge_archive(&input.session_id, input.expected_session_revision)?;
                json!({"schema":"podway.session-archive-purge-result/v1", "purged":true, "session":archive_summary(&purged)})
            }
            DirectSessionCommandV1::Reset(input) => {
                let current = transaction.use_prepared_graph(prepared.as_ref())?;
                let current = current.as_ref().ok_or_else(|| GraphMutationErrorV2::Domain(DomainError::SessionIdentityMismatch {
                    expected: input.preconditions.expected_session_id.clone(), actual: None }))?;
                let graph = current.as_ref();
                check_session_fences(graph, &input.preconditions.expected_session_id, input.preconditions.expected_session_revision)?;
                let disposed = transaction.terminal_dispositions()?.iter().any(|record| record.terminal_session_revision() == graph.trace().revision());
                let body = reset_result(graph, &input.mode, disposed, false)?;
                transaction.now();
                transaction.clear_graph(graph.workspace_revision(), graph.trace().revision())?;
                body
            }
            DirectSessionCommandV1::Archive(input) => {
                let current = transaction.use_prepared_graph(prepared.as_ref())?;
                let current = current.as_ref().ok_or_else(|| GraphMutationErrorV2::Domain(DomainError::SessionIdentityMismatch {
                    expected: input.preconditions.expected_session_id.clone(), actual: None }))?;
                let graph = current.as_ref();
                check_session_fences(graph, &input.preconditions.expected_session_id, input.preconditions.expected_session_revision)?;
                let disposed = transaction.terminal_dispositions()?.iter().any(|record| record.terminal_session_revision() == graph.trace().revision());
                if !matches!(graph.trace().lifecycle(), SessionLifecycle::Completed | SessionLifecycle::Cancelled) || !disposed {
                    return Err(StoreErrorV1::SessionArchiveNotEligibleV1 { lifecycle: graph.trace().lifecycle(), current_terminal_disposition: disposed }.into());
                }
                transaction.now();
                let archived = transaction.archive_graph(graph.workspace_revision(), graph.trace().revision())?;
                json!({"schema":"podway.session-archive-result/v2", "session_id":archived.session_id(),
                    "session_revision":archived.session_revision(), "activity":"inactive", "lifecycle":lifecycle(archived.lifecycle())})
            }
        };
        let response = OutputEnvelopeV4::new(OutputEnvelopeInputV4 { request_id: request.request_id,
            command: CommandNameV1::new(command.command_name()).expect("static command"),
            generated_at: Rfc3339MillisV1::from_unix_millis(transaction.now().get())
                .map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "clock exceeds the public timestamp range" })?,
            workspace: Some(WorkspaceOutputV1::new(transaction.binding().identity().workspace_uuid().clone(),
                resolved.worktree().roots().worktree_root().display().as_str(), transaction.next_workspace_sequence()?)
                .map_err(|_| DirectStoreErrorV1::BindingMismatch)?), session: None,
            result: body.as_object().expect("session result is an object").clone(), warnings: Vec::new() })
                .map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "session success violates the canonical envelope" })?;
        CanonicalExecutionJsonV1::new(podway_core::canonicalize_json_v1(response.as_value()).map_err(|_| DirectStoreErrorV1::Unreadable)?)
            .map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "session success exceeds the outcome bound" }).map_err(DirectExecutionErrorV1::from)
    });
    let result = result.map_err(podway_store::direct_store::DirectMutationFailureV1::into_error)?;
    serde_json::from_str(result.outcome().response().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable.into())
}

fn lifecycle(lifecycle: SessionLifecycle) -> &'static str {
    match lifecycle {
        SessionLifecycle::Prepared => "prepared",
        SessionLifecycle::Running => "running",
        SessionLifecycle::Completed => "completed",
        SessionLifecycle::Cancelled => "cancelled",
    }
}

fn reset_result(
    graph: &podway_store::GraphSessionStateV2,
    mode: &SessionDeletionModeV1,
    disposed: bool,
    dry_run: bool,
) -> Result<serde_json::Value, DirectStoreErrorV1> {
    let (eligible, action) = match graph.trace().lifecycle() {
        SessionLifecycle::Prepared => (true, "none"),
        SessionLifecycle::Running => (false, "force"),
        SessionLifecycle::Completed | SessionLifecycle::Cancelled => (
            disposed,
            if disposed {
                "none"
            } else {
                "record_disposition"
            },
        ),
    };
    let mode = match mode {
        SessionDeletionModeV1::Eligible => "eligible",
        SessionDeletionModeV1::Force { progress_summary }
            if !progress_summary.trim().is_empty()
                && progress_summary.chars().count()
                    <= podway_protocol::MAX_SLICE_LIFECYCLE_TEXT_SCALARS_V1 =>
        {
            "force"
        }
        SessionDeletionModeV1::Force { .. } => {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "invalid reset progress summary",
            });
        }
        SessionDeletionModeV1::LegacyConfirmed => {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "legacy reset is not a current-generation session reset",
            });
        }
        _ => {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "session reset rejects start resolutions",
            });
        }
    };
    if !dry_run && mode == "eligible" && !eligible {
        return Err(StoreErrorV1::SessionResetNotEligibleV1 {
            lifecycle: graph.trace().lifecycle(),
            current_terminal_disposition: disposed,
        }
        .into());
    }
    Ok(
        json!({"schema":"podway.session-reset-result/v2", "dry_run":dry_run, "mode":mode,
        "session_id":graph.trace().session_id(), "session_revision":graph.trace().revision(), "lifecycle":lifecycle(graph.trace().lifecycle()),
        "current_terminal_disposition":disposed, "eligible":eligible, "required_action":if dry_run { action } else { "none" }, "reset":!dry_run}),
    )
}
