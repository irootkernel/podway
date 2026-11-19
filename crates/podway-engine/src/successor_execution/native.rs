//! Typed Direct dispatch for successor cursor transitions.

use super::*;
use crate::{
    direct_execution::{DirectExecutionErrorV1, DirectReadRequestV1},
    execution::ExecutionClockV1,
    workspace::{DirectWorkspaceResolutionErrorV1, DirectWorkspaceResolverV1},
};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{ContractGenerationV1, OutputEnvelopeV4};
use podway_store::{WorkspaceBindingV1, successor_store::NativeSuccessorStore};
use std::{cell::RefCell, time::Instant};

/// An admitted activation proposal. A default identifies the immutable base digest
/// carried in the caller's exact cursor, including during original-key replay.
#[derive(Clone, Debug)]
pub enum DirectGroupSourceV1 {
    CatalogDefault,
    Document {
        source: String,
        format: ProcedureDocumentFormat,
    },
}

#[derive(Clone, Debug)]
pub struct DirectGroupActivationV1 {
    pub group: DirectGroupSourceV1,
    pub expected_definition: Sha256Digest,
    pub plan: PlanBindingV1,
    pub predecessor: Option<GroupExecutionIdV1>,
    pub working_copy_basis: CheckResultInputBasisV2,
    pub prior_work: Vec<PriorWorkDispositionV1>,
}

/// The Direct caller supplies typed arguments only. Filesystem and transaction
/// guards remain wholly inside the Engine.
#[derive(Clone, Debug)]
pub enum DirectSuccessorMutationV1 {
    Cursor {
        cursor: SuccessorCursor,
        operation: SuccessorCursorMutation,
        actor: Option<podway_core::ActorAttributionV2>,
    },
    Activate {
        cursor: SuccessorCursor,
        input: DirectGroupActivationV1,
    },
    Return(SuccessorCursor),
    Replan {
        cursor: SuccessorCursor,
        feedback: ReplanFeedbackV1,
    },
}

impl DirectSuccessorMutationV1 {
    pub fn command_name(&self) -> &'static str {
        match self {
            Self::Cursor { operation, .. } => operation.command_name(),
            Self::Activate { .. } => "group.activate",
            Self::Return(_) => "group.return",
            Self::Replan { .. } => "group.replan",
        }
    }

    fn cursor(&self) -> &SuccessorCursor {
        match self {
            Self::Cursor { cursor, .. }
            | Self::Activate { cursor, .. }
            | Self::Return(cursor)
            | Self::Replan { cursor, .. } => cursor,
        }
    }

    fn needs_progress_verification(&self) -> bool {
        match self {
            Self::Cursor { operation, .. } => operation.needs_progress_verification(),
            Self::Activate { .. } | Self::Return(_) | Self::Replan { .. } => true,
        }
    }
}

pub(crate) struct DirectSuccessorMutationRequestV1 {
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: WorkspaceId,
    pub key: IdempotencyKeyV1,
    pub request_id: RequestIdV1,
    pub operation: DirectSuccessorMutationV1,
    pub deadline: Instant,
    pub response: SuccessorMutationResponseV1,
}

