//! Source-backed prepared starts recover the pinned identity inside the authoritative lookup.

use std::{cell::RefCell, time::Instant};

use podway_core::{
    ActorAttributionV2, ReasonV2, Revision, SessionLifecycle, Sha256Digest, TerminalDispositionV2,
    UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{
    CommandNameV1, DirectSessionOutputV1, OutputEnvelopeInputV4, OutputEnvelopeV4, RequestIdV1,
    Rfc3339MillisV1, SessionDeletionModeV1, SessionStartCommandV1, SessionStartReplaceV1,
    SessionStartSourceV1, SessionStartV1, SliceCommandV1, TerminalDispositionInputV1,
    WorkspaceOutputV1, canonical_direct_start_identity_v1,
};
use podway_store::{
    CanonicalExecutionJsonV1, GraphMutationErrorV2, GraphSessionStateV2, IdempotencyKeyV1,
    StoreErrorV1,
    direct_store::{DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1},
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::{
    direct_execution::{DirectExecutionErrorV1, DirectIdSourceV1},
    direct_mutation::check_session_fences,
    execution::{
        ExecutionClockV1, PreparedProcedureSourceV2,
        ProcedureV2StartPreparationErrorV1 as StartError, prepare_preset_procedure_source_v2,
    },
    native_execution::NativeDirectWorkspaceV1,
    workspace::DirectWorkspaceResolverV1,
};

#[derive(Clone, Debug)]
pub enum DirectStartCommandV1 {
    Start(SessionStartCommandV1),
    Replace(SessionStartReplaceV1),
}
impl DirectStartCommandV1 {
    pub(crate) fn input(&self) -> &SessionStartV1 {
        match self {
            Self::Start(command) => &command.start,
            Self::Replace(input) => &input.start,
        }
    }
    pub(crate) fn slice(&self) -> SliceCommandV1 {
        match self {
            Self::Start(command) => SliceCommandV1::SessionStart(command.clone()),
            Self::Replace(input) => SliceCommandV1::SessionStartReplace(input.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirectStartRequestV1 {
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub request_id: RequestIdV1,
    pub key: Option<IdempotencyKeyV1>,
    pub command: DirectStartCommandV1,
    pub deadline: Instant,
}

pub fn start_session_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectStartRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    start_session_with_context_v1(resolver, request, clock, ids, None)
}

pub(crate) fn start_session_with_context_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectStartRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    context: Option<&mut crate::direct_api::DirectInvocationContextV1>,
) -> Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let (resolved, store, binding) = resolver.resolve_existing_with_binding(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    let native = NativeDirectWorkspaceV1::new(resolved.clone());
    let input = request.command.input();
    let source = match &input.source {
        SessionStartSourceV1::Procedure { procedure } => native
            .prepare_workspace_procedure_source_v2(&binding, procedure)
            .map_err(StartError::Source),
        SessionStartSourceV1::Preset { preset } => prepare_preset_procedure_source_v2(preset)
            .map_err(StartError::Source)
            .and_then(|source| source.ok_or(StartError::PresetNotFound)),
    };
    if input.dry_run {
        let source = source.map_err(DirectExecutionErrorV1::Start)?;
        confirm_source(input, &source).map_err(DirectExecutionErrorV1::Start)?;
        validate_title(input)?;
        return store.read(request.deadline, |snapshot| {
            Ok((|| {
                resolver
                    .revalidate(&resolved, snapshot.binding())
                    .map_err(crate::workspace::DirectWorkspaceResolutionErrorV1::from)?;
                revalidate_source(&native, snapshot.binding(), input, &source)
                    .map_err(DirectExecutionErrorV1::Start)?;
                let now = clock.now();
                let view = snapshot.view(now)?;
                let disposed = view.graph().is_some_and(|graph| {
                    view.terminal_dispositions().iter().any(|record| {
                        record.terminal_session_revision() == graph.trace().revision()
                    })
                });
                verify_start_fences(&request.command, view.graph())?;
                let policy = current_policy(&request.command, view.graph(), disposed, true)?;
                let mut body = prepared_start_result(&source, true);
                if let (Some(graph), Some(policy)) = (view.graph(), policy) {
                    body["existing_session"] = existing_session(graph, policy);
                }
                Ok(OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
                    request_id: request.request_id,
                    command: CommandNameV1::new(request.command.slice().command_name())?,
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
                    result: body.as_object().expect("start result is an object").clone(),
                    warnings: Vec::new(),
                })?)
            })())
        })?;
    }
    let key = request
        .key
        .as_ref()
        .ok_or(DirectStoreErrorV1::InvalidInput {
            reason: "start requires an idempotency key",
        })?;
    let command = request.command.slice();
    let prepared_graph = crate::direct_api::prepare_graph_v1(&store, request.deadline, context)?;
    let source = RefCell::new(Some(source));
    let result = store.mutate_resolving_identity_typed(
        key,
        request.deadline,
        || clock.now(),
        |outcome| -> Result<_, DirectExecutionErrorV1> {
            let original_digest = if let Some(outcome) = outcome {
                if outcome.command() != command.command_name() {
                    return Err(DirectStoreErrorV1::IdempotencyKeyReused.into());
                }
                let output: OutputEnvelopeV4 = serde_json::from_str(outcome.response().as_str())
                    .map_err(|_| DirectStoreErrorV1::Unreadable)?;
                serde_json::from_value::<Sha256Digest>(
                    output.as_value()["result"]["procedure_digest"].clone(),
                )
                .map_err(|_| DirectStoreErrorV1::Unreadable)?
            } else {
                let mut retained = source.borrow_mut();
                match retained
                    .as_ref()
                    .expect("source preparation is retained before execution")
                {
                    Ok(source) => source.digest().clone(),
                    Err(_) => {
                        let Some(Err(error)) = retained.take() else {
                            unreachable!("the retained source preparation failed");
                        };
                        return Err(DirectExecutionErrorV1::Start(error));
                    }
                }
            };
            let canonical = canonical_direct_start_identity_v1(
                &command,
                resolved.store_identity().workspace_uuid(),
                &original_digest,
            )
            .map_err(|_| DirectStoreErrorV1::InvalidInput {
                reason: "invalid start semantic identity",
            })?;
            let digest =
                Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
                    .expect("SHA-256 produces a canonical digest");
            DirectMutationRequestV1::new(
                key.clone(),
                digest,
                command.command_name(),
                request.request_id.as_str(),
                DirectOutcomeScopeV1::Workspace,
            )
            .map_err(DirectExecutionErrorV1::from)
        },
        |transaction| -> Result<_, DirectExecutionErrorV1> {
            let prior = transaction.use_prepared_graph(prepared_graph.as_ref())?;
            let prior = prior.as_deref();
            verify_start_fences(&request.command, prior)?;
            if let (Some(graph), DirectStartCommandV1::Replace(replace)) = (prior, &request.command)
            {
                check_session_fences(
                    graph,
                    &replace.preconditions.expected_session_id,
                    replace.preconditions.expected_session_revision,
                )
                .map_err(DirectExecutionErrorV1::Store)?;
            }
            resolver
                .revalidate(&resolved, transaction.binding())
                .map_err(DirectExecutionErrorV1::from)?;
            let dispositions = transaction.terminal_dispositions()?;
            let disposed = prior.is_some_and(|graph| {
                dispositions
                    .iter()
                    .any(|record| record.terminal_session_revision() == graph.trace().revision())
            });
            let policy = current_policy(&request.command, prior, disposed, false)?;
            let source = source
                .borrow_mut()
                .take()
                .expect("source preparation is consumed once")
                .map_err(DirectExecutionErrorV1::Start)?;
            confirm_source(input, &source)
                .and_then(|()| revalidate_source(&native, transaction.binding(), input, &source))
                .map_err(DirectExecutionErrorV1::Start)?;
            validate_title(input)?;
            let session = ids.next_session_id();
            let snapshot_id = ids.next_procedure_snapshot_id();
            let now = transaction.now();
            let mut body = prepared_start_result(&source, false);
            let snapshot = source
                .into_snapshot(snapshot_id, now)
                .map_err(|error| DirectExecutionErrorV1::Start(StartError::Source(error)))?;
            let graph = GraphSessionStateV2::prepared(
                Revision::new(1),
                &input.task_title,
                snapshot,
                session.clone(),
                now,
            )
            .map_err(GraphMutationErrorV2::InvalidState)?;
            if let (Some(prior), Some(policy)) = (prior, policy) {
                body["existing_session"] = existing_session(prior, policy);
                match policy {
                    Policy::Delete => transaction
                        .clear_graph(prior.workspace_revision(), prior.trace().revision())?,
                    Policy::Archive => {
                        transaction
                            .archive_graph(prior.workspace_revision(), prior.trace().revision())?;
                    }
                    Policy::Supersede => {
                        let DirectStartCommandV1::Replace(replace) = &request.command else {
                            unreachable!("supersession is an explicit policy");
                        };
                        let SessionDeletionModeV1::Supersede { reason, actor } = &replace.mode
                        else {
                            unreachable!("validated supersession policy");
                        };
                        let reason_value =
                            ReasonV2::new(reason.clone()).map_err(GraphMutationErrorV2::Domain)?;
                        let cancelled = prior
                            .cancel_active_session_v2(
                                prior.trace().revision(),
                                prior
                                    .trace()
                                    .active_attempt()
                                    .ok_or(GraphMutationErrorV2::SessionNotRunning)?
                                    .attempt_id(),
                                reason_value,
                                now,
                            )?
                            .into_state();
                        transaction.replace_graph(
                            prior.workspace_revision(),
                            prior.trace().revision(),
                            &cancelled,
                        )?;
                        let disposition = TerminalDispositionV2::superseded(
                            cancelled.trace().session_id().clone(),
                            cancelled.trace().revision(),
                            reason,
                            session.clone(),
                            actor
                                .clone()
                                .map(ActorAttributionV2::new)
                                .transpose()
                                .map_err(GraphMutationErrorV2::Domain)?,
                            now,
                        )
                        .map_err(GraphMutationErrorV2::Domain)?;
                        transaction.record_disposition(&disposition)?;
                        transaction.archive_graph(
                            cancelled.workspace_revision(),
                            cancelled.trace().revision(),
                        )?;
                    }
                    Policy::Dispose => {
                        let DirectStartCommandV1::Replace(replace) = &request.command else {
                            unreachable!("disposition is an explicit policy");
                        };
                        let SessionDeletionModeV1::DisposeAndArchive { disposition, actor } =
                            &replace.mode
                        else {
                            unreachable!("validated disposition policy");
                        };
                        let actor = actor
                            .clone()
                            .map(ActorAttributionV2::new)
                            .transpose()
                            .map_err(GraphMutationErrorV2::Domain)?;
                        let record = make_disposition(prior, disposition, actor, now)?;
                        transaction.record_disposition(&record)?;
                        transaction
                            .archive_graph(prior.workspace_revision(), prior.trace().revision())?;
                    }
                }
            }
            body["session_id"] = json!(session);
            body["revision"] = json!(0);
            let output = OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
                request_id: request.request_id.clone(),
                command: CommandNameV1::new(command.command_name()).expect("static command"),
                generated_at: Rfc3339MillisV1::from_unix_millis(now.get()).map_err(|_| {
                    DirectStoreErrorV1::InvalidInput {
                        reason: "clock exceeds the public timestamp range",
                    }
                })?,
                workspace: Some(
                    WorkspaceOutputV1::new(
                        transaction.binding().identity().workspace_uuid().clone(),
                        resolved
                            .worktree()
                            .roots()
                            .worktree_root()
                            .display()
                            .as_str(),
                        transaction.next_workspace_sequence()?,
                    )
                    .map_err(|_| DirectStoreErrorV1::BindingMismatch)?,
                ),
                session: Some(DirectSessionOutputV1 {
                    id: session.clone(),
                    title: input.task_title.clone(),
                    lifecycle: SessionLifecycle::Prepared,
                    revision_before: prior
                        .as_ref()
                        .map_or(Revision::ZERO, |graph| graph.trace().revision()),
                    revision_after: Revision::ZERO,
                }),
                result: body.as_object().expect("start result is an object").clone(),
                warnings: Vec::new(),
            })
            .map_err(|_| DirectStoreErrorV1::InvalidInput {
                reason: "start success violates the canonical envelope",
            })?;
            let response = CanonicalExecutionJsonV1::new(
                canonicalize_json_v1(output.as_value())
                    .map_err(|_| DirectStoreErrorV1::Unreadable)?,
            )
            .map_err(|_| DirectStoreErrorV1::InvalidInput {
                reason: "start success exceeds the outcome bound",
            })?;
            transaction.create_graph(&graph)?;
            transaction.scope_created_session_outcome(&session)?;
            Ok(response)
        },
    );
    let result = result.map_err(podway_store::direct_store::DirectMutationFailureV1::into_error)?;
    serde_json::from_str(result.outcome().response().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable.into())
}

