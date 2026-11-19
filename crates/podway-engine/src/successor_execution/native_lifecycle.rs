//! Current and archived lifecycle operations over the successor Store.

use super::*;
use crate::{
    direct_execution::DirectExecutionErrorV1,
    direct_mutation::validate_disposition_input,
    direct_read_service::terminal_disposition,
    direct_sessions::{DirectSessionCommandV1, DirectSessionRequestV1, reset_result_for_session},
    direct_start::make_bound_disposition,
    execution::ExecutionClockV1,
    workspace::{DirectWorkspaceResolutionErrorV1, DirectWorkspaceResolverV1, ResolvedWorkspaceV1},
};
use podway_core::{ActorAttributionV2, SessionLifecycle};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{
    OutputEnvelopeV4, ProcedureV2MutationCommandV1, SessionIdentityPreconditionsWireV1,
    SessionTerminalDispositionV1, canonical_direct_slice_mutation_identity_v1,
    canonical_procedure_v2_command_identity_v1,
};
use podway_store::{StoreErrorV1, successor_store::SuccessorSessionMetadata};
use std::time::Instant;

pub(crate) fn dispatch_successor_session<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectSessionRequestV1,
    clock: &impl ExecutionClockV1,
) -> std::result::Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let name = request.command.slice().command_name();
    if let DirectSessionCommandV1::Reset(input) = &request.command
        && input.dry_run
    {
        return resolver.resolve_existing_and_read(
            request.selector,
            request.expected_workspace_id.as_ref(),
            request.deadline,
            |resolved, snapshot| {
                resolver
                    .revalidate(resolved, snapshot.binding())
                    .map_err(DirectWorkspaceResolutionErrorV1::from)?;
                let (state, _) = require_current(
                    snapshot.successor_state(admit_candidate_definition)?,
                    &input.preconditions,
                )?;
                let fence = state.fence();
                let disposed = snapshot
                    .successor_disposition(&fence.session, fence.revision)?
                    .is_some();
                let body = reset_result(&state, &input.mode, disposed, true)?;
                lifecycle_output(
                    request.request_id,
                    name,
                    clock.now(),
                    snapshot.committed_sequence()?,
                    resolved,
                    None,
                    body,
                )
            },
        );
    }
    let (resolved, store) = resolver.resolve_existing(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    let workspace = resolved.store_identity().workspace_uuid();
    let key = request.key.ok_or(DirectExecutionErrorV1::InvalidRequest)?;
    let command = request.command.slice();
    let canonical = canonical_direct_slice_mutation_identity_v1(&command, workspace)
        .map_err(|_| DirectExecutionErrorV1::InvalidRequest)?;
    let digest = canonical_digest(&canonical)?;
    let scope = match &request.command {
        DirectSessionCommandV1::Archive(input) => {
            DirectOutcomeScopeV1::Session(input.preconditions.expected_session_id.clone())
        }
        DirectSessionCommandV1::Reset(_) | DirectSessionCommandV1::Purge(_) => {
            DirectOutcomeScopeV1::Workspace
        }
    };
    let mutation =
        DirectMutationRequestV1::new(key, digest, name, request.request_id.as_str(), scope)?;
    let outcome = store.mutate_typed(&mutation, request.deadline, || clock.now(), |transaction| {
        resolver.revalidate(&resolved, transaction.binding()).map_err(DirectWorkspaceResolutionErrorV1::from)?;
        let body = match &request.command {
            DirectSessionCommandV1::Reset(input) => {
                let (state, _) = require_current(transaction.successor_state(admit_candidate_definition)?, &input.preconditions)?;
                let fence = state.fence();
                let disposed = transaction.successor_disposition(&fence.session, fence.revision)?.is_some();
                let body = reset_result(&state, &input.mode, disposed, false)?;
                transaction.now();
                transaction.delete_successor_session(&fence.session, fence.revision)?;
                body
            }
            DirectSessionCommandV1::Archive(input) => {
                let (state, _) = require_current(transaction.successor_state(admit_candidate_definition)?, &input.preconditions)?;
                let fence = state.fence();
                let disposed = transaction.successor_disposition(&fence.session, fence.revision)?.is_some();
                if !is_terminal(state.lifecycle()) || !disposed {
                    return Err(StoreErrorV1::SessionArchiveNotEligibleV1 {
                        lifecycle: state.lifecycle(), current_terminal_disposition: disposed,
                    }.into());
                }
                transaction.now();
                transaction.archive_successor_session(&fence.session, fence.revision)?;
                json!({
                    "schema": "podway.successor-session-archive-result/v1",
                    "session_id": fence.session, "session_revision": fence.revision,
                    "lifecycle": lifecycle(state.lifecycle()), "activity": "inactive",
                    "retained_group_count": state.group_history().len(),
                })
            }
            DirectSessionCommandV1::Purge(input) => {
                if !input.confirmed {
                    return Err(DirectStoreErrorV1::InvalidInput { reason: "archive purge requires confirmation" }.into());
                }
                let (state, _) = transaction.archived_successor_state(&input.session_id, admit_candidate_definition)?
                    .ok_or_else(|| StoreErrorV1::SessionArchiveNotFoundV1 { session_id: input.session_id.clone() })?;
                let fence = state.fence();
                if fence.revision != input.expected_session_revision {
                    return Err(GraphMutationErrorV2::SessionRevisionConflict {
                        expected: input.expected_session_revision, actual: fence.revision,
                    }.into());
                }
                let metadata = transaction.successor_metadata(&fence.session)?
                    .ok_or(DirectStoreErrorV1::Unreadable)?;
                let summary = archive_summary(&state, metadata)?;
                transaction.now();
                transaction.purge_successor_archive(&input.session_id, fence.revision)?;
                json!({
                    "schema": "podway.successor-session-archive-purge-result/v1",
                    "purged": true, "session": summary, "deleted_group_count": state.group_history().len(),
                })
            }
        };
        let output = lifecycle_output(
            request.request_id, name, transaction.now(), transaction.next_workspace_sequence()?,
            &resolved, None, body,
        )?;
        canonical_lifecycle_output(output)
    }).map_err(podway_store::direct_store::DirectMutationFailureV1::into_error)?;
    serde_json::from_str(outcome.outcome().response().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable.into())
}

