//! Exact-key native maintenance over one worktree namespace.
use crate::{
    direct_execution::DirectIdSourceV1,
    direct_workspace::DirectWorkspaceErrorV1,
    execution::ExecutionClockV1,
    workspace::{
        DirectWorkspaceResolverV1, ValidatedPodwayRemovalDirectoryV1,
        WorkspaceRemovalFilesystemErrorV1,
    },
};
use podway_core::{Sha256Digest, UnixMillis, WorkspaceId, canonicalize_json_v1};
use podway_git::{GitResolverContractV1, ValidatedWorktreeV1, WorktreeSelectorV1};
use podway_protocol::{
    CommandNameV1, OutputEnvelopeInputV4, OutputEnvelopeV4, RequestIdV1, Rfc3339MillisV1,
    WorkspaceOutputV1, validate_daemon_free_document_v1,
};
use podway_store::{
    CanonicalExecutionJsonV1, IdempotencyKeyV1,
    direct_store::{
        DirectMaintenanceMarkerKindV1 as Kind, DirectMaintenanceMarkerV1 as Marker,
        DirectMutationRequestV1, DirectOutcomeScopeV1,
        DirectStoreClassificationV1 as Classification, DirectStoreErrorV1,
        DirectTransactionBoundaryV1, ExclusiveMaintenanceLeaseV1, classify_direct_store_v1,
        observe_direct_maintenance_marker_v1,
    },
};
use sha2::{Digest as _, Sha256};
use std::{fmt, sync::Arc, time::Instant};

#[derive(Clone, Debug)]
pub struct DirectMaintenanceRequestV1 {
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub request_id: RequestIdV1,
    pub key: IdempotencyKeyV1,
    pub confirmed: bool,
    pub force: bool,
    pub deadline: Instant,
}
#[derive(Debug)]
pub enum DirectMaintenanceErrorV1 {
    Workspace(DirectWorkspaceErrorV1),
    Filesystem(WorkspaceRemovalFilesystemErrorV1),
    ConfirmationRequired,
    ForceRequired,
}
impl fmt::Display for DirectMaintenanceErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "direct maintenance: {self:?}")
    }
}
impl std::error::Error for DirectMaintenanceErrorV1 {}
impl From<DirectWorkspaceErrorV1> for DirectMaintenanceErrorV1 {
    fn from(e: DirectWorkspaceErrorV1) -> Self {
        Self::Workspace(e)
    }
}
impl From<DirectStoreErrorV1> for DirectMaintenanceErrorV1 {
    fn from(e: DirectStoreErrorV1) -> Self {
        Self::Workspace(e.into())
    }
}
impl From<WorkspaceRemovalFilesystemErrorV1> for DirectMaintenanceErrorV1 {
    fn from(e: WorkspaceRemovalFilesystemErrorV1) -> Self {
        Self::Filesystem(e)
    }
}
impl From<crate::workspace::DirectWorkspaceResolutionErrorV1> for DirectMaintenanceErrorV1 {
    fn from(e: crate::workspace::DirectWorkspaceResolutionErrorV1) -> Self {
        Self::Workspace(e.into())
    }
}
impl From<crate::workspace::WorkspaceResolutionErrorV1> for DirectMaintenanceErrorV1 {
    fn from(e: crate::workspace::WorkspaceResolutionErrorV1) -> Self {
        Self::Workspace(e.into())
    }
}
impl From<podway_protocol::ProtocolError> for DirectMaintenanceErrorV1 {
    fn from(e: podway_protocol::ProtocolError) -> Self {
        Self::Workspace(e.into())
    }
}

/// Explicit isolated-fixture instrumentation of Engine-owned marker phases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectMaintenanceBoundaryV1 {
    LeaseAcquired { wait: std::time::Duration },
    BeforeRuntimeDestruction,
    IntentPublished,
    MarkerPublished,
    RuntimeCleared,
    PhaseAdvanced,
    StoreBoundary(DirectTransactionBoundaryV1),
    BeforeFinalRemoval,
    AfterFirstRemovalMarker,
    AfterLastRemovalMarker,
    ResetMarkerRemoved,
    IntentMarkerRemoved,
    DirectoriesRemoved,
}
type Observer = Arc<dyn Fn(DirectMaintenanceBoundaryV1) + Send + Sync>;
type LeaseObserver = Arc<
    dyn Fn(
            DirectMaintenanceBoundaryV1,
            &podway_store::direct_store::DirectStoreV1,
            &ExclusiveMaintenanceLeaseV1,
        ) + Send
        + Sync,
