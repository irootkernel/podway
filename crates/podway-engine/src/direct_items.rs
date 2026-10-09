//! Single-item mutations retain item-local fences and revalidate attachments before the clock.

use std::time::Instant;

use podway_core::{
    ArtifactLocationKindV1, ArtifactValueV1, DomainError, SessionLifecycle, Sha256Digest,
    WorkspaceId, canonicalize_json_v1,
};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{
    CommandNameV1, DirectSessionOutputV1, ItemAddV1, ItemAttachSourceV1, ItemAttachV1, ItemCheckV1,
    ItemClearV1, ItemMutationPreconditionsWireV1, ItemRemoveV1, ItemSetV1, ItemUncheckV1,
    OutputEnvelopeInputV4, OutputEnvelopeV4, RequestIdV1, Rfc3339MillisV1, SliceCommandV1,
    WorkspaceOutputV1, canonical_direct_slice_mutation_identity_v1,
};
use podway_store::{
    CanonicalExecutionJsonV1, GraphMutationErrorV2, IdempotencyKeyV1,
    direct_store::{DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1},
};
use sha2::{Digest as _, Sha256};

use crate::{
    direct_execution::DirectExecutionErrorV1,
    execution::{ArtifactVerifierV1, ExecutionClockV1, prepare_procedure_v2_item_mutation_v1},
    native_execution::NativeDirectWorkspaceV1,
    workspace::DirectWorkspaceResolverV1,
};

#[derive(Clone, Debug)]
pub struct DirectRecordManyRequestV1 {
    pub request_id: RequestIdV1,
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub key: IdempotencyKeyV1,
    pub input: podway_protocol::ItemRecordManyV1,
    pub deadline: Instant,
}

fn validate_record_many_input_bound(
    request: &DirectRecordManyRequestV1,
    workspace_uuid: &WorkspaceId,
) -> Result<(), DirectExecutionErrorV1> {
    // Count the retained stdin document, including fences and key, without
    // cloning its values or allocating an encoded request. Both public entry
    // points reach this boundary before canonicalization and preparation.
    #[derive(serde::Serialize)]
    struct Input<'a> {
        schema: &'static str,
        workspace_uuid: &'a WorkspaceId,
        session_id: &'a podway_core::SessionId,
        session_revision: podway_core::Revision,
        attempt_id: &'a podway_core::AttemptId,
        idempotency_key: &'a str,
        operations: &'a [podway_protocol::ItemRecordManyOperationV1],
    }
    struct Budget {
        remaining: usize,
        exceeded: bool,
    }
    impl std::io::Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.remaining {
                self.exceeded = true;
                return Err(std::io::Error::other("direct input byte bound exceeded"));
            }
            self.remaining -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let fences = &request.input.preconditions;
    let input = Input {
        schema: podway_protocol::ITEM_RECORD_MANY_INPUT_SCHEMA_V1,
        workspace_uuid,
        session_id: &fences.expected_session_id,
        session_revision: fences.expected_session_revision,
        attempt_id: &fences.expected_attempt_id,
        idempotency_key: request.key.as_str(),
        operations: &request.input.operations,
    };
    let mut budget = Budget {
        remaining: podway_protocol::MAX_DIRECT_INPUT_BYTES_V1,
        exceeded: false,
    };
    if serde_json::to_writer(&mut budget, &input).is_err() {
        return Err(if budget.exceeded {
            DirectExecutionErrorV1::InputTooLarge
        } else {
            DirectStoreErrorV1::InvalidInput {
                reason: "batch input cannot be encoded",
            }
            .into()
        });
    }
    Ok(())
}

