//! Native worktree-local initialization, diagnostics and binding repair.

use crate::{
    direct_execution::DirectIdSourceV1,
    execution::{
        ArtifactVerifierV1, ExecutionBoundaryErrorV1, ExecutionClockV1, required_local_artifacts_v2,
    },
    native_execution::NativeDirectWorkspaceV1,
    workspace::{
        DirectWorkspaceResolutionErrorV1, DirectWorkspaceResolverV1, WorkspaceResolutionErrorV1,
    },
};
use podway_core::{Sha256Digest, UnixMillis, WorkspaceId, canonicalize_json_v1};
use podway_git::{
    GitResolverContractV1, ValidatedWorktreeV1, WorkspaceLayoutErrorV1,
    WorkspaceLayoutInitializerV1, WorktreeSelectorV1,
};
use podway_protocol::{
    CommandNameV1, OutputEnvelopeInputV4, OutputEnvelopeV4, ProtocolError, RequestIdV1,
    Rfc3339MillisV1, SliceCommandV1, WorkspaceInitV1, WorkspaceOutputV1, WorkspaceRepairV1,
    canonical_direct_slice_mutation_identity_v1, validate_daemon_free_document_v1,
};
use podway_store::{
    CanonicalExecutionJsonV1, IdempotencyKeyV1,
    direct_store::{
        DirectMaintenanceMarkerKindV1, DirectMaintenanceMarkerV1, DirectMutationRequestV1,
        DirectOutcomeScopeV1, DirectStoreClassificationV1, DirectStoreErrorV1,
        DirectTransactionBoundaryV1, ExclusiveMaintenanceLeaseV1, classify_direct_store_v1,
    },
};
use sha2::{Digest as _, Sha256};
use std::{fmt, sync::Arc, time::Instant};

