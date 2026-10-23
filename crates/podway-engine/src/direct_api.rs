//! Typed in-process request boundary shared by CLI and programmatic callers.

use sha2::{Digest as _, Sha256};
use std::{sync::Arc, time::Instant};

use podway_core::{SessionId, WorkspaceId};
use podway_git::GitResolverContractV1;
pub use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{
    CommandNameV1, ContractGenerationV1, DelegationAcceptV1, ErrorEnvelopeV2, ItemRecordManyV1,
    OutputEnvelopeV4, ProcedureV2MutationCommandV1, ProtocolError, RequestIdV1, Rfc3339MillisV1,
};
pub use podway_store::IdempotencyKeyV1;
use podway_store::direct_store::DirectStoreErrorV1;

use crate::{
    direct_delegation::{DirectDelegationRequestV1, accept_delegation_v1},
    direct_error::{DirectErrorContextV1, DirectMaintenanceRecoveryV1, render_direct_error_v1},
    direct_execution::{
        DirectCursorCommandV1, DirectCursorRequestV1, DirectExecutionErrorV1, DirectIdSourceV1,
        DirectPreflightFactsRequestV1, DirectPreflightFactsResponseV1, DirectReadCommandV1,
        DirectReadRequestV1, OutcomeLookupRequestV1, execute_cursor_with_context_v1,
        execute_read_v1, lookup_outcome_v1, preflight_facts_with_view_v1,
    },
    direct_items::{
        DirectItemCommandV1, DirectItemRequestV1, DirectRecordManyRequestV1,
        execute_item_with_context_v1, record_many_v1,
    },
    direct_maintenance::{DirectMaintenanceRequestV1, remove_workspace_v1, reset_all_v1},
    direct_procedure::{DirectProcedureRequestV1, execute_procedure_with_context_v1},
    direct_sessions::{
        DirectSessionCommandV1, DirectSessionRequestV1, execute_session_with_context_v1,
    },
    direct_start::{DirectStartCommandV1, DirectStartRequestV1, start_session_with_context_v1},
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
}

/// One invocation's optional immutable graph hint. No Store types, stamps or
/// live guards are exposed to the CLI. A fresh preflight replaces prior state.
#[derive(Default)]
pub struct DirectInvocationContextV1 {
    prepared: Option<podway_store::direct_store::DirectPreparedGraphV1>,
}

pub(crate) fn prepare_graph_v1(
    store: &podway_store::direct_store::DirectStoreV1,
    deadline: Instant,
    context: Option<&mut DirectInvocationContextV1>,
) -> Result<Option<podway_store::direct_store::DirectPreparedGraphV1>, DirectStoreErrorV1> {
    let Some(context) = context else {
        return store.prepare_graph(deadline);
    };
    #[cfg(debug_assertions)]
    let prior = context.prepared.clone();
    let result = store.prepare_graph_with_hint(deadline, &mut context.prepared);
    #[cfg(debug_assertions)]
    if let Some(prior) = prior
        && context.prepared.is_none()
    {
        reads004_preflight_counters::record(&reads004_preflight_counters::CONSUMED);
        if result
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .is_some_and(|current| std::ptr::eq(prior.graph(), current.graph()))
        {
            reads004_preflight_counters::record(&reads004_preflight_counters::REUSED);
        }
    }
    result
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
    /// Presentation-only contract generation (ADR-0039); never part of
    /// request identity, idempotency or stored outcomes.
    pub generation: ContractGenerationV1,
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
    execute_direct_inner_v1(resolver, request, clock, ids, None, None, None)
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
    execute_direct_inner_v1(resolver, request, clock, ids, Some(&observer), None, None)
}

type MaintenanceObserver =
    Arc<dyn Fn(crate::direct_maintenance::DirectMaintenanceBoundaryV1) + Send + Sync>;

/// READS-004 debug-only, process-local preflight instrumentation: counters for
/// typed facts facade entries, Standard status dispatches, and graph hints
/// retained, consumed and reused.
/// Release builds compile the counters away entirely; the CLI prints them
/// only under its private test token.
#[cfg(debug_assertions)]
mod reads004_preflight_counters {
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(super) static RETAINED: AtomicUsize = AtomicUsize::new(0);
    pub(super) static CONSUMED: AtomicUsize = AtomicUsize::new(0);
    pub(super) static REUSED: AtomicUsize = AtomicUsize::new(0);
    pub(super) static FACTS_ENTRIES: AtomicUsize = AtomicUsize::new(0);
    pub(super) static STATUS_DISPATCHES: AtomicUsize = AtomicUsize::new(0);

