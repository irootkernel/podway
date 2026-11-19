//! Scoped item writes use native artifact preparation and the common success ledger.

use super::*;
use crate::{
    direct_execution::DirectExecutionErrorV1,
    direct_items::DirectItemCommandV1,
    execution::{
        ExecutionClockV1, PreparedProcedureV2ItemMutationV1, prepare_procedure_v2_item_mutation_v1,
    },
    native_execution::{NativeDirectWorkspaceV1, PreparedLocalArtifactV1},
    workspace::{DirectWorkspaceResolutionErrorV1, DirectWorkspaceResolverV1},
};
use podway_core::{
    ArtifactValueV1, ItemTypeV1, MAX_ACTIVE_ITEM_MUTATIONS_V2, RecordedItemValueV2,
    recorded_value_digest_v2,
};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{
    ItemAttachSourceV1, ItemRecordManyDispositionV1, ItemRecordManyOperationV1, ItemRecordValueV1,
    canonical_direct_slice_mutation_identity_v1, serialized_json_len_v1,
};
use podway_store::{WorkspaceBindingV1, successor_store::NativeSuccessorStore};
use std::time::Instant;

#[derive(Clone, Debug)]
pub enum DirectSuccessorWriteV1 {
    Edit {
        attempt: ScopedAttemptV1,
        selected_group: Option<GroupExecutionIdV1>,
        command: DirectItemCommandV1,
    },
    Atomic {
        cursor: SuccessorCursor,
        operations: Vec<ItemRecordManyOperationV1>,
    },
}

impl DirectSuccessorWriteV1 {
    pub fn command_name(&self) -> &'static str {
        match self {
            Self::Edit { command, .. } => command.slice_command().command_name(),
            Self::Atomic { .. } => "item.record_many",
        }
    }
}

pub(crate) struct DirectSuccessorWriteRequestV1 {
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: WorkspaceId,
    pub key: IdempotencyKeyV1,
    pub request_id: RequestIdV1,
    pub command: DirectSuccessorWriteV1,
    pub deadline: Instant,
    pub return_observation: bool,
}

struct PreparedWrite {
    expected_type: Option<ItemTypeV1>,
    write: GroupItemWriteV1,
    artifact: Option<PreparedLocalArtifactV1>,
}

fn prepare_write(
    native: &NativeDirectWorkspaceV1,
    binding: &WorkspaceBindingV1,
    input: &ItemRecordManyOperationV1,
) -> std::result::Result<PreparedWrite, DirectExecutionErrorV1> {
    let mut artifact = None;
    let (expected_type, value) = match &input.disposition {
        ItemRecordManyDispositionV1::Clear { clear: true } => (None, None),
        ItemRecordManyDispositionV1::Clear { clear: false } => {
            return Err(DirectExecutionErrorV1::InvalidRequest);
        }
        ItemRecordManyDispositionV1::Record { record } => {
            let (item_type, value) = match record {
                ItemRecordValueV1::Confirm { value } => (
                    ItemTypeV1::Confirm,
                    value.then(RecordedItemValueV2::confirm),
                ),
                ItemRecordValueV1::Text { value } => (
                    ItemTypeV1::Text,
                    Some(
                        RecordedItemValueV2::text(value.clone())
                            .map_err(DirectExecutionErrorV1::Domain)?,
                    ),
                ),
                ItemRecordValueV1::Choice { value } => (
                    ItemTypeV1::Choice,
                    Some(
                        RecordedItemValueV2::choice(value.clone())
                            .map_err(DirectExecutionErrorV1::Domain)?,
                    ),
                ),
                ItemRecordValueV1::Integer { value } => (
                    ItemTypeV1::Integer,
                    Some(RecordedItemValueV2::integer(*value)),
                ),
                ItemRecordValueV1::List { value } => (
                    ItemTypeV1::List,
                    Some(
                        RecordedItemValueV2::list(value.clone())
                            .map_err(DirectExecutionErrorV1::Domain)?,
                    ),
                ),
                ItemRecordValueV1::CheckResult { value } => (
                    ItemTypeV1::CheckResult,
                    Some(RecordedItemValueV2::check_result(value.clone())),
                ),
                ItemRecordValueV1::Artifact { .. } => {
                    let source = record
                        .artifact_source()
                        .transpose()
                        .map_err(|_| DirectExecutionErrorV1::InvalidRequest)?
                        .ok_or(DirectExecutionErrorV1::InvalidRequest)?;
                    let (value, stamp) = prepare_attachment(native, binding, &source)?;
                    artifact = stamp;
                    (
                        ItemTypeV1::Artifact,
                        Some(RecordedItemValueV2::artifact(value)),
                    )
                }
            };
            (Some(item_type), value)
        }
    };
    Ok(PreparedWrite {
        expected_type,
        write: GroupItemWriteV1 {
            item: input.item_id.clone(),
            expected_revision: input.expected_item_revision,
            value,
        },
        artifact,
    })
}

enum PendingWrite {
    Edit(
        Box<PreparedProcedureV2ItemMutationV1>,
        Option<PreparedLocalArtifactV1>,
    ),
    Atomic(Vec<PreparedWrite>),
}