/// The same typed operations serve parsed stdin and programmatic callers.
pub fn record_many_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    mut request: DirectRecordManyRequestV1,
    clock: &impl ExecutionClockV1,
) -> Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    use crate::{
        direct_mutation::{check_attempt_fence, check_session_fences, graph_success},
        execution::{ResolvedBatchArtifactV1, prepare_procedure_v2_item_mutations_v1},
    };
    let (resolved, store, binding) = resolver.resolve_existing_with_binding(
        request.selector.clone(),
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    validate_record_many_input_bound(&request, resolved.store_identity().workspace_uuid())?;
    if request.input.operations.is_empty() || request.input.operations.len() > 128 {
        return Err(DirectStoreErrorV1::InvalidInput {
            reason: "batch requires 1 through 128 operations",
        }
        .into());
    }
    let mut seen = std::collections::HashSet::new();
    for operation in &request.input.operations {
        if !seen.insert(&operation.item_id) {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "duplicate batch item",
            }
            .into());
        }
    }
    request
        .input
        .operations
        .sort_by(|left, right| left.item_id.cmp(&right.item_id));
    let command = SliceCommandV1::ItemRecordMany(request.input.clone());
    let canonical = canonical_direct_slice_mutation_identity_v1(
        &command,
        resolved.store_identity().workspace_uuid(),
    )
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "invalid batch semantic identity",
    })?;
    let digest = Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
        .expect("SHA-256 produces a canonical digest");
    let fences = &request.input.preconditions;
    let mutation = DirectMutationRequestV1::new(
        request.key,
        digest,
        command.command_name(),
        request.request_id.as_str(),
        DirectOutcomeScopeV1::Session(fences.expected_session_id.clone()),
    )?;
    let native = NativeDirectWorkspaceV1::new(resolved.clone());
    let attachments = request
        .input
        .operations
        .iter()
        .filter_map(|operation| {
            use podway_protocol::ItemRecordManyDispositionV1;
            let ItemRecordManyDispositionV1::Record { record } = &operation.disposition else {
                return None;
            };
            record.artifact_source().map(|source| {
                let source = source.map_err(|_| DirectStoreErrorV1::InvalidInput {
                    reason: "invalid batch artifact source",
                })?;
                let artifact = match source {
                    ItemAttachSourceV1::Path { path, media_type } => native
                        .hash_local_artifact(&binding, &path, media_type.as_deref())
                        .map_err(DirectExecutionErrorV1::Boundary)?,
                    ItemAttachSourceV1::OpaqueReference {
                        reference,
                        digest,
                        size_bytes,
                        media_type,
                    } => ArtifactValueV1::external_reference(
                        &reference,
                        digest,
                        size_bytes,
                        &media_type,
                    )
                    .map_err(DirectExecutionErrorV1::Domain)?,
                };
                Ok(ResolvedBatchArtifactV1 {
                    item_id: operation.item_id.clone(),
                    artifact,
                })
            })
        })
        .collect::<Result<Vec<_>, DirectExecutionErrorV1>>();
    let prepared_items = attachments
        .as_ref()
        .ok()
        .map(|attachments| prepare_procedure_v2_item_mutations_v1(&command, attachments));
    let prepared = store.prepare_graph(request.deadline)?;
    let mut execution_error = None;
    let result = store.mutate(&mutation, request.deadline, || clock.now(), |transaction| {
        let current = transaction.use_prepared_graph(prepared.as_ref())?;
        let current = current.as_ref().ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        let prior = current.as_ref();
        check_session_fences(prior, &fences.expected_session_id, fences.expected_session_revision)?;
        if prior.trace().lifecycle() != SessionLifecycle::Running { return Err(GraphMutationErrorV2::SessionNotRunning.into()); }
        check_attempt_fence(prior, &fences.expected_attempt_id)?;
        let active = prior.trace().active_attempt().ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        let memory = prior.workflow_memory().attempts().iter().find(|memory| memory.attempt_id() == active.attempt_id())
            .ok_or(DirectStoreErrorV1::Unreadable)?;
        for operation in &request.input.operations {
            let slot = memory.item_slots().iter().find(|slot| slot.item_id() == &operation.item_id)
                .ok_or_else(|| GraphMutationErrorV2::ItemNotFound { item_id: operation.item_id.clone() })?;
            if slot.revision() != operation.expected_item_revision {
                return Err(GraphMutationErrorV2::ItemRevisionConflict {
                    expected: operation.expected_item_revision, actual: slot.revision(),
                }.into());
            }
        }
        resolver.revalidate(&resolved, transaction.binding()).map_err(|error| {
            execution_error = Some(DirectExecutionErrorV1::from(error));
            DirectStoreErrorV1::InvalidInput { reason: "workspace revalidation failed" }
        })?;
        let attachments = attachments.map_err(|error| {
            execution_error = Some(error);
            DirectStoreErrorV1::InvalidInput { reason: "batch attachment preparation failed" }
        })?;
        let prepared_items = prepared_items.expect("successful attachment preparation produces item mutations")
            .map_err(|error| {
                execution_error = Some(DirectExecutionErrorV1::Preparation(error));
                DirectStoreErrorV1::InvalidInput { reason: "batch preparation failed" }
            })?;
        for attachment in &attachments {
            if attachment.artifact.location_kind() == ArtifactLocationKindV1::LocalPath
                && let Err(error) = native.revalidate_local_artifact(transaction.binding(), &attachment.item_id, &attachment.artifact) {
                execution_error = Some(DirectExecutionErrorV1::Boundary(error));
                return Err(DirectStoreErrorV1::InvalidInput { reason: "batch attachment revalidation failed" });
            }
        }
        let outcome = prior.mutate_active_items_v2(&fences.expected_attempt_id, &prepared_items, transaction.now())?;
        let next = outcome.state();
        let items = request.input.operations.iter().zip(outcome.items()).map(|(operation, item)| {
            let mut item_result = serde_json::json!({"item_id":item.item_id(), "expected_item_revision":operation.expected_item_revision,
                "changed":item.changed(), "item_revision":item.item_revision()});
            if let Some(digest) = item.value_digest() { item_result["value_digest"] = serde_json::json!(digest); }
            item_result
        }).collect::<Vec<_>>();
        let body = serde_json::json!({"schema":"podway.item-record-many-result/v2", "changed":outcome.changed(),
            "graph_node_id":active.graph_node_id(), "attempt_id":active.attempt_id(), "attempt_number":active.number().get(),
            "revision":next.trace().revision(), "items":items});
        let response = graph_success(transaction, &resolved, request.request_id, command.command_name(), prior, next,
            body.as_object().expect("batch result is an object").clone())?;
        if outcome.changed() { transaction.replace_graph(prior.workspace_revision(), prior.trace().revision(), next)?; }
        Ok(response)
    });
    if let Some(error) = execution_error {
        return Err(error);
    }
    let result = result?;
    serde_json::from_str(result.outcome().response().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable.into())
}