pub(crate) fn dispatch_successor_observation<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectReadRequestV1,
    clock: &impl ExecutionClockV1,
) -> std::result::Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    resolver.resolve_existing_and_read(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
        |resolved, snapshot| {
            resolver
                .revalidate(resolved, snapshot.binding())
                .map_err(DirectWorkspaceResolutionErrorV1::from)?;
            let current = snapshot
                .successor_state(admit_candidate_definition)
                .map_err(SuccessorExecutionError::from)?;
            let current_session = current.as_ref().map(|(state, _)| state.fence().session);
            if let Some(expected) = request.expected_session_id
                && current_session.as_ref() != Some(&expected)
            {
                return Err(DirectExecutionErrorV1::Domain(
                    DomainError::SessionIdentityMismatch {
                        expected,
                        actual: current_session,
                    },
                ));
            }
            let (state, sources) =
                current.ok_or(crate::v2_read_service::GraphViewErrorV2::MissingGraphSession)?;
            let fence = state.fence();
            let workspace = snapshot.binding().identity().workspace_uuid();
            let title = snapshot
                .successor_metadata(&fence.session)
                .map_err(SuccessorExecutionError::from)?
                .ok_or(DirectStoreErrorV1::Unreadable)?
                .task_title;
            let sequence = snapshot.committed_sequence()?;
            use crate::direct_execution::DirectReadCommandV1;
            use crate::v2_read_service::GraphStatusTierV2;
            let result = match request.command {
                DirectReadCommandV1::Status {
                    tier: GraphStatusTierV2::Compact,
                    ..
                } => presentation::compact_status(workspace, &state, sequence),
                _ => {
                    let terminal_disposition = snapshot
                        .successor_disposition(&fence.session, fence.revision)?
                        .is_some();
                    let observation = presentation::public_observation(
                        workspace,
                        &title,
                        &state,
                        &sources,
                        sequence,
                        if matches!(request.command, DirectReadCommandV1::Observe) {
                            request.generation
                        } else {
                            ContractGenerationV1::Generation1
                        },
                        presentation::ObservationActivityV1::Current {
                            terminal_disposition,
                        },
                    )?;
                    match request.command {
                        DirectReadCommandV1::Status {
                            tier: GraphStatusTierV2::Verbose,
                            history_before,
                        } => presentation::verbose_status(observation, &state, history_before)?,
                        _ => observation,
                    }
                }
            };
            let Value::Object(result) = result else {
                unreachable!("successor observation is an object");
            };
            Ok(OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
                request_id: request.request_id,
                command: CommandNameV1::new(request.command.command_name())?,
                generated_at: Rfc3339MillisV1::from_unix_millis(clock.now().get())?,
                workspace: Some(WorkspaceOutputV1::new(
                    workspace.clone(),
                    resolved
                        .workspace_root()
                        .to_path_buf()
                        .display()
                        .to_string(),
                    sequence,
                )?),
                session: Some(DirectSessionOutputV1 {
                    id: fence.session,
                    title,
                    lifecycle: state.lifecycle(),
                    revision_before: fence.revision,
                    revision_after: fence.revision,
                }),
                result,
                warnings: Vec::new(),
            })?)
        },
    )
}

pub(crate) fn preflight_successor_facts<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: crate::direct_execution::DirectPreflightFactsRequestV1,
) -> std::result::Result<crate::direct_execution::DirectPreflightFactsV1, DirectExecutionErrorV1> {
    resolver.resolve_existing_and_read(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
        |resolved, snapshot| {
            resolver
                .revalidate(resolved, snapshot.binding())
                .map_err(DirectWorkspaceResolutionErrorV1::from)?;
            let current = snapshot
                .successor_state(admit_candidate_definition)
                .map_err(SuccessorExecutionError::from)?;
            let actual = current.as_ref().map(|(state, _)| state.fence().session);
            if let Some(expected) = request.expected_session_id
                && actual.as_ref() != Some(&expected)
            {
                return Err(DirectExecutionErrorV1::Domain(
                    DomainError::SessionIdentityMismatch { expected, actual },
                ));
            }
            let (state, _) =
                current.ok_or(crate::v2_read_service::GraphViewErrorV2::MissingGraphSession)?;
            let cursor = state.fence();
            let item_revisions = state
                .active_attempt()
                .map(|active| {
                    active
                        .slots()
                        .iter()
                        .map(|slot| (slot.item_id().as_str().to_owned(), slot.revision()))
                        .collect()
                })
                .unwrap_or_default();

            Ok(crate::direct_execution::DirectPreflightFactsV1 {
                workspace_id: snapshot.binding().identity().workspace_uuid().clone(),
                successor_cursor: Some(Box::new(SuccessorCursor::from_state(&state))),
                session_id: cursor.session,
                session_revision: cursor.revision,
                attempt_id: cursor.attempt.map(|a| a.attempt().clone()),
                item_revisions,
                goal_revision: cursor.goal_revision,
                lifecycle: state.lifecycle(),
            })
        },
    )
}