fn validate_title(input: &SessionStartV1) -> Result<(), DirectStoreErrorV1> {
    if input.task_title.trim().is_empty()
        || input.task_title.chars().count() > podway_protocol::MAX_SLICE_TASK_TITLE_SCALARS_V1
    {
        return Err(DirectStoreErrorV1::InvalidInput {
            reason: "invalid task title",
        });
    }
    Ok(())
}

fn confirm_source(
    input: &SessionStartV1,
    source: &PreparedProcedureSourceV2,
) -> Result<(), StartError> {
    if let Some(expected) = &input.expected_procedure_digest {
        if expected != source.digest() {
            return Err(StartError::ProcedureDigestMismatch {
                expected: expected.clone(),
                actual: source.digest().clone(),
            });
        }
    } else if !input.dry_run && matches!(input.source, SessionStartSourceV1::Procedure { .. }) {
        return Err(StartError::DigestConfirmationRequired {
            procedure_digest: source.digest().clone(),
        });
    }
    Ok(())
}

fn revalidate_source(
    native: &NativeDirectWorkspaceV1,
    binding: &podway_store::WorkspaceBindingV1,
    input: &SessionStartV1,
    source: &PreparedProcedureSourceV2,
) -> Result<(), StartError> {
    if let SessionStartSourceV1::Procedure { procedure } = &input.source {
        let actual = native
            .revalidate_procedure_source_v2(binding, procedure, source)
            .map_err(StartError::Source)?;
        if &actual != source.digest() {
            return Err(StartError::ProcedureDigestMismatch {
                expected: source.digest().clone(),
                actual,
            });
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Policy {
    Delete,
    Archive,
    Supersede,
    Dispose,
}

/// Verifies supplied plain-start fences against the current session inside the
/// caller's authoritative view. `None` fences keep the unfenced behavior, and
/// the errors match the fenced replacement path for the same state.
fn verify_start_fences(
    command: &DirectStartCommandV1,
    prior: Option<&GraphSessionStateV2>,
) -> Result<(), DirectExecutionErrorV1> {
    let DirectStartCommandV1::Start(start) = command else {
        return Ok(());
    };
    let Some(preconditions) = &start.preconditions else {
        return Ok(());
    };
    match prior {
        Some(graph) => check_session_fences(
            graph,
            &preconditions.expected_session_id,
            preconditions.expected_session_revision,
        )
        .map_err(DirectExecutionErrorV1::Store),
        None => Err(DirectExecutionErrorV1::Domain(
            podway_core::DomainError::SessionIdentityMismatch {
                expected: preconditions.expected_session_id.clone(),
                actual: None,
            },
        )),
    }
}

fn current_policy(
    command: &DirectStartCommandV1,
    graph: Option<&GraphSessionStateV2>,
    disposed: bool,
    dry_run: bool,
) -> Result<Option<Policy>, DirectExecutionErrorV1> {
    let Some(graph) = graph else {
        if let DirectStartCommandV1::Replace(input) = command {
            return Err(DirectExecutionErrorV1::Domain(
                podway_core::DomainError::SessionIdentityMismatch {
                    expected: input.preconditions.expected_session_id.clone(),
                    actual: None,
                },
            ));
        }
        return Ok(None);
    };
    let lifecycle = graph.trace().lifecycle();
    if let DirectStartCommandV1::Start(_) = command {
        if matches!(
            lifecycle,
            SessionLifecycle::Completed | SessionLifecycle::Cancelled
        ) && disposed
        {
            return Ok(Some(Policy::Archive));
        }
        return Err(DirectExecutionErrorV1::StartDecisionRequired {
            session_id: graph.trace().session_id().clone(),
            revision: graph.trace().revision(),
            lifecycle,
            current_terminal_disposition: disposed,
        });
    }
    let DirectStartCommandV1::Replace(input) = command else {
        unreachable!("plain start was handled above");
    };
    check_session_fences(
        graph,
        &input.preconditions.expected_session_id,
        input.preconditions.expected_session_revision,
    )?;
    let conflict = || {
        DirectExecutionErrorV1::Store(DirectStoreErrorV1::State(
            StoreErrorV1::SessionStartStateConflictV1 {
                lifecycle,
                current_terminal_disposition: disposed,
            },
        ))
    };
    let policy = match &input.mode {
        SessionDeletionModeV1::Delete {
            confirmed,
            progress_summary,
        } => {
            if !dry_run && !confirmed {
                return Err(DirectStoreErrorV1::InvalidInput {
                    reason: "start deletion requires confirmation",
                }
                .into());
            }
            match lifecycle {
                SessionLifecycle::Prepared if progress_summary.is_none() => {}
                SessionLifecycle::Running
                    if progress_summary.as_ref().is_some_and(|summary| {
                        !summary.trim().is_empty()
                            && summary.chars().count()
                                <= podway_protocol::MAX_SLICE_LIFECYCLE_TEXT_SCALARS_V1
                    }) => {}
                SessionLifecycle::Prepared | SessionLifecycle::Running => {
                    return Err(DirectStoreErrorV1::InvalidInput {
                        reason: "deletion progress summary does not match the current lifecycle",
                    }
                    .into());
                }
                _ => return Err(conflict()),
            }
            Policy::Delete
        }
        SessionDeletionModeV1::Supersede { reason, actor } => {
            if lifecycle != SessionLifecycle::Running {
                return Err(conflict());
            }
            ReasonV2::new(reason.clone()).map_err(DirectExecutionErrorV1::Domain)?;
            actor
                .clone()
                .map(ActorAttributionV2::new)
                .transpose()
                .map_err(DirectExecutionErrorV1::Domain)?;
            Policy::Supersede
        }
        SessionDeletionModeV1::DisposeAndArchive { disposition, actor } => {
            if !matches!(
                lifecycle,
                SessionLifecycle::Completed | SessionLifecycle::Cancelled
            ) || disposed
            {
                return Err(conflict());
            }
            actor
                .clone()
                .map(ActorAttributionV2::new)
                .transpose()
                .map_err(DirectExecutionErrorV1::Domain)?;
            crate::direct_mutation::validate_disposition_input(disposition)?;
            Policy::Dispose
        }
        SessionDeletionModeV1::Eligible if lifecycle == SessionLifecycle::Prepared => {
            Policy::Delete
        }
        SessionDeletionModeV1::Eligible
            if matches!(
                lifecycle,
                SessionLifecycle::Completed | SessionLifecycle::Cancelled
            ) && disposed =>
        {
            Policy::Archive
        }
        SessionDeletionModeV1::Eligible => {
            return Err(
                DirectStoreErrorV1::State(StoreErrorV1::SessionResetNotEligibleV1 {
                    lifecycle,
                    current_terminal_disposition: disposed,
                })
                .into(),
            );
        }
        SessionDeletionModeV1::Force { progress_summary }
            if !progress_summary.trim().is_empty()
                && progress_summary.chars().count()
                    <= podway_protocol::MAX_SLICE_LIFECYCLE_TEXT_SCALARS_V1 =>
        {
            Policy::Delete
        }
        _ => return Err(conflict()),
    };
    Ok(Some(policy))
}

fn prepared_start_result(source: &PreparedProcedureSourceV2, dry_run: bool) -> Value {
    // Metadata comes from the already admitted canonical source, without a synthetic identity.
    json!({"schema":"podway.session-start-result/v4", "procedure_schema":"podway.procedure/v2", "procedure_digest":source.digest(),
        "dry_run":dry_run, "session_state":"prepared", "goal_tracking":source.goal_tracking(),
        "goal_defined":false, "active_attempt":null, "goal_revision":null, "entry_graph_node_id":source.entry_graph_node_id()})
}

fn existing_session(graph: &GraphSessionStateV2, policy: Policy) -> Value {
    json!({"session_id":graph.trace().session_id(), "session_revision":graph.trace().revision(),
        "lifecycle":match graph.trace().lifecycle() { SessionLifecycle::Prepared => "prepared", SessionLifecycle::Running => "running",
            SessionLifecycle::Completed => "completed", SessionLifecycle::Cancelled => "cancelled" },
        "action":match policy { Policy::Delete => "delete", Policy::Archive => "archive", Policy::Supersede => "supersede_archive", Policy::Dispose => "dispose_archive" }})
}

fn make_disposition(
    graph: &GraphSessionStateV2,
    input: &TerminalDispositionInputV1,
    actor: Option<ActorAttributionV2>,
    now: UnixMillis,
) -> Result<TerminalDispositionV2, DirectStoreErrorV1> {
    Ok(match input {
        TerminalDispositionInputV1::HandedOff { summary, reference } => {
            TerminalDispositionV2::handed_off(
                graph.trace().session_id().clone(),
                graph.trace().revision(),
                summary,
                reference,
                actor,
                now,
            )
        }
        TerminalDispositionInputV1::NotRequired { reason } => TerminalDispositionV2::not_required(
            graph.trace().session_id().clone(),
            graph.trace().revision(),
            reason,
            actor,
            now,
        ),
    }
    .map_err(GraphMutationErrorV2::Domain)?)
}