    pub(super) fn record(counter: &AtomicUsize) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// Reads the debug-only process-local preflight counters (facts entries,
/// Standard status dispatches). Absent from release builds.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn reads004_debug_preflight_counters_v1() -> (usize, usize) {
    use std::sync::atomic::Ordering;
    (
        reads004_preflight_counters::FACTS_ENTRIES.load(Ordering::Relaxed),
        reads004_preflight_counters::STATUS_DISPATCHES.load(Ordering::Relaxed),
    )
}

/// Supporting invocation wiring evidence, absent from release builds.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn reads004_debug_graph_hint_counters_v1() -> (usize, usize, usize) {
    use std::sync::atomic::Ordering;
    (
        reads004_preflight_counters::RETAINED.load(Ordering::Relaxed),
        reads004_preflight_counters::CONSUMED.load(Ordering::Relaxed),
        reads004_preflight_counters::REUSED.load(Ordering::Relaxed),
    )
}

/// Execute a distinct Engine request with one invocation-local optional hint.
pub fn execute_direct_with_context_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    context: &mut DirectInvocationContextV1,
) -> Result<DirectResponseV1, ProtocolError> {
    execute_direct_in_context_v1(resolver, request, clock, ids, context, None)
}

#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn execute_direct_with_context_and_maintenance_observer_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    context: &mut DirectInvocationContextV1,
    observer: impl Fn(crate::direct_maintenance::DirectMaintenanceBoundaryV1) + Send + Sync + 'static,
) -> Result<DirectResponseV1, ProtocolError> {
    execute_direct_in_context_v1(
        resolver,
        request,
        clock,
        ids,
        context,
        Some(Arc::new(observer)),
    )
}

fn execute_direct_in_context_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    context: &mut DirectInvocationContextV1,
    maintenance_observer: Option<MaintenanceObserver>,
) -> Result<DirectResponseV1, ProtocolError> {
    // Classify without validating inputs or moving any command work earlier.
    let consumes = match &request.command {
        DirectCommandV1::Cursor(_) | DirectCommandV1::Item(_) | DirectCommandV1::Procedure(_) => {
            true
        }
        DirectCommandV1::Start(command) => !command.input().dry_run,
        DirectCommandV1::Session(DirectSessionCommandV1::Reset(input)) => !input.dry_run,
        DirectCommandV1::Session(DirectSessionCommandV1::Archive(_)) => true,
        _ => false,
    };
    let result = execute_direct_inner_v1(
        resolver,
        request,
        clock,
        ids,
        None,
        maintenance_observer,
        Some(context),
    );
    if consumes {
        context.prepared = None;
    }
    result
}

fn execute_direct_inner_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    observer: Option<&dyn Fn(&DirectExecutionErrorV1)>,
    maintenance_observer: Option<MaintenanceObserver>,
    invocation: Option<&mut DirectInvocationContextV1>,
) -> Result<DirectResponseV1, ProtocolError> {
    let mut context = DirectErrorContextV1 {
        request_id: request.request_id.clone(),
        command: CommandNameV1::new(request.command.command_name())?,
        observed_at: request.observed_at.clone(),
        idempotency_key: request.idempotency_key.clone(),
        timeout_ms: request.timeout_ms,
        generation: request.generation,
        maintenance: None,
    };
    let selector = request.selector.clone();
    let result = dispatch(
        resolver,
        request,
        clock,
        ids,
        maintenance_observer,
        invocation,
    );
    match result {
        Ok(output) => Ok(DirectResponseV1::Success(output)),
        Err(error) => Ok(DirectResponseV1::Failure(finish_direct_error_v1(
            error,
            &mut context,
            resolver,
            &selector,
            observer,
        )?)),
    }
}

/// The one shared error finisher: observer notification, maintenance recovery
/// observation with its replacement semantics, and public error rendering.
/// Every direct entry point must fail through this single path.
fn finish_direct_error_v1<R: GitResolverContractV1>(
    mut error: DirectExecutionErrorV1,
    context: &mut DirectErrorContextV1,
    resolver: &DirectWorkspaceResolverV1<R>,
    selector: &WorktreeSelectorV1,
    observer: Option<&dyn Fn(&DirectExecutionErrorV1)>,
) -> Result<ErrorEnvelopeV2, ProtocolError> {
    if let Some(observer) = observer {
        observer(&error);
    }
    if is_maintenance(&error) {
        match observe_maintenance(resolver, selector.clone()) {
            Ok(recovery) => context.maintenance = recovery,
            Err(observation_error) => {
                if let Some(observer) = observer {
                    observer(&observation_error);
                }
                error = observation_error;
            }
        }
    }
    render_direct_error_v1(&error, context)
}