pub(crate) fn dispatch_successor_mutation<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectSuccessorMutationRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> std::result::Result<CanonicalExecutionJsonV1, DirectExecutionErrorV1> {
    let workspace = request.expected_workspace_id;
    #[cfg(feature = "test-support")]
    if matches!(
        &request.operation,
        DirectSuccessorMutationV1::Cursor {
            operation: SuccessorCursorMutation::Decide { .. },
            ..
        }
    ) {
        return Err(DirectExecutionErrorV1::InvalidRequest);
    }
    let (resolved, store, binding) = resolver.resolve_existing_with_binding(
        request.selector,
        Some(&workspace),
        request.deadline,
    )?;
    let native = resolver.native_workspace(resolved.clone());
    let admit = |binding: &WorkspaceBindingV1| {
        resolver
            .revalidate(&resolved, binding)
            .map_err(|_| CandidateStoreError::from(DirectStoreErrorV1::BindingMismatch))
    };
    let mut adapter = NativeSuccessorStore::new(&store, request.deadline, admit);
    let cursor = request.operation.cursor();
    let prepared = RefCell::new(request.operation.needs_progress_verification().then(|| {
        adapter
            .snapshot_for_transition(
                &workspace,
                &cursor.execution.session,
                admit_candidate_definition,
            )
            .and_then(|(state, _)| prepare_successor_progress(&native, &binding, &state))
    }));
    let verify = |binding: &WorkspaceBindingV1, state: &ExecutionSessionV3| {
        let progress = prepared
            .borrow_mut()
            .take()
            .ok_or(CandidateStoreError::Corrupt(
                "successor progress verification lacks request-local preparation",
            ))??;
        verify_successor_progress(&native, binding, state, &progress)
    };
    let mut adapter = adapter.with_progress_verifier(&verify);
    let mutation = SuccessorMutationRequest {
        workspace,
        key: request.key,
        request_id: request.request_id,
        cursor: cursor.clone(),
        response: request.response,
    };
    let result = match request.operation {
        DirectSuccessorMutationV1::Cursor {
            operation, actor, ..
        } => mutations::issue_cursor_mutation_attributed(
            &mut adapter,
            mutation,
            operation,
            actor,
            || clock.now(),
            ids,
        ),
        DirectSuccessorMutationV1::Activate { input, .. } => {
            let (document, format) = resolve_group_source(&mutation.cursor, input.group)?;
            activate_group(
                &mut adapter,
                GroupActivationRequest {
                    mutation,
                    document,
                    format,
                    expected_definition: input.expected_definition,
                    plan: input.plan,
                    predecessor: input.predecessor,
                    working_copy_basis: input.working_copy_basis,
                    prior_work: input.prior_work,
                },
                || clock.now(),
                ids,
            )
        }
        DirectSuccessorMutationV1::Return(_) => {
            return_from_group(&mut adapter, mutation, || clock.now(), ids)
        }
        DirectSuccessorMutationV1::Replan { feedback, .. } => {
            replan_group(&mut adapter, mutation, feedback, || clock.now(), ids)
        }
    }?;
    Ok(result.outcome().response().clone())
}

fn resolve_group_source(
    cursor: &SuccessorCursor,
    group: DirectGroupSourceV1,
) -> Result<(String, ProcedureDocumentFormat)> {
    match group {
        DirectGroupSourceV1::Document { source, format } => Ok((source, format)),
        DirectGroupSourceV1::CatalogDefault => {
            let base = cursor
                .execution
                .attempt
                .as_ref()
                .ok_or(SuccessorExecutionError::Invalid(
                    "activation must bind an exact current design attempt",
                ))?
                .base_digest();
            let preset = podway_presets::list_v3()
                .iter()
                .find(|preset| preset.base_digest == base.as_str())
                .ok_or(SuccessorExecutionError::Invalid(
                    "the base definition has no catalog default execution group",
                ))?;
            Ok((preset.group_yaml.to_owned(), ProcedureDocumentFormat::Yaml))
        }
    }
}

impl From<SuccessorExecutionError> for DirectExecutionErrorV1 {
    fn from(error: SuccessorExecutionError) -> Self {
        match error {
            SuccessorExecutionError::Store(CandidateStoreError::Direct(error)) => error.into(),
            SuccessorExecutionError::Store(CandidateStoreError::Item(error)) => {
                DirectStoreErrorV1::State(error).into()
            }
            SuccessorExecutionError::Store(_) => DirectStoreErrorV1::Unreadable.into(),
            SuccessorExecutionError::Domain(error) => Self::Domain(error),
            SuccessorExecutionError::ItemMutation(error) => error.into(),
            SuccessorExecutionError::Protocol(error) => Self::Protocol(error),
            SuccessorExecutionError::Delegation(error) => Self::Delegation(error),
            SuccessorExecutionError::Invalid(reason) => {
                DirectStoreErrorV1::InvalidInput { reason }.into()
            }
            error => Self::Successor(error),
        }
    }
}

impl From<CandidateStoreError> for DirectExecutionErrorV1 {
    fn from(error: CandidateStoreError) -> Self {
        SuccessorExecutionError::from(error).into()
    }
}
