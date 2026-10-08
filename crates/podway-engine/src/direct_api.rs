//! Typed in-process request boundary shared by CLI and programmatic callers.

use std::{sync::Arc, time::Instant};

use podway_core::{Revision, SessionId, WorkspaceId};
use podway_git::GitResolverContractV1;
pub use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{
    CommandNameV1, DelegationAcceptV1, ErrorEnvelopeV2, ItemRecordManyV1, OutputEnvelopeV4,
    ProcedureV2MutationCommandV1, ProtocolError, RequestIdV1, Rfc3339MillisV1,
};
pub use podway_store::IdempotencyKeyV1;
use podway_store::direct_store::DirectStoreErrorV1;

use crate::{
    direct_delegation::{DirectDelegationRequestV1, accept_delegation_v1},
    direct_error::{DirectErrorContextV1, DirectMaintenanceRecoveryV1, render_direct_error_v1},
    direct_execution::{
        DirectCursorCommandV1, DirectCursorRequestV1, DirectExecutionErrorV1, DirectIdSourceV1,
        DirectReadCommandV1, DirectReadRequestV1, OutcomeLookupRequestV1, execute_cursor_v1,
        execute_read_v1, lookup_outcome_v1,
    },
    direct_items::{
        DirectItemCommandV1, DirectItemRequestV1, DirectRecordManyRequestV1, execute_item_v1,
        record_many_v1,
    },
    direct_maintenance::{DirectMaintenanceRequestV1, remove_workspace_v1, reset_all_v1},
    direct_procedure::{DirectProcedureRequestV1, execute_procedure_v1},
    direct_sessions::{DirectSessionCommandV1, DirectSessionRequestV1, execute_session_v1},
    direct_start::{DirectStartCommandV1, DirectStartRequestV1, start_session_v1},
    direct_workspace::{
        DirectWorkspaceMutationRequestV1, DirectWorkspaceReadRequestV1, doctor_workspace_v1,
        initialize_workspace_v1, repair_workspace_v1, show_workspace_v1,
    },
    execution::ExecutionClockV1,
    workspace::DirectWorkspaceResolverV1,
};

#[derive(Clone, Debug)]
pub enum DirectCommandV1 {
    Read {
        command: DirectReadCommandV1,
        expected_session_id: Option<SessionId>,
    },
    Start(DirectStartCommandV1),
    Cursor(DirectCursorCommandV1),
    Item(DirectItemCommandV1),
    RecordMany(ItemRecordManyV1),
    Procedure(ProcedureV2MutationCommandV1),
    Session(DirectSessionCommandV1),
    WorkspaceInit {
        repair: bool,
    },
    WorkspaceShow,
    WorkspaceDoctor {
        deep: bool,
    },
    WorkspaceRepair,
    DelegationAccept(DelegationAcceptV1),
    OutcomeLookup {
        key: IdempotencyKeyV1,
    },
    WorkspaceResetAll {
        confirmed: bool,
    },
    WorkspaceRemove {
        confirmed: bool,
        force: bool,
    },
}