>;

pub fn reset_all_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectMaintenanceRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> Result<OutputEnvelopeV4, DirectMaintenanceErrorV1> {
    reset_inner(resolver, request, clock, ids, None, None)
}
#[doc(hidden)]
pub fn reset_all_with_boundary_observer_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectMaintenanceRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    observer: impl Fn(DirectMaintenanceBoundaryV1) + Send + Sync + 'static,
) -> Result<OutputEnvelopeV4, DirectMaintenanceErrorV1> {
    reset_inner(
        resolver,
        request,
        clock,
        ids,
        Some(Arc::new(observer)),
        None,
    )
}

/// Isolated qualification can inspect the retiring generation under its actual lease.
#[doc(hidden)]
pub fn reset_all_with_lease_observers_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectMaintenanceRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    observer: impl Fn(DirectMaintenanceBoundaryV1) + Send + Sync + 'static,
    lease_observer: impl Fn(
        DirectMaintenanceBoundaryV1,
        &podway_store::direct_store::DirectStoreV1,
        &ExclusiveMaintenanceLeaseV1,
    ) + Send
    + Sync
    + 'static,
) -> Result<OutputEnvelopeV4, DirectMaintenanceErrorV1> {
    reset_inner(
        resolver,
        request,
        clock,
        ids,
        Some(Arc::new(observer)),
        Some(Arc::new(lease_observer)),
    )
}