fn prepare_attachment(
    native: &NativeDirectWorkspaceV1,
    binding: &WorkspaceBindingV1,
    source: &ItemAttachSourceV1,
) -> std::result::Result<(ArtifactValueV1, Option<PreparedLocalArtifactV1>), DirectExecutionErrorV1>
{
    match source {
        ItemAttachSourceV1::Path { path, media_type } => {
            let prepared = native
                .prepare_local_artifact_v1(binding, path, media_type.as_deref())
                .map_err(DirectExecutionErrorV1::Boundary)?;
            Ok((prepared.artifact().clone(), Some(prepared)))
        }
        ItemAttachSourceV1::OpaqueReference {
            reference,
            digest,
            size_bytes,
            media_type,
        } => {
            let artifact = ArtifactValueV1::external_reference(
                reference,
                digest.clone(),
                *size_bytes,
                media_type,
            )
            .map_err(DirectExecutionErrorV1::Domain)?;
            Ok((artifact, None))
        }
    }
}

pub(crate) fn dispatch_successor_write<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    mut request: DirectSuccessorWriteRequestV1,
    clock: &impl ExecutionClockV1,
) -> std::result::Result<CanonicalExecutionJsonV1, DirectExecutionErrorV1> {
    let name = request.command.command_name();
    let workspace = request.expected_workspace_id;
    let (target, fences, body, revisions): (&ScopedAttemptV1, Value, Value, Vec<_>) =
        match &mut request.command {
            DirectSuccessorWriteV1::Edit {
                attempt,
                selected_group,
                command,
            } => {
                let expected = command.preconditions();
                if attempt.session() != &expected.expected_session_id
                    || attempt.attempt() != &expected.expected_attempt_id
                {
                    return Err(DirectExecutionErrorV1::InvalidRequest);
                }
                let semantic = canonical_direct_slice_mutation_identity_v1(
                    &command.slice_command(),
                    &workspace,
                )
                .map_err(|_| DirectExecutionErrorV1::InvalidRequest)?;
                let body = serde_json::from_str(&semantic)
                    .map_err(|_| DirectExecutionErrorV1::InvalidRequest)?;
                (
                    attempt,
                    json!({"attempt": attempt, "selected_group": selected_group}),
                    body,
                    vec![(command.item_id().clone(), expected.expected_item_revision)],
                )
            }
            DirectSuccessorWriteV1::Atomic { cursor, operations } => {
                if operations.is_empty() || operations.len() > MAX_ACTIVE_ITEM_MUTATIONS_V2 {
                    return Err(DirectExecutionErrorV1::InvalidRequest);
                }
                operations.sort_by(|left, right| left.item_id.cmp(&right.item_id));
                if let Some(duplicate) = operations
                    .windows(2)
                    .find(|pair| pair[0].item_id == pair[1].item_id)
                {
                    return Err(GraphMutationErrorV2::DuplicateItemMutation {
                        item_id: duplicate[0].item_id.clone(),
                    }
                    .into());
                }
                if serialized_json_len_v1(operations)
                    .map_err(|_| DirectExecutionErrorV1::InputTooLarge)?
                    > MAX_DIRECT_INPUT_BYTES_V1
                {
                    return Err(DirectExecutionErrorV1::InputTooLarge);
                }
                let body = serde_json::to_value(&operations)
                    .map_err(|_| DirectExecutionErrorV1::InvalidRequest)?;
                let target = cursor
                    .execution
                    .attempt
                    .as_ref()
                    .ok_or(DirectExecutionErrorV1::InvalidRequest)?;
                let revisions = operations
                    .iter()
                    .map(|input| (input.item_id.clone(), input.expected_item_revision))
                    .collect();
                (target, cursor_json(cursor), body, revisions)
            }
        };
    let exact = MutationIdentity {
        workspace: &workspace,
        key: &request.key,
        request_id: &request.request_id,
        session: target.session(),
    }
    .prepare(name, fences, body, request.return_observation)?;
    let (resolved, store, binding) = resolver.resolve_existing_with_binding(
        request.selector,
        Some(&workspace),
        request.deadline,
    )?;
    let native = resolver.native_workspace(resolved.clone());
    let prepared = (|| -> std::result::Result<PendingWrite, DirectExecutionErrorV1> {
        match &request.command {
            DirectSuccessorWriteV1::Edit { command, .. } => {
                let (value, stamp) = if let DirectItemCommandV1::Attach(input) = command {
                    let (value, stamp) = prepare_attachment(&native, &binding, &input.source)?;
                    (Some(value), stamp)
                } else {
                    (None, None)
                };
                let prepared =
                    prepare_procedure_v2_item_mutation_v1(&command.slice_command(), value.as_ref())
                        .map_err(DirectExecutionErrorV1::Preparation)?;
                Ok(PendingWrite::Edit(Box::new(prepared), stamp))
            }
            DirectSuccessorWriteV1::Atomic { operations, .. } => {
                let writes = operations
                    .iter()
                    .map(|operation| prepare_write(&native, &binding, operation))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(PendingWrite::Atomic(writes))
            }
        }
    })();
    let mut adapter = NativeSuccessorStore::new(&store, request.deadline, |_| Ok(()));
    let result = adapter.scoped_mutation(
        &workspace,
        &exact,
        || clock.now(),
        |transaction, now| {
            let (mut state, sources) = transaction.load(admit_candidate_definition)?;
            let exact_attempt = match &request.command {
                DirectSuccessorWriteV1::Edit {
                    attempt,
                    selected_group,
                    ..
                } => {
                    verify_selection(&state, selected_group.as_ref())?;
                    state
                        .verify_attempt(attempt)
                        .map_err(DirectExecutionErrorV1::Domain)?;
                    attempt
                }
                DirectSuccessorWriteV1::Atomic { cursor, .. } => {
                    verify_cursor(&state, cursor)?;
                    cursor
                        .execution
                        .attempt
                        .as_ref()
                        .ok_or(DirectExecutionErrorV1::InvalidRequest)?
                }
            };
            let binding = transaction
                .binding()
                .ok_or(DirectStoreErrorV1::BindingMismatch)?;
            resolver
                .revalidate(&resolved, binding)
                .map_err(DirectWorkspaceResolutionErrorV1::from)?;
            let attempt = state
                .active_attempt()
                .ok_or(DirectExecutionErrorV1::InvalidRequest)?;
            for (item, expected) in &revisions {
                let slot = attempt
                    .slots()
                    .iter()
                    .find(|slot| slot.item_id() == item)
                    .ok_or_else(|| {
                        DirectExecutionErrorV1::Domain(DomainError::ItemNotFound {
                            item_id: item.clone(),
                        })
                    })?;
                if slot.revision() != *expected {
                    return Err(GraphMutationErrorV2::ItemRevisionConflict {
                        expected: *expected,
                        actual: slot.revision(),
                    }
                    .into());
                }
            }
            let verify_stamp = |stamp| {
                if let Some(stamp) = stamp {
                    native
                        .verify_prepared_local_artifact_v1(binding, &stamp)
                        .map_err(DirectExecutionErrorV1::Boundary)?;
                }
                Ok::<_, DirectExecutionErrorV1>(())
            };
            let before = state.fence().revision;
            match prepared? {
                PendingWrite::Edit(edit, stamp) => {
                    verify_stamp(stamp)?;
                    let (item, _, expected, edit) = edit.into_parts();
                    let value = state.prepare_item_edit(exact_attempt, &item, edit)?;
                    state
                        .record_item(exact_attempt, &item, expected, value, now())
                        .map_err(DirectExecutionErrorV1::Domain)?;
                }
                PendingWrite::Atomic(writes) => {
                    let declarations = state
                        .current_item_declarations()
                        .ok_or(DirectStoreErrorV1::Unreadable)?;
                    let writes = writes
                        .into_iter()
                        .map(|prepared| {
                            let declaration = declarations
                                .iter()
                                .find(|item| item.id() == &prepared.write.item)
                                .ok_or(DirectStoreErrorV1::Unreadable)?;
                            if prepared
                                .expected_type
                                .is_some_and(|expected| expected != declaration.item_type())
                            {
                                return Err(GraphMutationErrorV2::ItemTypeMismatch.into());
                            }
                            if let Some(value) = prepared.write.value.as_ref() {
                                if let Some(error) = declaration.recorded_value_bound_error(value) {
                                    return Err(DirectExecutionErrorV1::Domain(error));
                                }
                                if !declaration.admits_recorded_value(value) {
                                    return Err(GraphMutationErrorV2::ItemConstraintFailed.into());
                                }
                            }
                            verify_stamp(prepared.artifact)?;
                            Ok(prepared.write)
                        })
                        .collect::<std::result::Result<Vec<_>, DirectExecutionErrorV1>>()?;
                    let DirectSuccessorWriteV1::Atomic { cursor, .. } = &request.command else {
                        unreachable!("atomic item preparation corresponds to the atomic request");
                    };
                    state.record_many(&cursor.execution, writes, now())?;
                }
            }
            let next = state
                .active_attempt()
                .ok_or(DirectStoreErrorV1::Unreadable)?;
            let items = revisions
                .iter()
                .map(|(item, expected)| {
                    let slot = next
                        .slots()
                        .iter()
                        .find(|slot| slot.item_id() == item)
                        .ok_or(DirectStoreErrorV1::Unreadable)?;
                    let digest = slot
                        .value()
                        .map(recorded_value_digest_v2)
                        .transpose()
                        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
                    Ok::<_, DirectStoreErrorV1>(json!({
                        "item": slot.item_id(),
                        "revision": slot.revision(),
                        "recorded": slot.value().is_some(),
                        "previous_revision": expected,
                        "changed": slot.revision() != *expected,
                        "value_digest": digest,
                    }))
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok::<_, DirectExecutionErrorV1>(persist_transition(
                transaction,
                &exact,
                &state,
                &sources,
                before,
                json!({"items": items}),
                now(),
                SuccessorMutationResponseV1::Public {
                    return_observation: request.return_observation,
                },
            )?)
        },
    )?;
    Ok(result.outcome().response().clone())
}