pub(crate) struct DirectSuccessorDispositionRequestV1 {
    pub request_id: RequestIdV1,
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub deadline: Instant,
    pub key: IdempotencyKeyV1,
    pub input: SessionTerminalDispositionV1,
}

pub(crate) fn dispatch_successor_disposition<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectSuccessorDispositionRequestV1,
    clock: &impl ExecutionClockV1,
) -> std::result::Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let (resolved, store) = resolver.resolve_existing(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    let command = ProcedureV2MutationCommandV1::SessionTerminalDisposition(request.input);
    let name = command.command_name();
    let identity = canonical_procedure_v2_command_identity_v1(
        &command,
        resolved.store_identity().workspace_uuid(),
    )
    .map_err(|_| DirectExecutionErrorV1::InvalidRequest)?;
    let ProcedureV2MutationCommandV1::SessionTerminalDisposition(input) = command else {
        unreachable!("disposition request creates a disposition command");
    };
    let mutation = DirectMutationRequestV1::new(
        request.key,
        canonical_digest(&identity)?,
        name,
        request.request_id.as_str(),
        DirectOutcomeScopeV1::Session(input.preconditions.expected_session_id.clone()),
    )?;
    let result = store
        .mutate_typed(
            &mutation,
            request.deadline,
            || clock.now(),
            |transaction| {
                resolver
                    .revalidate(&resolved, transaction.binding())
                    .map_err(DirectWorkspaceResolutionErrorV1::from)?;
                let (state, _) = require_current(
                    transaction.successor_state(admit_candidate_definition)?,
                    &input.preconditions,
                )?;
                if !is_terminal(state.lifecycle()) {
                    return Err(DirectExecutionErrorV1::SessionNotTerminal);
                }
                validate_disposition_input(&input.disposition)?;
                let actor = input
                    .actor
                    .map(ActorAttributionV2::new)
                    .transpose()
                    .map_err(DirectExecutionErrorV1::Domain)?;
                let fence = state.fence();
                let title = transaction
                    .successor_metadata(&fence.session)?
                    .ok_or(DirectStoreErrorV1::Unreadable)?
                    .task_title;
                let now = transaction.now();
                let disposition = make_bound_disposition(
                    &fence.session,
                    fence.revision,
                    &input.disposition,
                    actor,
                    now,
                )?;
                transaction.dispose_successor_session(&disposition)?;
                let output = lifecycle_output(
                    request.request_id,
                    name,
                    now,
                    transaction.next_workspace_sequence()?,
                    &resolved,
                    Some(DirectSessionOutputV1 {
                        id: fence.session.clone(),
                        title,
                        lifecycle: state.lifecycle(),
                        revision_before: fence.revision,
                        revision_after: fence.revision,
                    }),
                    json!({
                        "schema": "podway.terminal-disposition-result/v2",
                        "session_id": fence.session, "session_revision": fence.revision,
                        "disposition": terminal_disposition(&disposition)?,
                    }),
                )?;
                canonical_lifecycle_output(output)
            },
        )
        .map_err(podway_store::direct_store::DirectMutationFailureV1::into_error)?;
    serde_json::from_str(result.outcome().response().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable.into())
}