impl DirectCommandV1 {
    pub fn command_name(&self) -> &'static str {
        match self {
            Self::Read { command, .. } => command.command_name(),
            Self::Start(command) => command.slice().command_name(),
            Self::Cursor(command) => command.slice_command().command_name(),
            Self::Item(command) => command.slice_command().command_name(),
            Self::RecordMany(_) => "item.record_many",
            Self::Procedure(command) => command.command_name(),
            Self::Session(command) => command.slice().command_name(),
            Self::WorkspaceInit { .. } => "workspace.init",
            Self::WorkspaceShow => "workspace.show",
            Self::WorkspaceDoctor { .. } => "workspace.doctor",
            Self::WorkspaceRepair => "workspace.repair",
            Self::DelegationAccept(_) => "delegation.accept",
            Self::OutcomeLookup { .. } => "outcome.lookup",
            Self::WorkspaceResetAll { .. } => "workspace.reset_all",
            Self::WorkspaceRemove { .. } => "workspace.remove",
        }
    }

    fn expected_revision(&self) -> Option<Revision> {
        match self {
            Self::Cursor(command) => Some(command.preconditions().expected_session_revision),
            Self::Item(command) => Some(command.preconditions().expected_item_revision),
            Self::RecordMany(input) => Some(input.preconditions.expected_session_revision),
            Self::DelegationAccept(input) => Some(input.preconditions.expected_session_revision),
            Self::Start(DirectStartCommandV1::Replace(input)) => {
                Some(input.preconditions.expected_session_revision)
            }
            Self::Session(command) => Some(match command {
                DirectSessionCommandV1::Reset(input) => {
                    input.preconditions.expected_session_revision
                }
                DirectSessionCommandV1::Archive(input) => {
                    input.preconditions.expected_session_revision
                }
                DirectSessionCommandV1::Purge(input) => input.expected_session_revision,
            }),
            Self::Procedure(command) => Some(match command {
                ProcedureV2MutationCommandV1::SessionBegin(input) => {
                    input.preconditions.expected_session_revision
                }
                ProcedureV2MutationCommandV1::SessionTerminalDisposition(input) => {
                    input.preconditions.expected_session_revision
                }
                ProcedureV2MutationCommandV1::SessionDecide(input) => {
                    input.preconditions.expected_session_revision
                }
                ProcedureV2MutationCommandV1::SessionRework(input) => {
                    input.preconditions.expected_session_revision
                }
                ProcedureV2MutationCommandV1::GoalDefine(input) => {
                    input.preconditions.expected_session_revision
                }
                ProcedureV2MutationCommandV1::GoalRevise(input) => {
                    input.preconditions.expected_session_revision
                }
                ProcedureV2MutationCommandV1::GoalAssessCriterion(input) => {
                    input.preconditions.expected_session_revision
                }
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirectRequestV1 {
    pub request_id: RequestIdV1,
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub command: DirectCommandV1,
    pub idempotency_key: Option<IdempotencyKeyV1>,
    pub deadline: Instant,
    /// Actual host observation time, independent of the mutation clock.
    pub observed_at: Rfc3339MillisV1,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug)]
pub enum DirectResponseV1 {
    Success(OutputEnvelopeV4),
    Failure(ErrorEnvelopeV2),
}

/// Mutations replay before consulting their clock or identifier source. Error formatting
/// uses the request's host timestamp so rejected requests never stamp a transaction.
pub fn execute_direct_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> Result<DirectResponseV1, ProtocolError> {
    execute_direct_inner_v1(resolver, request, clock, ids, None, None)
}

/// Observe original typed failures and any failed recovery observation without changing dispatch.
#[doc(hidden)]
pub fn execute_direct_with_error_observer_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    observer: impl Fn(&DirectExecutionErrorV1),
) -> Result<DirectResponseV1, ProtocolError> {
    execute_direct_inner_v1(resolver, request, clock, ids, Some(&observer), None)
}

type MaintenanceObserver =
    Arc<dyn Fn(crate::direct_maintenance::DirectMaintenanceBoundaryV1) + Send + Sync>;

/// Isolated debug-binary qualification observes the same maintenance boundaries as Engine tests.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn execute_direct_with_maintenance_observer_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    observer: impl Fn(crate::direct_maintenance::DirectMaintenanceBoundaryV1) + Send + Sync + 'static,
) -> Result<DirectResponseV1, ProtocolError> {
    execute_direct_inner_v1(
        resolver,
        request,
        clock,
        ids,
        None,
        Some(Arc::new(observer)),
    )
}

fn execute_direct_inner_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    observer: Option<&dyn Fn(&DirectExecutionErrorV1)>,
    maintenance_observer: Option<MaintenanceObserver>,
) -> Result<DirectResponseV1, ProtocolError> {
    let mut context = DirectErrorContextV1 {
        request_id: request.request_id.clone(),
        command: CommandNameV1::new(request.command.command_name())?,
        observed_at: request.observed_at.clone(),
        idempotency_key: request.idempotency_key.clone(),
        timeout_ms: request.timeout_ms,
        expected_revision: request.command.expected_revision(),
        maintenance: None,
    };
    let selector = request.selector.clone();
    let result = dispatch(resolver, request, clock, ids, maintenance_observer);
    match result {
        Ok(output) => Ok(DirectResponseV1::Success(output)),
        Err(mut error) => {
            if let Some(observer) = observer {
                observer(&error);
            }
            if is_maintenance(&error) {
                match observe_maintenance(resolver, selector) {
                    Ok(recovery) => context.maintenance = recovery,
                    Err(observation_error) => {
                        if let Some(observer) = observer {
                            observer(&observation_error);
                        }
                        error = observation_error;
                    }
                }
            }
            Ok(DirectResponseV1::Failure(render_direct_error_v1(
                &error, &context,
            )?))
        }
    }
}