#[derive(Debug)]
pub enum DirectWorkspaceErrorV1 {
    Resolution(DirectWorkspaceResolutionErrorV1),
    Store(DirectStoreErrorV1),
    Protocol(ProtocolError),
    Layout(WorkspaceLayoutErrorV1),
    Boundary(ExecutionBoundaryErrorV1),
}
impl fmt::Display for DirectWorkspaceErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "direct workspace command: {self:?}")
    }
}
impl std::error::Error for DirectWorkspaceErrorV1 {}
impl From<DirectWorkspaceResolutionErrorV1> for DirectWorkspaceErrorV1 {
    fn from(value: DirectWorkspaceResolutionErrorV1) -> Self {
        Self::Resolution(value)
    }
}
impl From<WorkspaceResolutionErrorV1> for DirectWorkspaceErrorV1 {
    fn from(value: WorkspaceResolutionErrorV1) -> Self {
        Self::Resolution(value.into())
    }
}
impl From<DirectStoreErrorV1> for DirectWorkspaceErrorV1 {
    fn from(value: DirectStoreErrorV1) -> Self {
        Self::Store(value)
    }
}
impl From<ProtocolError> for DirectWorkspaceErrorV1 {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

#[derive(Clone, Debug)]
pub struct DirectWorkspaceReadRequestV1 {
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub request_id: RequestIdV1,
    pub deadline: Instant,
}

#[derive(Clone, Debug)]
pub struct DirectWorkspaceMutationRequestV1 {
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub request_id: RequestIdV1,
    pub key: IdempotencyKeyV1,
    pub deadline: Instant,
}

pub fn show_workspace_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectWorkspaceReadRequestV1,
    clock: &impl ExecutionClockV1,
) -> Result<OutputEnvelopeV4, DirectWorkspaceErrorV1> {
    let (resolved, store) = resolver.resolve_existing(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    store.read(request.deadline, |snapshot| {
        Ok((|| {
            resolver.revalidate(&resolved, snapshot.binding())?;
            let binding = snapshot.binding();
            envelope(
                &request.request_id,
                "workspace.show",
                clock.now(),
                binding.identity().workspace_uuid(),
                resolved.worktree(),
                snapshot.committed_sequence()?,
                serde_json::json!({"initialized":true, "schema_generation":binding.schema_version(),
                "contract_manifest_digest":resolver.manifest(),
                "stored_root":binding.last_validated_root().as_encoded(),
                "root_rebind_required":binding.last_validated_root() != resolved.workspace_root(),
                "git_common_fingerprint":binding.identity().common_dir_identity(),
                "git_worktree_fingerprint":binding.identity().worktree_admin_identity()}),
            )
        })())
    })?
}

/// Runs actual SQLite integrity, foreign-key and complete graph reconstruction
/// under native exclusion. Deep mode additionally rehashes current required local artifacts.
pub fn doctor_workspace_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectWorkspaceReadRequestV1,
    deep: bool,
    clock: &impl ExecutionClockV1,
) -> Result<OutputEnvelopeV4, DirectWorkspaceErrorV1> {
    let (resolved, store) = resolver.resolve_existing(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    let lease = ExclusiveMaintenanceLeaseV1::acquire(store.layout(), request.deadline, false)?;
    let checked = store.verify_integrity_under_exclusive_lease(&lease, request.deadline, |snapshot| Ok((|| {
        resolver.revalidate(&resolved, snapshot.binding())?;
        let mut checked_artifacts = 0;
        if deep {
            // The deep rehash reuses the verifier's admitted graph; it still
            // precedes the clock and the view inside this callback.
            checked_artifacts += snapshot.with_admitted_current_graph(
                |graph| -> Result<usize, DirectWorkspaceErrorV1> {
                    let Some(graph) = graph else {
                        return Ok(0);
                    };
                    let artifacts = required_local_artifacts_v2(graph)
                        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
                    let native = NativeDirectWorkspaceV1::new(resolved.clone());
                    let mut checked = 0;
                    for (item, artifact) in artifacts {
                        native
                            .revalidate_local_artifact(snapshot.binding(), &item, &artifact)
                            .map_err(DirectWorkspaceErrorV1::Boundary)?;
                        checked += 1;
                    }
                    Ok(checked)
                },
            )?;
        }
        let observed_at = clock.now();
        let view = snapshot.view(observed_at)?;
        if view.graph().is_some() && crate::direct_read_service::project_direct_status_v1(
            &view, crate::v2_read_service::GraphStatusTierV2::Verbose, None).is_err() {
            return envelope(&request.request_id, "workspace.doctor", observed_at,
                snapshot.binding().identity().workspace_uuid(), resolved.worktree(), snapshot.committed_sequence()?,
                serde_json::json!({"healthy":false, "deep":deep,
                    "checks":[{"id":"storage_integrity", "status":"pass"},
                        {"id":"public_status_projection", "status":"fail", "reason":"PUBLIC_PROJECTION_INVALID"}]}));
        }
        envelope(&request.request_id, "workspace.doctor", observed_at,
            snapshot.binding().identity().workspace_uuid(), resolved.worktree(), snapshot.committed_sequence()?,
            serde_json::json!({"healthy":true, "deep":deep,
                "checks":[{"id":"binding_identity", "status":"pass"},
                    {"id":"schema_object_identity", "status":"pass"},
                    {"id":"writer_identity", "status":"pass"},
                    {"id":"quick_check", "status":"pass"},
                    {"id":"foreign_key_check", "status":"pass"},
                    {"id":"current_and_archived_graph_reconstruction", "status":"pass"},
                    {"id":"retained_outcome_records", "status":"pass"},
                    {"id":"public_status_projection", "status":"pass"},
                    {"id":"current_required_local_artifacts", "status":if deep {"pass"} else {"skipped"}}],
                "current_required_local_artifacts_checked":checked_artifacts}))
    })()));
    match checked {
        Ok(output) => output,
        // A readable binding remains useful for diagnosis when persisted domain
        // records fail reconstruction. This read stays under the same lease;
        // neither a damaged graph nor infrastructure diagnostics are projected.
        Err(DirectStoreErrorV1::Unreadable | DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 { .. })) =>
            store.read_under_exclusive_lease(&lease, request.deadline, |snapshot| Ok((|| {
                resolver.revalidate(&resolved, snapshot.binding())?;
                let observed_at = clock.now();
                envelope(&request.request_id, "workspace.doctor", observed_at,
                    snapshot.binding().identity().workspace_uuid(), resolved.worktree(), snapshot.committed_sequence()?,
                    serde_json::json!({"healthy":false, "deep":deep,
                        "checks":[{"id":"binding_identity", "status":"pass"},
                            {"id":"schema_object_identity", "status":"pass"},
                            {"id":"writer_identity", "status":"pass"},
                            {"id":"storage_integrity", "status":"fail", "reason":"STORAGE_INTEGRITY_FAILED"}]}))
            })()))?,
        Err(error) => Err(error.into()),
    }
}