fn require_current(
    current: Option<(ExecutionSessionV3, CandidateSources)>,
    preconditions: &SessionIdentityPreconditionsWireV1,
) -> std::result::Result<(ExecutionSessionV3, CandidateSources), DirectExecutionErrorV1> {
    let expected = &preconditions.expected_session_id;
    let (state, sources) = match current {
        Some((state, sources)) if &state.fence().session == expected => (state, sources),
        other => {
            return Err(DirectExecutionErrorV1::Domain(
                DomainError::SessionIdentityMismatch {
                    expected: expected.clone(),
                    actual: other.map(|(state, _)| state.fence().session),
                },
            ));
        }
    };
    let current = state.fence().revision;
    if current != preconditions.expected_session_revision {
        return Err(GraphMutationErrorV2::SessionRevisionConflict {
            expected: preconditions.expected_session_revision,
            actual: current,
        }
        .into());
    }
    Ok((state, sources))
}

fn archive_summary(
    state: &ExecutionSessionV3,
    metadata: SuccessorSessionMetadata,
) -> std::result::Result<Value, DirectExecutionErrorV1> {
    let archive = metadata.archived.ok_or(DirectStoreErrorV1::Unreadable)?;
    let fence = state.fence();
    Ok(json!({
        "session_id": fence.session, "task_title": metadata.task_title,
        "lifecycle": lifecycle(state.lifecycle()), "session_revision": fence.revision,
        "archive_slot": archive.slot, "archived_at_ms": archive.at.get(),
        "archived_workspace_revision": archive.workspace_sequence,
    }))
}

fn reset_result(
    state: &ExecutionSessionV3,
    mode: &podway_protocol::SessionDeletionModeV1,
    disposed: bool,
    dry_run: bool,
) -> std::result::Result<Value, DirectExecutionErrorV1> {
    let fence = state.fence();
    let mut body = reset_result_for_session(
        &fence.session,
        fence.revision,
        state.lifecycle(),
        mode,
        disposed,
        dry_run,
    )?;
    body["schema"] = json!("podway.successor-session-reset-result/v1");
    body["group_count_to_delete"] = json!(state.group_history().len());
    Ok(body)
}

pub(super) fn lifecycle_output(
    request_id: RequestIdV1,
    command: &str,
    now: UnixMillis,
    sequence: u64,
    resolved: &ResolvedWorkspaceV1,
    session: Option<DirectSessionOutputV1>,
    body: Value,
) -> std::result::Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let Value::Object(result) = body else {
        return Err(DirectStoreErrorV1::Unreadable.into());
    };
    Ok(OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
        request_id,
        command: CommandNameV1::new(command)?,
        generated_at: Rfc3339MillisV1::from_unix_millis(now.get())?,
        workspace: Some(WorkspaceOutputV1::new(
            resolved.store_identity().workspace_uuid().clone(),
            resolved
                .worktree()
                .roots()
                .worktree_root()
                .display()
                .as_str(),
            sequence,
        )?),
        session,
        result,
        warnings: Vec::new(),
    })?)
}

pub(super) fn canonical_lifecycle_output(
    output: OutputEnvelopeV4,
) -> std::result::Result<CanonicalExecutionJsonV1, DirectExecutionErrorV1> {
    validate_direct_outcome_lookup_headroom_v1(output.as_value())
        .map_err(|_| SuccessorExecutionError::OutcomeLookupBudget)?;
    Ok(canonical_response(output.as_value())?)
}

fn is_terminal(lifecycle: SessionLifecycle) -> bool {
    matches!(
        lifecycle,
        SessionLifecycle::Completed | SessionLifecycle::Cancelled
    )
}