/// READS-004 typed fence-preflight facts entry: renders failures through the
/// same shared error finisher as every direct response, with the original
/// `session.status` command attribution the CLI status preflight consumed.
/// Returns owned typed facts on success; never constructs a public output
/// envelope, and never consumes an idempotency key.
#[doc(hidden)]
pub fn execute_direct_preflight_facts_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectPreflightFactsRequestV1,
    clock: &impl ExecutionClockV1,
) -> Result<DirectPreflightFactsResponseV1, ProtocolError> {
    execute_direct_preflight_facts_inner_v1(resolver, request, clock, None)
}

pub fn execute_direct_preflight_facts_with_context_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectPreflightFactsRequestV1,
    clock: &impl ExecutionClockV1,
    context: &mut DirectInvocationContextV1,
) -> Result<DirectPreflightFactsResponseV1, ProtocolError> {
    execute_direct_preflight_facts_inner_v1(resolver, request, clock, Some(context))
}

fn execute_direct_preflight_facts_inner_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectPreflightFactsRequestV1,
    clock: &impl ExecutionClockV1,
    mut invocation: Option<&mut DirectInvocationContextV1>,
) -> Result<DirectPreflightFactsResponseV1, ProtocolError> {
    if let Some(invocation) = invocation.as_mut() {
        invocation.prepared = None;
    }

    #[cfg(debug_assertions)]
    reads004_preflight_counters::record(&reads004_preflight_counters::FACTS_ENTRIES);
    let mut context = DirectErrorContextV1 {
        request_id: request.request_id.clone(),
        command: CommandNameV1::new(
            DirectReadCommandV1::Status {
                tier: crate::v2_read_service::GraphStatusTierV2::Standard,
                history_before: None,
            }
            .command_name(),
        )?,
        observed_at: request.observed_at.clone(),
        idempotency_key: None,
        timeout_ms: request.timeout_ms,
        generation: request.generation,
        maintenance: None,
    };
    let selector = request.selector.clone();
    match preflight_facts_with_view_v1(resolver, request, clock, invocation.is_some()) {
        Ok((facts, view)) => {
            // The composed read has returned: its final witness and guard drops
            // precede conversion and publication into the invocation context.
            if let Some(invocation) = invocation {
                invocation.prepared = view.and_then(|view| view.into_prepared_graph());
                #[cfg(debug_assertions)]
                if invocation.prepared.is_some() {
                    reads004_preflight_counters::record(&reads004_preflight_counters::RETAINED);
                }
            }
            Ok(DirectPreflightFactsResponseV1::Facts(facts))
        }
        Err(error) => Ok(DirectPreflightFactsResponseV1::Failure(
            finish_direct_error_v1(error, &mut context, resolver, &selector, None)?,
        )),
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
    use crate::workspace::{DirectWorkspaceResolutionErrorV1, WorkspaceResolutionErrorV1};
    use podway_git::{GitInvariantViolationV1, GitResolverErrorV1};

    let mut observed_boundary = "maintenance_lock";
    match observe_maintenance_snapshot(resolver, selector, &mut observed_boundary) {
        Err(DirectExecutionErrorV1::Workspace(DirectWorkspaceResolutionErrorV1::Resolution(
            WorkspaceResolutionErrorV1::Git {
                source:
                    GitResolverErrorV1::Invariant {
                        problem: GitInvariantViolationV1::MetadataChangedDuringResolution,
                    },
                ..
            },
        ))) => Ok(Some(DirectMaintenanceRecoveryV1::WriterIdentity {
            boundary: observed_boundary,
            reason: "Maintenance changed while inspecting recovery. Inspect writer identity, wait for the maintenance owner to finish, then retry the original request with its key and fences.",
        })),
        observed => observed,
    }
}

fn observe_maintenance_snapshot<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    selector: WorktreeSelectorV1,
    observed_boundary: &mut &'static str,
) -> Result<Option<DirectMaintenanceRecoveryV1>, DirectExecutionErrorV1> {
    let (worktree, store) = resolver.resolve_candidate(selector)?;
    let Some(marker) =
        podway_store::direct_store::observe_direct_maintenance_marker_v1(store.layout())?
    else {
        if podway_store::direct_store::classify_direct_store_v1(store.layout())?
            == podway_store::direct_store::DirectStoreClassificationV1::Missing
        {
            resolver.revalidate_candidate(&worktree)?;
            return Ok(Some(DirectMaintenanceRecoveryV1::WriterIdentity {
                boundary: "maintenance_lock",
                reason: "Inspect writer identity without opening a Store. Wait for the maintenance owner to finish, then retry the original request with its key and fences.",
            }));
        }
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(marker.document().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    let boundary = match marker.kind() {
        podway_store::direct_store::DirectMaintenanceMarkerKindV1::Intent => "intent_marker",
        podway_store::direct_store::DirectMaintenanceMarkerKindV1::Reset => "reset_marker",
        podway_store::direct_store::DirectMaintenanceMarkerKindV1::Removal => "removal_marker",
    };
    *observed_boundary = boundary;
    let schema = value["schema"]
        .as_str()
        .ok_or(DirectStoreErrorV1::Unreadable)?;
    if !podway_protocol::validate_daemon_free_document_v1(schema, &value) {
        return Err(DirectStoreErrorV1::Unreadable.into());
    }
    resolver.revalidate_candidate(&worktree)?;
    if value["contract_manifest_digest"] != resolver.manifest().as_str()
        || value["root_identity"]
            != serde_json::json!({
            "root_path_bytes_base64url":worktree.roots().worktree_root().path_bytes_base64url().as_str(),
            "common_dir_identity":worktree.identity().common_directory_fingerprint(),
            "worktree_admin_identity":worktree.identity().worktree_administration_fingerprint()})
    {
        return Ok(Some(DirectMaintenanceRecoveryV1::WriterIdentity {
            boundary,
            reason: "Inspect the current writer identity; resume this marker with its original Podway writer and original worktree. The current writer cannot recover it.",
        }));
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
            Some(DirectMaintenanceRecoveryV1::Reset {
                boundary,
                key,
                workspace,
            })
        }
        Some("workspace.remove") => {
            let (workspace, force) = parameters()?;
            Some(DirectMaintenanceRecoveryV1::Removal {
                boundary,
                key,
                workspace,
                force,
            })
        }
        Some("workspace.init") => {
            let workspace = WorkspaceId::new(
                value["workspace_uuid"]
                    .as_str()
                    .ok_or(DirectStoreErrorV1::Unreadable)?,
            )
            .map_err(|_| DirectStoreErrorV1::Unreadable)?;
            let mut matched = None;
            for repair in [false, true] {
                let command = podway_protocol::SliceCommandV1::WorkspaceInit(
                    podway_protocol::WorkspaceInitV1 { repair },
                );
                let canonical = podway_protocol::canonical_direct_slice_mutation_identity_v1(
                    &command, &workspace,
                )
                .map_err(|_| DirectStoreErrorV1::Unreadable)?;
                if value["request_digest"]
                    == format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
                {
                    matched = Some(repair);
                    break;
                }
            }
            let repair = matched.ok_or(DirectStoreErrorV1::Unreadable)?;
            Some(DirectMaintenanceRecoveryV1::Init { key, repair })
        }
        Some("workspace.migrate") => Some(DirectMaintenanceRecoveryV1::WriterIdentity {
            boundary,
            reason: "Inspect writer identity; this writer declares no predecessor. Resume the migration intent with its original Podway writer and original worktree.",
        }),
        _ => return Err(DirectStoreErrorV1::Unreadable.into()),
    })
}