fn is_maintenance(error: &DirectExecutionErrorV1) -> bool {
    use crate::{
        direct_maintenance::DirectMaintenanceErrorV1 as M,
        direct_workspace::DirectWorkspaceErrorV1 as W,
        workspace::DirectWorkspaceResolutionErrorV1 as R,
    };
    matches!(
        error,
        DirectExecutionErrorV1::Store(DirectStoreErrorV1::Maintenance)
            | DirectExecutionErrorV1::Workspace(R::Store(DirectStoreErrorV1::Maintenance))
            | DirectExecutionErrorV1::Maintenance(M::Workspace(
                W::Store(DirectStoreErrorV1::Maintenance)
                    | W::Resolution(R::Store(DirectStoreErrorV1::Maintenance))
            ))
    )
}

fn observe_maintenance<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    selector: WorktreeSelectorV1,
) -> Result<Option<DirectMaintenanceRecoveryV1>, DirectExecutionErrorV1> {
    let (worktree, store) = resolver.resolve_candidate(selector)?;
    let Some(marker) =
        podway_store::direct_store::observe_direct_maintenance_marker_v1(store.layout())?
    else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(marker.document().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    let schema = value["schema"]
        .as_str()
        .ok_or(DirectStoreErrorV1::Unreadable)?;
    if !podway_protocol::validate_daemon_free_document_v1(schema, &value)
        || value["contract_manifest_digest"] != resolver.manifest().as_str()
        || value["root_identity"]
            != serde_json::json!({
            "root_path_bytes_base64url":worktree.roots().worktree_root().path_bytes_base64url().as_str(),
            "common_dir_identity":worktree.identity().common_directory_fingerprint(),
            "worktree_admin_identity":worktree.identity().worktree_administration_fingerprint()})
    {
        return Err(DirectStoreErrorV1::Unreadable.into());
    }
    let key = IdempotencyKeyV1::new(
        value["idempotency_key"]
            .as_str()
            .ok_or(DirectStoreErrorV1::Unreadable)?,
    )
    .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    resolver.revalidate_candidate(&worktree)?;
    let parameters = || -> Result<_, DirectExecutionErrorV1> {
        if crate::direct_maintenance::marker_request_digest_v1(&value)?.as_str()
            != value["request_digest"]
                .as_str()
                .ok_or(DirectStoreErrorV1::Unreadable)?
        {
            return Err(DirectStoreErrorV1::Unreadable.into());
        }
        let workspace = match value["request"]["workspace_uuid"].as_str() {
            Some(uuid) => Some(WorkspaceId::new(uuid).map_err(|_| DirectStoreErrorV1::Unreadable)?),
            None if value["request"]["workspace_uuid"].is_null() => None,
            _ => return Err(DirectStoreErrorV1::Unreadable.into()),
        };
        let force = value["request"]["force"]
            .as_bool()
            .ok_or(DirectStoreErrorV1::Unreadable)?;
        Ok((workspace, force))
    };
    Ok(match value["command"].as_str() {
        Some("workspace.reset_all") => {
            let (workspace, _) = parameters()?;
            Some(DirectMaintenanceRecoveryV1::Reset { key, workspace })
        }
        Some("workspace.remove") => {
            let (workspace, force) = parameters()?;
            Some(DirectMaintenanceRecoveryV1::Removal {
                key,
                workspace,
                force,
            })
        }
        // Init and predecessor intents use the canonical diagnostic recipe. They
        // are never resumed by observing them or by another command.
        _ => None,
    })
}

fn dispatch<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    maintenance_observer: Option<MaintenanceObserver>,
) -> Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    let key = || {
        request.idempotency_key.clone().ok_or_else(|| {
            DirectExecutionErrorV1::Store(DirectStoreErrorV1::InvalidInput {
                reason: "mutation requires an idempotency key",
            })
        })
    };
    let read = || DirectWorkspaceReadRequestV1 {
        request_id: request.request_id.clone(),
        selector: request.selector.clone(),
        expected_workspace_id: request.expected_workspace_id.clone(),
        deadline: request.deadline,
    };
    let mutation = || {
        Ok::<_, DirectExecutionErrorV1>(DirectWorkspaceMutationRequestV1 {
            request_id: request.request_id.clone(),
            selector: request.selector.clone(),
            expected_workspace_id: request.expected_workspace_id.clone(),
            key: key()?,
            deadline: request.deadline,
        })
    };
    match request.command {
        DirectCommandV1::Read {
            command,
            expected_session_id,
        } => execute_read_v1(
            resolver,
            DirectReadRequestV1 {
                request_id: request.request_id,
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                expected_session_id,
                command,
                deadline: request.deadline,
            },
            clock,
        ),
        DirectCommandV1::Start(command) => start_session_v1(
            resolver,
            DirectStartRequestV1 {
                request_id: request.request_id,
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                key: request.idempotency_key,
                command,
                deadline: request.deadline,
            },
            clock,
            ids,
        ),
        DirectCommandV1::Cursor(command) => execute_cursor_v1(
            resolver,
            DirectCursorRequestV1 {
                key: key()?,
                request_id: request.request_id,
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                command,
                deadline: request.deadline,
            },
            clock,
            ids,
        ),
        DirectCommandV1::Item(command) => execute_item_v1(
            resolver,
            DirectItemRequestV1 {
                key: key()?,
                request_id: request.request_id,
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                command,
                deadline: request.deadline,
            },
            clock,
        ),
        DirectCommandV1::RecordMany(input) => record_many_v1(
            resolver,
            DirectRecordManyRequestV1 {
                key: key()?,
                request_id: request.request_id,
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                input,
                deadline: request.deadline,
            },
            clock,
        ),
        DirectCommandV1::Procedure(command) => execute_procedure_v1(
            resolver,
            DirectProcedureRequestV1 {
                key: key()?,
                request_id: request.request_id,
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                command,
                deadline: request.deadline,
            },
            clock,
            ids,
        ),
        DirectCommandV1::Session(command) => execute_session_v1(
            resolver,
            DirectSessionRequestV1 {
                key: request.idempotency_key,
                request_id: request.request_id,
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                command,
                deadline: request.deadline,
            },
            clock,
        ),
        DirectCommandV1::WorkspaceInit { repair } => {
            initialize_workspace_v1(resolver, mutation()?, repair, clock, ids).map_err(Into::into)
        }
        DirectCommandV1::WorkspaceShow => {
            show_workspace_v1(resolver, read(), clock).map_err(Into::into)
        }
        DirectCommandV1::WorkspaceDoctor { deep } => {
            doctor_workspace_v1(resolver, read(), deep, clock).map_err(Into::into)
        }
        DirectCommandV1::WorkspaceRepair => {
            repair_workspace_v1(resolver, mutation()?, clock).map_err(Into::into)
        }
        DirectCommandV1::DelegationAccept(input) => accept_delegation_v1(
            resolver,
            DirectDelegationRequestV1 {
                key: key()?,
                request_id: request.request_id,
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                input,
                deadline: request.deadline,
            },
            clock,
            ids,
        ),
        DirectCommandV1::OutcomeLookup { key } => lookup_outcome_v1(
            resolver,
            OutcomeLookupRequestV1 {
                key,
                request_id: request.request_id,
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                deadline: request.deadline,
            },
            clock,
        ),
        DirectCommandV1::WorkspaceResetAll { confirmed } => {
            let input = DirectMaintenanceRequestV1 {
                key: key()?,
                request_id: request.request_id,
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                confirmed,
                force: false,
                deadline: request.deadline,
            };
            match maintenance_observer {
                Some(observer) => {
                    let lease_observer = observer.clone();
                    crate::direct_maintenance::reset_all_with_lease_observers_v1(
                        resolver,
                        input,
                        clock,
                        ids,
                        move |boundary| observer(boundary),
                        move |boundary, _, _| lease_observer(boundary),
                    )
                }
                None => reset_all_v1(resolver, input, clock, ids),
            }
            .map_err(DirectExecutionErrorV1::Maintenance)
        }
        DirectCommandV1::WorkspaceRemove { confirmed, force } => {
            let input = DirectMaintenanceRequestV1 {
                key: key()?,
                request_id: request.request_id,
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                confirmed,
                force,
                deadline: request.deadline,
            };
            match maintenance_observer {
                Some(observer) => {
                    let lease_observer = observer.clone();
                    crate::direct_maintenance::remove_workspace_with_lease_observers_v1(
                        resolver,
                        input,
                        clock,
                        move |boundary| observer(boundary),
                        move |boundary, _, _| lease_observer(boundary),
                    )
                }
                None => remove_workspace_v1(resolver, input, clock),
            }
            .map_err(DirectExecutionErrorV1::Maintenance)
        }
    }
}