fn lifecycle(lifecycle: SessionLifecycle) -> &'static str {
    match lifecycle {
        SessionLifecycle::Prepared => "prepared",
        SessionLifecycle::Running => "running",
        SessionLifecycle::Completed => "completed",
        SessionLifecycle::Cancelled => "cancelled",
    }
}

pub(crate) fn dispatch_successor_archive_read<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: crate::direct_execution::DirectReadRequestV1,
    clock: &impl ExecutionClockV1,
) -> std::result::Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    use crate::direct_execution::DirectReadCommandV1;
    use crate::v2_read_service::GraphStatusTierV2;
    resolver.resolve_existing_and_read(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
        |resolved, snapshot| {
            resolver.revalidate(resolved, snapshot.binding()).map_err(DirectWorkspaceResolutionErrorV1::from)?;
            let sequence = snapshot.committed_sequence()?;
            let (body, session) = match &request.command {
                DirectReadCommandV1::ArchiveList => {
                    let limit = podway_protocol::SESSION_ARCHIVE_LIST_WINDOW_V2;
                    let archived = snapshot.successor_archives(limit)?;
                    let summaries = archived.sessions.iter().map(archive_list_summary).collect::<Vec<_>>();
                    let oldest = archived.oldest.as_ref().map(archive_list_summary);
                    (json!({"schema":"podway.session-archive-list-result/v2", "count":archived.count,
                        "truncated":archived.count > limit as u64, "oldest":oldest, "sessions":summaries}), None)
                }
                DirectReadCommandV1::ArchiveShow { session_id, tier, history_before } => {
                    if let Some(expected) = request.expected_session_id
                        && &expected != session_id
                    {
                        return Err(DirectExecutionErrorV1::Domain(DomainError::SessionIdentityMismatch {
                            expected, actual: Some(session_id.clone()),
                        }));
                    }
                    let (state, sources) = snapshot.archived_successor_state(session_id, admit_candidate_definition)?
                        .ok_or_else(|| StoreErrorV1::SessionArchiveNotFoundV1 { session_id: session_id.clone() })?;
                    let fence = state.fence();
                    let metadata = snapshot.successor_metadata(&fence.session)?.ok_or(DirectStoreErrorV1::Unreadable)?;
                    let title = metadata.task_title.clone();
                    let disposition = snapshot.successor_disposition(&fence.session, fence.revision)?.ok_or(DirectStoreErrorV1::Unreadable)?;
                    let observation = super::presentation::public_observation(
                        snapshot.binding().identity().workspace_uuid(), &title, &state, &sources,
                        sequence, podway_protocol::ContractGenerationV1::Generation2,
                        super::presentation::ObservationActivityV1::Archived,
                    )?;
                    let mut body = json!({
                        "schema": "podway.successor-session-archive-show-result/v1",
                        "archive": archive_summary(&state, metadata)?,
                        "disposition": terminal_disposition(&disposition)?,
                        "observation": observation,
                    });
                    if *tier == GraphStatusTierV2::Verbose {
                        let mut page = super::presentation::verbose_status(
                            body["observation"].take(), &state, *history_before,
                        )?;
                        body["observation"] = page["observation"].take();
                        body["attempt_history"] = json!({
                            "attempts": page["attempts"].take(),
                            "trace_length": page["trace_length"].take(),
                            "history_before": page["history_before"].take(),
                        });
                    }
                    (body, Some(DirectSessionOutputV1 {
                        id: fence.session, title, lifecycle: state.lifecycle(),
                        revision_before: fence.revision, revision_after: fence.revision,
                    }))
                }
                _ => return Err(DirectExecutionErrorV1::InvalidRequest),
            };
            lifecycle_output(
                request.request_id, request.command.command_name(), clock.now(), sequence,
                resolved, session, body,
            )
        },
    )
}

fn archive_list_summary(summary: &podway_store::successor_store::SuccessorArchiveSummary) -> Value {
    json!({
        "session_id": summary.session, "task_title": summary.task_title,
        "lifecycle": lifecycle(summary.lifecycle), "session_revision": summary.revision,
        "archive_slot": summary.archive.slot, "archived_at_ms": summary.archive.at.get(),
        "archived_workspace_revision": summary.archive.workspace_sequence,
    })
}