fn reset_inner<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectMaintenanceRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    observer: Option<Observer>,
    lease_observer: Option<LeaseObserver>,
) -> Result<OutputEnvelopeV4, DirectMaintenanceErrorV1> {
    if !request.confirmed {
        return Err(DirectMaintenanceErrorV1::ConfirmationRequired);
    }
    if request.force {
        return Err(DirectStoreErrorV1::InvalidInput {
            reason: "reset-all has no force flag",
        }
        .into());
    }
    let (_, store) = resolver.resolve_candidate(request.selector.clone())?;
    let store = match observer.as_ref() {
        Some(observer) => {
            let observer = observer.clone();
            store.with_boundary_observer(move |b| {
                observer(DirectMaintenanceBoundaryV1::StoreBoundary(b))
            })
        }
        None => store,
    };
    if let Classification::Newer { generation } = classify_direct_store_v1(store.layout())? {
        return Err(DirectStoreErrorV1::UnsupportedGeneration { generation }.into());
    }
    let acquisition_started = lease_observer.as_ref().map(|_| Instant::now());
    let lease = ExclusiveMaintenanceLeaseV1::acquire(store.layout(), request.deadline, true)?;
    if let Some(started) = acquisition_started {
        notify_lease(
            &lease_observer,
            DirectMaintenanceBoundaryV1::LeaseAcquired {
                wait: started.elapsed(),
            },
            &store,
            &lease,
        );
    }
    let (worktree, _) = resolver.resolve_candidate(request.selector.clone())?;
    resolver.revalidate_candidate(&worktree)?;
    if lease.read_marker(store.layout(), Kind::Removal)?.is_some() {
        return Err(DirectStoreErrorV1::Maintenance.into());
    }
    let digest = identity("workspace.reset_all", &request, &worktree)?;
    let existing_intent = lease.read_marker(store.layout(), Kind::Intent)?;
    let existing_reset = lease.read_marker(store.layout(), Kind::Reset)?;
    let (intent, target, now, response) = if let Some(intent) = existing_intent {
        let value = validate_marker(
            &intent,
            "workspace.reset_all",
            &request,
            &worktree,
            resolver.manifest(),
            &digest,
        )?;
        let target = workspace_from_marker(&value)?.ok_or(DirectStoreErrorV1::Unreadable)?;
        let response = RequestIdV1::new(
            value["response_request_id"]
                .as_str()
                .ok_or(DirectStoreErrorV1::Unreadable)?,
        )?;
        let now = time_from_marker(&value)?;
        (intent, target, now, response)
    } else {
        if existing_reset.is_some() {
            return Err(DirectStoreErrorV1::Maintenance.into());
        }
        match classify_direct_store_v1(store.layout())? {
            Classification::Current => {
                let observed =
                    store.read_under_exclusive_lease(&lease, request.deadline, |snapshot| {
                        if let Some(outcome) = snapshot.outcome(&request.key)? {
                            if outcome.digest() != &digest
                                || outcome.command() != "workspace.reset_all"
                            {
                                return Err(DirectStoreErrorV1::IdempotencyKeyReused);
                            }
                            return Ok(Some(outcome));
                        }
                        guard_workspace(&request, snapshot.binding().identity().workspace_uuid())?;
                        Ok(None)
                    })?;
                if let Some(outcome) = observed {
                    return decode(outcome.response());
                }
            }
            Classification::Legacy { .. }
            | Classification::Unrecognized
            | Classification::Missing => {
                if request.expected_workspace_id.is_some()
                    || request.selector.durable_identity().is_some()
                {
                    return Err(DirectStoreErrorV1::BindingMismatch.into());
                }
            }
            Classification::Newer { generation } => {
                return Err(DirectStoreErrorV1::UnsupportedGeneration { generation }.into());
            }
            Classification::Maintenance => return Err(DirectStoreErrorV1::Maintenance.into()),
        }
        let target = ids.next_workspace_id();
        let now = clock.now();
        let intent = make_marker(
            Kind::Intent,
            "workspace.reset_all",
            &request,
            &worktree,
            resolver.manifest(),
            &digest,
            Some(&target),
            now,
            &request.request_id,
            None,
        )?;
        lease.write_marker(store.layout(), &intent, None)?;
        notify(&observer, DirectMaintenanceBoundaryV1::IntentPublished);
        (intent, target, now, request.request_id.clone())
    };
    let intent_value: serde_json::Value = serde_json::from_str(intent.document().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    let generation = intent_value["operation_id"]
        .as_str()
        .ok_or(DirectStoreErrorV1::Unreadable)?;
    let mutation = DirectMutationRequestV1::new(
        request.key.clone(),
        digest.clone(),
        "workspace.reset_all",
        response.as_str(),
        DirectOutcomeScopeV1::Workspace,
    )?;
    let mut marker = match existing_reset {
        Some(marker) => {
            validate_marker(
                &marker,
                "workspace.reset_all",
                &request,
                &worktree,
                resolver.manifest(),
                &digest,
            )?;
            match_intent(&intent, &marker)?;
            marker
        }
        None => {
            // A crash after reset-marker cleanup leaves the durable intent and new success.
            if store
                .layout()
                .database()
                .try_exists()
                .map_err(DirectStoreErrorV1::from)?
            {
                let observed =
                    store.read_under_exclusive_lease(&lease, request.deadline, |snapshot| {
                        if snapshot.binding().identity().workspace_uuid() != &target {
                            return Ok(None);
                        }
                        snapshot.outcome(&request.key)
                    });
                let observed = match observed {
                    Ok(value) => value,
                    Err(DirectStoreErrorV1::Legacy { .. } | DirectStoreErrorV1::Unreadable) => None,
                    Err(error) => return Err(error.into()),
                };
                if let Some(outcome) = observed {
                    if outcome.digest() != &digest || outcome.command() != "workspace.reset_all" {
                        return Err(DirectStoreErrorV1::IdempotencyKeyReused.into());
                    }
                    let output = decode(outcome.response())?;
                    lease.remove_marker(store.layout(), &intent)?;
                    notify(&observer, DirectMaintenanceBoundaryV1::IntentMarkerRemoved);
                    return Ok(output);
                }
            }
            let marker = make_marker(
                Kind::Reset,
                "workspace.reset_all",
                &request,
                &worktree,
                resolver.manifest(),
                &digest,
                Some(&target),
                now,
                &response,
                Some("prepared"),
            )?;
            lease.write_marker(store.layout(), &marker, None)?;
            notify(&observer, DirectMaintenanceBoundaryV1::MarkerPublished);
            marker
        }
    };
    if phase(&marker)? == "prepared" {
        podway_git::WorkspaceLayoutInitializerV1::new()
            .initialize_with_config(&worktree, crate::direct_workspace::DEFAULT_DIRECT_CONFIG)
            .map_err(DirectWorkspaceErrorV1::Layout)?;
    }
    let (worktree, _) = resolver.resolve_candidate(request.selector.clone())?;
    resolver.revalidate_candidate(&worktree)?;
    let removal = ValidatedPodwayRemovalDirectoryV1::open(&worktree)?
        .ok_or(WorkspaceRemovalFilesystemErrorV1::UnsafePath)?;
    if phase(&marker)? == "prepared" {
        lease.revalidate(store.layout())?;
        resolver.revalidate_candidate(&worktree)?;
        notify_lease(
            &lease_observer,
            DirectMaintenanceBoundaryV1::BeforeRuntimeDestruction,
            &store,
            &lease,
        );
        removal.clear_direct_contents(false)?;
        removal.remove_direct_legacy_configuration_marker()?;
        notify(&observer, DirectMaintenanceBoundaryV1::RuntimeCleared);
        let next = with_phase(&marker, "runtime_removed")?;
        lease.write_marker(store.layout(), &next, Some(&marker))?;
        marker = next;
        notify(&observer, DirectMaintenanceBoundaryV1::PhaseAdvanced);
    }
    let outcome = if store
        .layout()
        .database()
        .try_exists()
        .map_err(DirectStoreErrorV1::from)?
    {
        store.read_under_exclusive_lease(&lease, request.deadline, |snapshot| {
            if snapshot.binding().identity().workspace_uuid() != &target {
                return Err(DirectStoreErrorV1::BindingMismatch);
            }
            let outcome = snapshot
                .outcome(&request.key)?
                .ok_or(DirectStoreErrorV1::Maintenance)?;
            if outcome.digest() != &digest || outcome.command() != "workspace.reset_all" {
                return Err(DirectStoreErrorV1::IdempotencyKeyReused);
            }
            Ok(outcome)
        })?
    } else if let Some(outcome) = store.recover_bootstrap_publication(
        &lease,
        &target,
        generation,
        &mutation,
        request.deadline,
    )? {
        outcome
    } else {
        store
            .bootstrap(
                &lease,
                target.clone(),
                generation,
                &mutation,
                request.deadline,
                || now,
                |transaction| {
                    let output = envelope(
                        &response,
                        "workspace.reset_all",
                        transaction.now(),
                        Some((&target, &worktree, transaction.next_workspace_sequence()?)),
                        serde_json::json!({"reset":true,"revision":0,"workspace_uuid":target}),
                    )
                    .map_err(|_| DirectStoreErrorV1::InvalidInput {
                        reason: "invalid reset-all success",
                    })?;
                    canonical(output)
                },
            )?
            .outcome()
            .clone()
    };
    let output = decode(outcome.response())?;
    if phase(&marker)? != "store_created" {
        let next = with_phase(&marker, "store_created")?;
        lease.write_marker(store.layout(), &next, Some(&marker))?;
        marker = next;
        notify(&observer, DirectMaintenanceBoundaryV1::PhaseAdvanced);
    }
    lease.remove_marker(store.layout(), &marker)?;
    notify(&observer, DirectMaintenanceBoundaryV1::ResetMarkerRemoved);
    lease.remove_marker(store.layout(), &intent)?;
    notify(&observer, DirectMaintenanceBoundaryV1::IntentMarkerRemoved);
    Ok(output)
}

pub fn remove_workspace_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectMaintenanceRequestV1,
    clock: &impl ExecutionClockV1,
) -> Result<OutputEnvelopeV4, DirectMaintenanceErrorV1> {
    remove_inner(resolver, request, clock, None, None)
}
#[doc(hidden)]
pub fn remove_workspace_with_boundary_observer_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectMaintenanceRequestV1,
    clock: &impl ExecutionClockV1,
    observer: impl Fn(DirectMaintenanceBoundaryV1) + Send + Sync + 'static,
) -> Result<OutputEnvelopeV4, DirectMaintenanceErrorV1> {
    remove_inner(resolver, request, clock, Some(Arc::new(observer)), None)
}
/// The normal API has no lease callback or acquisition instrumentation.
#[doc(hidden)]
pub fn remove_workspace_with_lease_observers_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectMaintenanceRequestV1,
    clock: &impl ExecutionClockV1,
    observer: impl Fn(DirectMaintenanceBoundaryV1) + Send + Sync + 'static,
    lease_observer: impl Fn(
        DirectMaintenanceBoundaryV1,
        &podway_store::direct_store::DirectStoreV1,
        &ExclusiveMaintenanceLeaseV1,
    ) + Send
    + Sync
    + 'static,
) -> Result<OutputEnvelopeV4, DirectMaintenanceErrorV1> {
    remove_inner(
        resolver,
        request,
        clock,
        Some(Arc::new(observer)),
        Some(Arc::new(lease_observer)),
    )
}
fn remove_inner<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectMaintenanceRequestV1,
    clock: &impl ExecutionClockV1,
    observer: Option<Observer>,
    lease_observer: Option<LeaseObserver>,
) -> Result<OutputEnvelopeV4, DirectMaintenanceErrorV1> {
    if !request.confirmed {
        return Err(DirectMaintenanceErrorV1::ConfirmationRequired);
    }
    let (initial, store) = resolver.resolve_candidate(request.selector.clone())?;
    if !store
        .layout()
        .podway_directory()
        .try_exists()
        .map_err(DirectStoreErrorV1::from)?
    {
        return removal_result(
            &request.request_id,
            clock.now(),
            &initial,
            None,
            true,
            false,
        );
    }
    match classify_direct_store_v1(store.layout())? {
        Classification::Legacy { generation, marker } => {
            return Err(DirectStoreErrorV1::Legacy { generation, marker }.into());
        }
        Classification::Newer { generation } => {
            return Err(DirectStoreErrorV1::UnsupportedGeneration { generation }.into());
        }
        Classification::Unrecognized => return Err(DirectStoreErrorV1::Unreadable.into()),
        Classification::Missing => {
            // Without a Store or recovery marker, no file is deletion authority.
            // The descriptor traversal accepts directories only and revalidates
            // their ancestry before removing an empty residual.
            resolver.revalidate_candidate(&initial)?;
            let removal = ValidatedPodwayRemovalDirectoryV1::open(&initial)?
                .ok_or(WorkspaceRemovalFilesystemErrorV1::UnsafePath)?;
            removal.remove_empty_residual()?;
            notify(&observer, DirectMaintenanceBoundaryV1::DirectoriesRemoved);
            return removal_result(&request.request_id, clock.now(), &initial, None, true, true);
        }
        Classification::Maintenance => {
            // Final removal can retain only intent after unlinking both locks.
            // Refuse a mismatched recovery request before recreating them;
            // the exclusive lease still revalidates every marker before effects.
            if let Some(marker) = observe_direct_maintenance_marker_v1(store.layout())? {
                let digest = identity("workspace.remove", &request, &initial)?;
                let value = validate_marker(
                    &marker,
                    "workspace.remove",
                    &request,
                    &initial,
                    resolver.manifest(),
                    &digest,
                )?;
                if let Some(uuid) = workspace_from_marker(&value)? {
                    guard_workspace(&request, &uuid)?;
                }
            }
        }
        Classification::Current => {}
    }
    let acquisition_started = lease_observer.as_ref().map(|_| Instant::now());
    let lease = ExclusiveMaintenanceLeaseV1::acquire(store.layout(), request.deadline, true)?;
    if let Some(started) = acquisition_started {
        notify_lease(
            &lease_observer,
            DirectMaintenanceBoundaryV1::LeaseAcquired {
                wait: started.elapsed(),
            },
            &store,
            &lease,
        );
    }
    let (worktree, _) = resolver.resolve_candidate(request.selector.clone())?;
    resolver.revalidate_candidate(&worktree)?;
    if lease.read_marker(store.layout(), Kind::Reset)?.is_some() {
        return Err(DirectStoreErrorV1::Maintenance.into());
    }
    let digest = identity("workspace.remove", &request, &worktree)?;
    let existing_intent = lease.read_marker(store.layout(), Kind::Intent)?;
    let existing_removal = lease.read_marker(store.layout(), Kind::Removal)?;
    let (intent, uuid, now, response) = if let Some(intent) = existing_intent {
        let value = validate_marker(
            &intent,
            "workspace.remove",
            &request,
            &worktree,
            resolver.manifest(),
            &digest,
        )?;
        let uuid = workspace_from_marker(&value)?;
        if let Some(uuid) = uuid.as_ref() {
            guard_workspace(&request, uuid)?;
        }
        let now = time_from_marker(&value)?;
        let response = RequestIdV1::new(
            value["response_request_id"]
                .as_str()
                .ok_or(DirectStoreErrorV1::Unreadable)?,
        )?;
        (intent, uuid, now, response)
    } else {
        if existing_removal.is_some() {
            return Err(DirectStoreErrorV1::Maintenance.into());
        }
        let uuid = match classify_direct_store_v1(store.layout())? {
            Classification::Current => {
                let (uuid, active) =
                    store.read_under_exclusive_lease(&lease, request.deadline, |snapshot| {
                        guard_workspace(&request, snapshot.binding().identity().workspace_uuid())?;
                        let active = snapshot.current_graph()?.is_some_and(|graph| {
                            matches!(
                                graph.trace().lifecycle(),
                                podway_core::SessionLifecycle::Prepared
                                    | podway_core::SessionLifecycle::Running
                            )
                        });
                        Ok((
                            snapshot.binding().identity().workspace_uuid().clone(),
                            active,
                        ))
                    })?;
                if active && !request.force {
                    return Err(DirectMaintenanceErrorV1::ForceRequired);
                }
                Some(uuid)
            }
            Classification::Missing => None,
            Classification::Legacy { generation, marker } => {
                return Err(DirectStoreErrorV1::Legacy { generation, marker }.into());
            }
            Classification::Newer { generation } => {
                return Err(DirectStoreErrorV1::UnsupportedGeneration { generation }.into());
            }
            Classification::Maintenance => return Err(DirectStoreErrorV1::Maintenance.into()),
            Classification::Unrecognized => return Err(DirectStoreErrorV1::Unreadable.into()),
        };
        let now = clock.now();
        let intent = make_marker(
            Kind::Intent,
            "workspace.remove",
            &request,
            &worktree,
            resolver.manifest(),
            &digest,
            uuid.as_ref(),
            now,
            &request.request_id,
            None,
        )?;
        lease.write_marker(store.layout(), &intent, None)?;
        notify(&observer, DirectMaintenanceBoundaryV1::IntentPublished);
        (intent, uuid, now, request.request_id.clone())
    };
    let mut marker = match existing_removal {
        Some(marker) => {
            validate_marker(
                &marker,
                "workspace.remove",
                &request,
                &worktree,
                resolver.manifest(),
                &digest,
            )?;
            match_intent(&intent, &marker)?;
            marker
        }
        None => {
            let marker = make_marker(
                Kind::Removal,
                "workspace.remove",
                &request,
                &worktree,
                resolver.manifest(),
                &digest,
                uuid.as_ref(),
                now,
                &response,
                Some("prepared"),
            )?;
            lease.write_marker(store.layout(), &marker, None)?;
            notify(&observer, DirectMaintenanceBoundaryV1::MarkerPublished);
            marker
        }
    };
    let removal = ValidatedPodwayRemovalDirectoryV1::open(&worktree)?
        .ok_or(WorkspaceRemovalFilesystemErrorV1::UnsafePath)?;
    if phase(&marker)? == "prepared" {
        lease.revalidate(store.layout())?;
        resolver.revalidate_candidate(&worktree)?;
        notify_lease(
            &lease_observer,
            DirectMaintenanceBoundaryV1::BeforeRuntimeDestruction,
            &store,
            &lease,
        );
        removal.clear_direct_contents(false)?;
        notify(&observer, DirectMaintenanceBoundaryV1::RuntimeCleared);
        let next = with_phase(&marker, "runtime_removed")?;
        lease.write_marker(store.layout(), &next, Some(&marker))?;
        marker = next;
        notify(&observer, DirectMaintenanceBoundaryV1::PhaseAdvanced);
    }
    if phase(&marker)? == "runtime_removed" {
        lease.revalidate(store.layout())?;
        resolver.revalidate_candidate(&worktree)?;
        removal.clear_direct_contents(true)?;
        let next = with_phase(&marker, "configuration_removed")?;
        lease.write_marker(store.layout(), &next, Some(&marker))?;
        marker = next;
        notify(&observer, DirectMaintenanceBoundaryV1::PhaseAdvanced);
    }
    // Prepare the complete response before namespace deletion destroys retention.
    let output = removal_result(
        &response,
        now,
        &worktree,
        uuid.as_ref(),
        uuid.is_none(),
        true,
    )?;
    removal.finish_direct_removal(
        || {
            lease
                .revalidate(store.layout())
                .map_err(|_| WorkspaceRemovalFilesystemErrorV1::UnsafePath)?;
            if lease
                .read_marker(store.layout(), Kind::Intent)
                .map_err(|_| WorkspaceRemovalFilesystemErrorV1::MarkerChanged)?
                .as_ref()
                != Some(&intent)
                || lease
                    .read_marker(store.layout(), Kind::Removal)
                    .map_err(|_| WorkspaceRemovalFilesystemErrorV1::MarkerChanged)?
                    .as_ref()
                    != Some(&marker)
            {
                return Err(WorkspaceRemovalFilesystemErrorV1::MarkerChanged);
            }
            notify(&observer, DirectMaintenanceBoundaryV1::BeforeFinalRemoval);
            Ok(())
        },
        || {
            notify(
                &observer,
                DirectMaintenanceBoundaryV1::AfterFirstRemovalMarker,
            )
        },
        || {
            notify(
                &observer,
                DirectMaintenanceBoundaryV1::AfterLastRemovalMarker,
            )
        },
    )?;
    notify(&observer, DirectMaintenanceBoundaryV1::DirectoriesRemoved);
    Ok(output)
}