#[derive(Clone, Debug)]
pub enum DirectItemCommandV1 {
    Check(ItemCheckV1),
    Uncheck(ItemUncheckV1),
    Set(ItemSetV1),
    Add(ItemAddV1),
    Remove(ItemRemoveV1),
    Attach(ItemAttachV1),
    Clear(ItemClearV1),
}

impl DirectItemCommandV1 {
    fn item_id(&self) -> &podway_core::ItemId {
        match self {
            Self::Check(input) => &input.item_id,
            Self::Uncheck(input) => &input.item_id,
            Self::Set(input) => &input.item_id,
            Self::Add(input) => &input.item_id,
            Self::Remove(input) => &input.item_id,
            Self::Attach(input) => &input.item_id,
            Self::Clear(input) => &input.item_id,
        }
    }
    pub(crate) fn slice_command(&self) -> SliceCommandV1 {
        match self {
            Self::Check(input) => SliceCommandV1::ItemCheck(input.clone()),
            Self::Uncheck(input) => SliceCommandV1::ItemUncheck(input.clone()),
            Self::Set(input) => SliceCommandV1::ItemSet(input.clone()),
            Self::Add(input) => SliceCommandV1::ItemAdd(input.clone()),
            Self::Remove(input) => SliceCommandV1::ItemRemove(input.clone()),
            Self::Attach(input) => SliceCommandV1::ItemAttach(input.clone()),
            Self::Clear(input) => SliceCommandV1::ItemClear(input.clone()),
        }
    }
    pub(crate) fn preconditions(&self) -> &ItemMutationPreconditionsWireV1 {
        match self {
            Self::Check(input) => &input.preconditions,
            Self::Uncheck(input) => &input.preconditions,
            Self::Set(input) => &input.preconditions,
            Self::Add(input) => &input.preconditions,
            Self::Remove(input) => &input.preconditions,
            Self::Attach(input) => &input.preconditions,
            Self::Clear(input) => &input.preconditions,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirectItemRequestV1 {
    pub request_id: RequestIdV1,
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub key: IdempotencyKeyV1,
    pub command: DirectItemCommandV1,
    pub deadline: Instant,
}

pub fn execute_item_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectItemRequestV1,
    clock: &impl ExecutionClockV1,
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
        reason: "invalid item semantic identity",
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
    let native = NativeDirectWorkspaceV1::new(resolved.clone());
    // Preparation stays outside the writer gate, but its failure belongs to the
    // mutation callback. An authoritative replay must not depend on today's file.
    let attachment = (|| {
        Ok::<_, DirectExecutionErrorV1>(match &request.command {
            DirectItemCommandV1::Attach(input) => Some(match &input.source {
                ItemAttachSourceV1::Path { path, media_type } => native
                    .hash_local_artifact(&binding, path, media_type.as_deref())
                    .map_err(DirectExecutionErrorV1::Boundary)?,
                ItemAttachSourceV1::OpaqueReference {
                    reference,
                    digest,
                    size_bytes,
                    media_type,
                } => ArtifactValueV1::external_reference(
                    reference,
                    digest.clone(),
                    *size_bytes,
                    media_type,
                )
                .map_err(DirectExecutionErrorV1::Domain)?,
            }),
            _ => None,
        })
    })();
    let prepared_item = attachment
        .as_ref()
        .ok()
        .map(|attachment| prepare_procedure_v2_item_mutation_v1(&command, attachment.as_ref()));
    let item_id = request.command.item_id();
    let expected_attempt = &fences.expected_attempt_id;
    let expected_item_revision = fences.expected_item_revision;
    let prepared = store.prepare_graph(request.deadline)?;
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
        if prior.trace().lifecycle() != SessionLifecycle::Running {
            return Err(GraphMutationErrorV2::SessionNotRunning.into());
        }
        let active = prior.trace().active_attempt().ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        if active.attempt_id() != expected_attempt {
            return Err(GraphMutationErrorV2::AttemptNotCurrent {
                expected: expected_attempt.clone(), actual: Some(active.attempt_id().clone()),
            }.into());
        }
        let slot = prior.workflow_memory().attempts().iter().find(|memory| memory.attempt_id() == expected_attempt)
            .and_then(|memory| memory.item_slots().iter().find(|slot| slot.item_id() == item_id))
            .ok_or_else(|| GraphMutationErrorV2::ItemNotFound { item_id: item_id.clone() })?;
        if slot.revision() != expected_item_revision {
            return Err(GraphMutationErrorV2::ItemRevisionConflict { expected: expected_item_revision, actual: slot.revision() }.into());
        }
        resolver.revalidate(&resolved, transaction.binding()).map_err(|error| {
            execution_error = Some(DirectExecutionErrorV1::from(error));
            DirectStoreErrorV1::InvalidInput { reason: "workspace revalidation failed" }
        })?;
        let attachment = attachment.map_err(|error| {
            execution_error = Some(error);
            DirectStoreErrorV1::InvalidInput { reason: "attachment preparation failed" }
        })?;
        let (_, _, _, item_mutation) = prepared_item
            .expect("successful attachment preparation produces a prepared mutation")
            .map_err(|error| {
                execution_error = Some(DirectExecutionErrorV1::Preparation(error));
                DirectStoreErrorV1::InvalidInput { reason: "item preparation failed" }
            })?.into_parts();
        if let Some(artifact) = &attachment
            && artifact.location_kind() == ArtifactLocationKindV1::LocalPath
            && let Err(error) = native.revalidate_local_artifact(transaction.binding(), item_id, artifact) {
                execution_error = Some(DirectExecutionErrorV1::Boundary(error));
                return Err(DirectStoreErrorV1::InvalidInput { reason: "attachment revalidation failed" });
        }
        let now = transaction.now();
        let outcome = prior.mutate_active_item_v2(expected_attempt, item_id, expected_item_revision, item_mutation, now)?;
        let next = outcome.state();
        let mut body = serde_json::json!({"schema":"podway.item-mutation-result/v3", "changed":outcome.changed(),
            "graph_node_id":active.graph_node_id(), "attempt_id":active.attempt_id(), "attempt_number":active.number().get(),
            "item_id":item_id, "revision":next.trace().revision()});
        if let Some(digest) = outcome.value_digest() { body["value_digest"] = serde_json::json!(digest); }
        let response = OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
            request_id: request.request_id, command: CommandNameV1::new(command.command_name()).expect("static command"),
            generated_at: Rfc3339MillisV1::from_unix_millis(now.get())
                .map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "clock exceeds the public timestamp range" })?,
            workspace: Some(WorkspaceOutputV1::new(transaction.binding().identity().workspace_uuid().clone(),
                resolved.worktree().roots().worktree_root().display().as_str(), transaction.next_workspace_sequence()?)
                .map_err(|_| DirectStoreErrorV1::BindingMismatch)?),
            session: Some(DirectSessionOutputV1 { id: next.trace().session_id().clone(), title: next.task_title().into(),
                lifecycle: next.trace().lifecycle(), revision_before: prior.trace().revision(), revision_after: next.trace().revision() }),
            result: body.as_object().expect("item result is an object").clone(), warnings: Vec::new(),
        }).map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "item success violates the canonical envelope" })?;
        let response = CanonicalExecutionJsonV1::new(canonicalize_json_v1(response.as_value())
            .map_err(|_| DirectStoreErrorV1::Unreadable)?)
            .map_err(|_| DirectStoreErrorV1::InvalidInput { reason: "item success exceeds the outcome bound" })?;
        if outcome.changed() { transaction.replace_graph(prior.workspace_revision(), prior.trace().revision(), next)?; }
        Ok(response)
    });
    if let Some(error) = execution_error {
        return Err(error);
    }
    let result = result?;
    serde_json::from_str(result.outcome().response().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable.into())
}