/// Binding repair is a normal fenced success transaction. Read-only resolution
/// reports a moved root but never updates the persisted locator.
pub fn repair_workspace_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectWorkspaceMutationRequestV1,
    clock: &impl ExecutionClockV1,
) -> Result<OutputEnvelopeV4, DirectWorkspaceErrorV1> {
    existing_mutation(
        resolver,
        request,
        SliceCommandV1::WorkspaceRepair(WorkspaceRepairV1 {}),
        clock,
    )
}

fn existing_mutation<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectWorkspaceMutationRequestV1,
    command: SliceCommandV1,
    clock: &impl ExecutionClockV1,
) -> Result<OutputEnvelopeV4, DirectWorkspaceErrorV1> {
    let (resolved, store) = resolver.resolve_existing(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    let mutation = mutation(
        &command,
        resolved.store_identity().workspace_uuid(),
        &request.key,
        &request.request_id,
    )?;
    let plain_init = matches!(
        &command,
        SliceCommandV1::WorkspaceInit(WorkspaceInitV1 { repair: false })
    );
    if plain_init
        && store.read(request.deadline, |snapshot| {
            Ok(snapshot.outcome(&request.key)?.is_none())
        })?
    {
        // A retained key still reaches the normal exact-replay validation. A
        // new plain init cannot replace or recommit an existing workspace.
        return Err(WorkspaceResolutionErrorV1::BootstrapBindingAlreadyPresent.into());
    }
    let outcome = store.mutate_typed(&mutation, request.deadline, || clock.now(), |transaction| -> Result<_, DirectWorkspaceErrorV1> {
        resolver.revalidate(&resolved, transaction.binding()).map_err(DirectWorkspaceErrorV1::from)?;
        if plain_init {
            // Retention can remove the preliminary outcome before BEGIN. Only
            // the transaction's retained exact success may replay plain init.
            return Err(WorkspaceResolutionErrorV1::BootstrapBindingAlreadyPresent.into());
        }
        let repaired = transaction.binding().last_validated_root() != resolved.workspace_root();
        let result = if matches!(command, SliceCommandV1::WorkspaceInit(_)) {
            serde_json::json!({"schema":"podway.workspace-init-result/v2", "initialized":true,"revision":0})
        } else { serde_json::json!({"changed":repaired,
            "changes":if repaired { vec!["workspace_binding.last_validated_root"] } else { Vec::<&str>::new() },
            "moved_identity_proven":true}) };
        let uuid = transaction.binding().identity().workspace_uuid().clone();
        let sequence = transaction.next_workspace_sequence()?;
        canonical(envelope(&request.request_id, command.command_name(), transaction.now(),
            &uuid, resolved.worktree(), sequence, result).map_err(|_| DirectStoreErrorV1::InvalidInput {
                reason:"invalid workspace success envelope" })?).map_err(DirectWorkspaceErrorV1::from)
    });
    let outcome =
        outcome.map_err(podway_store::direct_store::DirectMutationFailureV1::into_error)?;
    decode(outcome.outcome().response())
}

pub fn initialize_workspace_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectWorkspaceMutationRequestV1,
    repair: bool,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> Result<OutputEnvelopeV4, DirectWorkspaceErrorV1> {
    initialize_workspace_inner_v1(resolver, request, repair, clock, ids, None)
}

/// Explicit isolated-fixture instrumentation; no environment flags or product
/// configuration alter the native initialization protocol.
#[doc(hidden)]
pub fn initialize_workspace_with_boundary_observer_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectWorkspaceMutationRequestV1,
    repair: bool,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    observer: impl Fn(DirectTransactionBoundaryV1) + Send + Sync + 'static,
) -> Result<OutputEnvelopeV4, DirectWorkspaceErrorV1> {
    initialize_workspace_inner_v1(
        resolver,
        request,
        repair,
        clock,
        ids,
        Some(Arc::new(observer)),
    )
}