fn guard_workspace(
    request: &DirectMaintenanceRequestV1,
    actual: &WorkspaceId,
) -> Result<(), DirectStoreErrorV1> {
    for expected in request.expected_workspace_id.iter().chain(
        request
            .selector
            .durable_identity()
            .map(|i| i.workspace_id()),
    ) {
        if expected != actual {
            return Err(DirectStoreErrorV1::ExpectedWorkspaceUuidMismatch {
                expected: expected.clone(),
                actual: actual.clone(),
            });
        }
    }
    Ok(())
}
fn root_identity(worktree: &ValidatedWorktreeV1) -> serde_json::Value {
    serde_json::json!({"root_path_bytes_base64url":worktree.roots().worktree_root().path_bytes_base64url().as_str(),
        "common_dir_identity":worktree.identity().common_directory_fingerprint(),
        "worktree_admin_identity":worktree.identity().worktree_administration_fingerprint()})
}
fn identity(
    command: &str,
    request: &DirectMaintenanceRequestV1,
    worktree: &ValidatedWorktreeV1,
) -> Result<Sha256Digest, DirectStoreErrorV1> {
    let canonical = canonicalize_json_v1(
        &serde_json::json!({"command":command,"root_identity":root_identity(worktree),
        "confirmed":request.confirmed}),
    )
    .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    Ok(
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
            .expect("SHA-256 is canonical"),
    )
}
#[allow(clippy::too_many_arguments)]
fn make_marker(
    kind: Kind,
    command: &str,
    request: &DirectMaintenanceRequestV1,
    worktree: &ValidatedWorktreeV1,
    manifest: &Sha256Digest,
    digest: &Sha256Digest,
    uuid: Option<&WorkspaceId>,
    now: UnixMillis,
    response: &RequestIdV1,
    phase: Option<&str>,
) -> Result<Marker, DirectMaintenanceErrorV1> {
    let schema = match kind {
        Kind::Intent => "podway.workspace-maintenance-intent-marker/v1",
        Kind::Reset => "podway.workspace-reset-marker/v1",
        Kind::Removal => "podway.workspace-removal-marker/v2",
    };
    let mut value = serde_json::json!({"schema":schema,"operation_id":response,"idempotency_key":request.key.as_str(),
        "request_digest":digest,"response_request_id":response,"workspace_uuid":uuid,
        "root_identity":root_identity(worktree),"contract_manifest_digest":manifest,
        "created_at":Rfc3339MillisV1::from_unix_millis(now.get())?,"command":command});
    if let Some(phase) = phase {
        value["phase"] = serde_json::json!(phase);
    }
    if !validate_daemon_free_document_v1(schema, &value) {
        return Err(DirectStoreErrorV1::InvalidInput {
            reason: "invalid maintenance marker",
        }
        .into());
    }
    let canonical = canonicalize_json_v1(&value).map_err(|_| DirectStoreErrorV1::Unreadable)?;
    Ok(Marker::new(
        kind,
        CanonicalExecutionJsonV1::new(canonical).map_err(|_| DirectStoreErrorV1::Unreadable)?,
    )?)
}
fn validate_marker(
    marker: &Marker,
    command: &str,
    request: &DirectMaintenanceRequestV1,
    worktree: &ValidatedWorktreeV1,
    manifest: &Sha256Digest,
    digest: &Sha256Digest,
) -> Result<serde_json::Value, DirectMaintenanceErrorV1> {
    let value: serde_json::Value = serde_json::from_str(marker.document().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    let schema = value["schema"]
        .as_str()
        .ok_or(DirectStoreErrorV1::Unreadable)?;
    if !validate_daemon_free_document_v1(schema, &value)
        || value["command"] != command
        || value["root_identity"] != root_identity(worktree)
        || value["contract_manifest_digest"] != manifest.as_str()
        || value["request_digest"] != digest.as_str()
        || value["idempotency_key"] != request.key.as_str()
    {
        return Err(DirectStoreErrorV1::Maintenance.into());
    }
    Ok(value)
}
fn match_intent(intent: &Marker, operation: &Marker) -> Result<(), DirectStoreErrorV1> {
    let mut intent: serde_json::Value = serde_json::from_str(intent.document().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    let mut operation: serde_json::Value = serde_json::from_str(operation.document().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    intent
        .as_object_mut()
        .ok_or(DirectStoreErrorV1::Unreadable)?
        .remove("schema");
    let object = operation
        .as_object_mut()
        .ok_or(DirectStoreErrorV1::Unreadable)?;
    object.remove("schema");
    object.remove("phase");
    if intent != operation {
        return Err(DirectStoreErrorV1::Maintenance);
    }
    Ok(())
}
fn workspace_from_marker(
    value: &serde_json::Value,
) -> Result<Option<WorkspaceId>, DirectStoreErrorV1> {
    if value["workspace_uuid"].is_null() {
        return Ok(None);
    }
    WorkspaceId::new(
        value["workspace_uuid"]
            .as_str()
            .ok_or(DirectStoreErrorV1::Unreadable)?,
    )
    .map(Some)
    .map_err(|_| DirectStoreErrorV1::Unreadable)
}
fn time_from_marker(value: &serde_json::Value) -> Result<UnixMillis, DirectMaintenanceErrorV1> {
    Ok(UnixMillis::new(
        Rfc3339MillisV1::new(
            value["created_at"]
                .as_str()
                .ok_or(DirectStoreErrorV1::Unreadable)?,
        )?
        .to_unix_millis()
        .ok_or(DirectStoreErrorV1::Unreadable)?,
    ))
}
fn phase(marker: &Marker) -> Result<String, DirectStoreErrorV1> {
    let value: serde_json::Value = serde_json::from_str(marker.document().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    Ok(value["phase"]
        .as_str()
        .ok_or(DirectStoreErrorV1::Unreadable)?
        .to_owned())
}
fn with_phase(marker: &Marker, phase: &str) -> Result<Marker, DirectStoreErrorV1> {
    let mut value: serde_json::Value = serde_json::from_str(marker.document().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    value["phase"] = serde_json::json!(phase);
    let schema = value["schema"]
        .as_str()
        .ok_or(DirectStoreErrorV1::Unreadable)?;
    if !validate_daemon_free_document_v1(schema, &value) {
        return Err(DirectStoreErrorV1::Unreadable);
    }
    Marker::new(
        marker.kind(),
        CanonicalExecutionJsonV1::new(
            canonicalize_json_v1(&value).map_err(|_| DirectStoreErrorV1::Unreadable)?,
        )
        .map_err(|_| DirectStoreErrorV1::Unreadable)?,
    )
}
fn notify(observer: &Option<Observer>, boundary: DirectMaintenanceBoundaryV1) {
    if let Some(observer) = observer {
        observer(boundary);
    }
}
fn notify_lease(
    observer: &Option<LeaseObserver>,
    boundary: DirectMaintenanceBoundaryV1,
    store: &podway_store::direct_store::DirectStoreV1,
    lease: &ExclusiveMaintenanceLeaseV1,
) {
    if let Some(observer) = observer {
        observer(boundary, store, lease);
    }
}
fn canonical(output: OutputEnvelopeV4) -> Result<CanonicalExecutionJsonV1, DirectStoreErrorV1> {
    CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&output).map_err(|_| DirectStoreErrorV1::Unreadable)?,
    )
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "maintenance output exceeds bound",
    })
}
fn decode(
    document: &CanonicalExecutionJsonV1,
) -> Result<OutputEnvelopeV4, DirectMaintenanceErrorV1> {
    Ok(OutputEnvelopeV4::from_value(
        serde_json::from_str(document.as_str()).map_err(|_| DirectStoreErrorV1::Unreadable)?,
    )?)
}
fn envelope(
    response: &RequestIdV1,
    command: &str,
    now: UnixMillis,
    workspace: Option<(&WorkspaceId, &ValidatedWorktreeV1, u64)>,
    result: serde_json::Value,
) -> Result<OutputEnvelopeV4, DirectMaintenanceErrorV1> {
    Ok(OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
        request_id: response.clone(),
        command: CommandNameV1::new(command)?,
        generated_at: Rfc3339MillisV1::from_unix_millis(now.get())?,
        workspace: workspace
            .map(|(uuid, worktree, sequence)| {
                WorkspaceOutputV1::new(
                    uuid.clone(),
                    worktree.roots().worktree_root().display().as_str(),
                    sequence,
                )
            })
            .transpose()?,
        session: None,
        result: result
            .as_object()
            .expect("maintenance result is an object")
            .clone(),
        warnings: Vec::new(),
    })?)
}
fn removal_result(
    response: &RequestIdV1,
    now: UnixMillis,
    worktree: &ValidatedWorktreeV1,
    uuid: Option<&WorkspaceId>,
    absent: bool,
    removed: bool,
) -> Result<OutputEnvelopeV4, DirectMaintenanceErrorV1> {
    envelope(
        response,
        "workspace.remove",
        now,
        None,
        serde_json::json!({"schema":"podway.workspace-removal-result/v2",
        "worktree_root":worktree.roots().worktree_root().display().as_str(),"workspace_uuid":uuid,
        "podway_directory_removed":removed,"already_absent":absent}),
    )
}