fn dispatch<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    maintenance_observer: Option<MaintenanceObserver>,
    invocation: Option<&mut DirectInvocationContextV1>,
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
        } => {
            #[cfg(debug_assertions)]
            if matches!(command, DirectReadCommandV1::Status { .. }) {
                reads004_preflight_counters::record(&reads004_preflight_counters::STATUS_DISPATCHES);
            }
            execute_read_v1(
                resolver,
                DirectReadRequestV1 {
                    request_id: request.request_id,
                    selector: request.selector,
                    expected_workspace_id: request.expected_workspace_id,
                    expected_session_id,
                    command,
                    deadline: request.deadline,
                    generation: request.generation,
                },
                clock,
            )
        }
        DirectCommandV1::Start(command) => start_session_with_context_v1(
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
            invocation,
        ),
        DirectCommandV1::Cursor(command) => execute_cursor_with_context_v1(
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
            invocation,
        ),
        DirectCommandV1::Item(command) => execute_item_with_context_v1(
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
            invocation,
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
        DirectCommandV1::Procedure(command) => execute_procedure_with_context_v1(
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
            invocation,
        ),
        DirectCommandV1::Session(command) => execute_session_with_context_v1(
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
            invocation,
        ),
        DirectCommandV1::WorkspaceInit { repair } => {
            match maintenance_observer {
                Some(observer) => crate::direct_workspace::initialize_workspace_with_boundary_observer_v1(
                    resolver, mutation()?, repair, clock, ids,
                    move |boundary| observer(match boundary {
                        podway_store::direct_store::DirectTransactionBoundaryV1::BeforeMaintenancePublication => crate::direct_maintenance::DirectMaintenanceBoundaryV1::BeforeStorePublication,
                        boundary => crate::direct_maintenance::DirectMaintenanceBoundaryV1::StoreBoundary(boundary),
                    }),
                ).map_err(Into::into),
                None => initialize_workspace_v1(resolver, mutation()?, repair, clock, ids).map_err(Into::into),
            }
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