fn initialize_workspace_inner_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectWorkspaceMutationRequestV1,
    repair: bool,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    observer: Option<Arc<dyn Fn(DirectTransactionBoundaryV1) + Send + Sync>>,
) -> Result<OutputEnvelopeV4, DirectWorkspaceErrorV1> {
    let command = SliceCommandV1::WorkspaceInit(WorkspaceInitV1 { repair });
    let (candidate, store) = resolver.resolve_candidate(request.selector.clone())?;
    let classification = classify_direct_store_v1(store.layout())?;
    if let DirectStoreClassificationV1::Newer { generation } = classification {
        return Err(DirectStoreErrorV1::UnsupportedGeneration { generation }.into());
    }
    if let Err(error) = resolver.validate_workspace_config(&candidate) {
        if classification == DirectStoreClassificationV1::Maintenance {
            return Err(DirectStoreErrorV1::Maintenance.into());
        }
        return Err(error.into());
    }
    let store = match observer {
        Some(observer) => store.with_boundary_observer(move |boundary| observer(boundary)),
        None => store,
    };
    match classification {
        DirectStoreClassificationV1::Current => {
            return existing_mutation(resolver, request, command, clock);
        }
        DirectStoreClassificationV1::Missing | DirectStoreClassificationV1::Maintenance => {}
        DirectStoreClassificationV1::Legacy { generation, marker } => {
            return Err(DirectStoreErrorV1::Legacy { generation, marker }.into());
        }
        DirectStoreClassificationV1::Newer { generation } => {
            return Err(DirectStoreErrorV1::UnsupportedGeneration { generation }.into());
        }
        DirectStoreClassificationV1::Unrecognized => {
            return Err(DirectStoreErrorV1::Unreadable.into());
        }
    }
    if (request.expected_workspace_id.is_some() || request.selector.durable_identity().is_some())
        && classify_direct_store_v1(store.layout())? == DirectStoreClassificationV1::Missing
    {
        return Err(DirectStoreErrorV1::NotInitialized.into());
    }
    let lease = ExclusiveMaintenanceLeaseV1::acquire(store.layout(), request.deadline, true)?;
    // Runtime directory creation is monotone; re-resolve it before taking it as Git authority.
    let (candidate, _) = resolver.resolve_candidate(request.selector.clone())?;
    resolver.revalidate_candidate(&candidate)?;
    for kind in [
        DirectMaintenanceMarkerKindV1::Reset,
        DirectMaintenanceMarkerKindV1::Removal,
    ] {
        if lease.read_marker(store.layout(), kind)?.is_some() {
            return Err(DirectStoreErrorV1::Maintenance.into());
        }
    }
    let existing = lease.read_marker(store.layout(), DirectMaintenanceMarkerKindV1::Intent)?;
    let (marker, workspace, mutation, submitted_at, response_id) = if let Some(marker) = existing {
        let value: serde_json::Value = serde_json::from_str(marker.document().as_str())
            .map_err(|_| DirectStoreErrorV1::Unreadable)?;
        if !validate_daemon_free_document_v1(
            "podway.workspace-maintenance-intent-marker/v1",
            &value,
        ) || value["command"] != "workspace.init"
            || value["contract_manifest_digest"] != resolver.manifest().as_str()
            || value["root_identity"] != root_identity(&candidate)
        {
            return Err(DirectStoreErrorV1::Maintenance.into());
        }
        let workspace = WorkspaceId::new(
            value["workspace_uuid"]
                .as_str()
                .ok_or(DirectStoreErrorV1::Unreadable)?,
        )
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
        if request
            .expected_workspace_id
            .as_ref()
            .is_some_and(|expected| expected != &workspace)
        {
            return Err(DirectStoreErrorV1::BindingMismatch.into());
        }
        let response_id = RequestIdV1::new(
            value["response_request_id"]
                .as_str()
                .ok_or(DirectStoreErrorV1::Unreadable)?,
        )?;
        let mutation = mutation(&command, &workspace, &request.key, &response_id)?;
        if value["idempotency_key"] != request.key.as_str()
            || value["request_digest"] != mutation.digest().as_str()
        {
            return Err(DirectStoreErrorV1::Maintenance.into());
        }
        let timestamp = Rfc3339MillisV1::new(
            value["created_at"]
                .as_str()
                .ok_or(DirectStoreErrorV1::Unreadable)?,
        )?;
        (
            marker,
            workspace,
            mutation,
            UnixMillis::new(
                timestamp
                    .to_unix_millis()
                    .ok_or(DirectStoreErrorV1::Unreadable)?,
            ),
            response_id,
        )
    } else {
        // A contender may have completed while this caller waited for exclusivity.
        if classify_direct_store_v1(store.layout())? == DirectStoreClassificationV1::Current {
            drop(lease);
            return existing_mutation(resolver, request, command, clock);
        }
        let workspace = ids.next_workspace_id();
        let mutation = mutation(&command, &workspace, &request.key, &request.request_id)?;
        let now = clock.now();
        let value = serde_json::json!({"schema":"podway.workspace-maintenance-intent-marker/v1",
            "operation_id":request.request_id, "idempotency_key":request.key.as_str(),
            "request_digest":mutation.digest(), "response_request_id":request.request_id,
            "workspace_uuid":workspace, "root_identity":root_identity(&candidate),
            "contract_manifest_digest":resolver.manifest(),
            "created_at":Rfc3339MillisV1::from_unix_millis(now.get())?, "command":"workspace.init"});
        if !validate_daemon_free_document_v1(
            "podway.workspace-maintenance-intent-marker/v1",
            &value,
        ) {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "invalid init intent",
            }
            .into());
        }
        let document = CanonicalExecutionJsonV1::new(
            canonicalize_json_v1(&value).map_err(|_| DirectStoreErrorV1::Unreadable)?,
        )
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
        let marker =
            DirectMaintenanceMarkerV1::new(DirectMaintenanceMarkerKindV1::Intent, document)?;
        lease.write_marker(store.layout(), &marker, None)?;
        (marker, workspace, mutation, now, request.request_id.clone())
    };
    let generation: serde_json::Value = serde_json::from_str(marker.document().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    let generation = generation["operation_id"]
        .as_str()
        .ok_or(DirectStoreErrorV1::Unreadable)?;
    if store
        .layout()
        .database()
        .try_exists()
        .map_err(DirectStoreErrorV1::from)?
    {
        let outcome = store.read_under_exclusive_lease(&lease, request.deadline, |snapshot| {
            if snapshot.binding().identity().workspace_uuid() != &workspace {
                return Err(DirectStoreErrorV1::BindingMismatch);
            }
            snapshot
                .outcome(&request.key)?
                .ok_or(DirectStoreErrorV1::Maintenance)
        })?;
        if outcome.digest() != mutation.digest() || outcome.command() != mutation.command() {
            return Err(DirectStoreErrorV1::IdempotencyKeyReused.into());
        }
        let output = decode(outcome.response())?;
        lease.remove_marker(store.layout(), &marker)?;
        return Ok(output);
    }
    if let Some(outcome) = store.recover_bootstrap_publication(
        &lease,
        &workspace,
        generation,
        &mutation,
        request.deadline,
    )? {
        let output = decode(outcome.response())?;
        lease.remove_marker(store.layout(), &marker)?;
        return Ok(output);
    }
    WorkspaceLayoutInitializerV1::new()
        .initialize_with_config(&candidate, podway_config::DEFAULT_WORKSPACE_CONFIG_YAML_V1)
        .map_err(DirectWorkspaceErrorV1::Layout)?;
    let (candidate, _) = resolver.resolve_candidate(request.selector.clone())?;
    resolver.revalidate_candidate(&candidate)?;
    let outcome = store.bootstrap(&lease, workspace.clone(), generation, &mutation, request.deadline,
        || submitted_at, |transaction| canonical(envelope(&response_id, "workspace.init", transaction.now(),
            &workspace, &candidate, transaction.next_workspace_sequence()?,
            serde_json::json!({"schema":"podway.workspace-init-result/v2","initialized":true,"revision":0}))
            .map_err(|_| DirectStoreErrorV1::InvalidInput { reason:"invalid init envelope" })?))?;
    let output = decode(outcome.outcome().response())?;
    lease.remove_marker(store.layout(), &marker)?;
    Ok(output)
}

fn root_identity(worktree: &ValidatedWorktreeV1) -> serde_json::Value {
    serde_json::json!({"root_path_bytes_base64url":worktree.roots().worktree_root().path_bytes_base64url().as_str(),
        "common_dir_identity":worktree.identity().common_directory_fingerprint(),
        "worktree_admin_identity":worktree.identity().worktree_administration_fingerprint()})
}
fn mutation(
    command: &SliceCommandV1,
    workspace: &WorkspaceId,
    key: &IdempotencyKeyV1,
    response: &RequestIdV1,
) -> Result<DirectMutationRequestV1, DirectStoreErrorV1> {
    let identity =
        canonical_direct_slice_mutation_identity_v1(command, workspace).map_err(|_| {
            DirectStoreErrorV1::InvalidInput {
                reason: "invalid workspace mutation identity",
            }
        })?;
    let digest = Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(identity.as_bytes())))
        .expect("SHA-256 is a canonical digest");
    DirectMutationRequestV1::new(
        key.clone(),
        digest,
        command.command_name(),
        response.as_str(),
        DirectOutcomeScopeV1::Workspace,
    )
}
fn envelope(
    request: &RequestIdV1,
    command: &str,
    now: UnixMillis,
    workspace: &WorkspaceId,
    worktree: &ValidatedWorktreeV1,
    sequence: u64,
    result: serde_json::Value,
) -> Result<OutputEnvelopeV4, DirectWorkspaceErrorV1> {
    Ok(OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
        request_id: request.clone(),
        command: CommandNameV1::new(command)?,
        generated_at: Rfc3339MillisV1::from_unix_millis(now.get())?,
        workspace: Some(WorkspaceOutputV1::new(
            workspace.clone(),
            worktree.roots().worktree_root().display().as_str(),
            sequence,
        )?),
        session: None,
        result: result
            .as_object()
            .expect("workspace result is an object")
            .clone(),
        warnings: Vec::new(),
    })?)
}
fn canonical(output: OutputEnvelopeV4) -> Result<CanonicalExecutionJsonV1, DirectStoreErrorV1> {
    CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&output).map_err(|_| DirectStoreErrorV1::Unreadable)?,
    )
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "workspace result exceeds output bound",
    })
}
fn decode(value: &CanonicalExecutionJsonV1) -> Result<OutputEnvelopeV4, DirectWorkspaceErrorV1> {
    Ok(OutputEnvelopeV4::from_value(
        serde_json::from_str(value.as_str()).map_err(|_| DirectStoreErrorV1::Unreadable)?,
    )?)
}
