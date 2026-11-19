//! Native, source-backed start for an immutable successor base Procedure.

use super::{
    SuccessorExecutionError, SuccessorMutationResponseV1, admit_candidate_definition,
    admit_canonical_definition, canonical_response, presentation,
};
use crate::{
    direct_execution::{DirectExecutionErrorV1, DirectIdSourceV1, DirectSuccessorSourceErrorV1},
    direct_start::{
        DirectStartCommandV1, DirectStartRequestV1, Policy, StartCurrent, current_policy,
        make_bound_disposition, require_start_identity, validate_title,
    },
    execution::{
        ExecutionClockV1, ProcedureV2StartPreparationErrorV1 as StartError,
        procedure_document_format,
    },
    native_execution::NativeDirectWorkspaceV1,
    workspace::{DirectWorkspaceResolutionErrorV1, DirectWorkspaceResolverV1, ResolvedWorkspaceV1},
};
use podway_config::ConfigError;
use podway_config::execution_groups::{
    SuccessorDefinition, ValidatedSuccessorDefinition, parse_successor_base_document,
    validate_successor_definition,
};
use podway_core::{
    ActorAttributionV2, DomainError, ReasonV2, SessionLifecycle, Sha256Digest,
    TerminalDispositionV2, UnixMillis, WorkspaceId,
    execution_groups::{EXECUTION_GROUP_SCHEMA_V1, ExecutionSessionV3},
};
use podway_git::GitResolverContractV1;
use podway_protocol::{
    CommandNameV1, DirectSessionOutputV1, OutputEnvelopeInputV4, RequestIdV1, Rfc3339MillisV1,
    SessionDeletionModeV1, SessionStartSourceV1, SessionStartV1, WorkspaceOutputV1,
    canonical_direct_start_digest_with_observation_v1,
};
use podway_store::{
    CanonicalExecutionJsonV1, WorkspaceBindingV1,
    direct_store::{
        DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1, DirectTransactionV1,
    },
    successor_store::{CandidateSources, CandidateStoreError},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug)]
struct PreparedBase {
    definition: ValidatedSuccessorDefinition,
    source_bytes: Option<Sha256Digest>,
    default_group: Option<Sha256Digest>,
}

/// Prepared start's native transaction. Its result is still internal until the
/// successor result catalog and public dispatch switch together.
#[cfg(feature = "test-support")]
pub fn start_native_successor<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectStartRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> Result<CanonicalExecutionJsonV1, DirectExecutionErrorV1> {
    start_native_successor_with_response(
        resolver,
        request,
        clock,
        ids,
        SuccessorMutationResponseV1::Fixture,
    )
}

pub(crate) fn start_native_successor_with_response<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectStartRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
    response: SuccessorMutationResponseV1,
) -> Result<CanonicalExecutionJsonV1, DirectExecutionErrorV1> {
    let (resolved, store, binding) = resolver.resolve_existing_with_binding(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    let native = resolver.native_workspace(resolved.clone());
    let input = request.command.input();
    let source = prepare(&native, &binding, &input.source);
    let workspace = resolved.store_identity().workspace_uuid();
    let command = request.command.slice();

    if input.dry_run {
        let source = source?;
        confirm(input, &source)?;
        validate_title(input)?;
        return store.read(request.deadline, |snapshot| {
            Ok((|| {
                resolver
                    .revalidate(&resolved, snapshot.binding())
                    .map_err(DirectWorkspaceResolutionErrorV1::from)?;
                revalidate(&native, snapshot.binding(), &input.source, &source)?;
                let state = snapshot
                    .successor_state(admit_candidate_definition)
                    .map_err(candidate_error)?;
                let disposition = state
                    .as_ref()
                    .map(|(state, _)| {
                        let fence = state.fence();
                        snapshot
                            .successor_disposition(&fence.session, fence.revision)
                            .map(|record| record.is_some())
                    })
                    .transpose()
                    .map_err(candidate_error)?
                    .unwrap_or(false);
                let state = state.as_ref().map(|(state, _)| state);
                let policy = choose_policy(&request.command, state, disposition, true)?;
                let body = prepared_body(
                    &source,
                    state.zip(policy),
                    None,
                    command.command_name(),
                    response,
                )?;
                start_response(
                    &request.request_id,
                    command.command_name(),
                    clock.now(),
                    workspace,
                    &resolved,
                    snapshot.committed_sequence()?,
                    None,
                    body,
                    response,
                )
            })())
        })?;
    }

    let key = request
        .key
        .as_ref()
        .ok_or(DirectStoreErrorV1::InvalidInput {
            reason: "start requires an idempotency key",
        })?;
    let source = std::cell::RefCell::new(Some(source));
    let result = store
        .mutate_resolving_identity_typed(
            key,
            request.deadline,
            || clock.now(),
            |outcome| -> Result<_, DirectExecutionErrorV1> {
                let digest = if let Some(outcome) = outcome {
                    if outcome.command() != command.command_name() {
                        return Err(DirectStoreErrorV1::IdempotencyKeyReused.into());
                    }
                    let output: Value = serde_json::from_str(outcome.response().as_str())
                        .map_err(|_| DirectStoreErrorV1::Unreadable)?;
                    let result = &output["result"];
                    let digest =
                        result.get("mutation_result").unwrap_or(result)["procedure_digest"].clone();
                    serde_json::from_value::<Sha256Digest>(digest)
                        .map_err(|_| DirectStoreErrorV1::Unreadable)?
                } else {
                    let mut retained = source.borrow_mut();
                    if retained.as_ref().is_some_and(Result::is_err) {
                        return Err(retained
                            .take()
                            .expect("prepared base was retained")
                            .unwrap_err());
                    }
                    retained
                        .as_ref()
                        .expect("prepared base was retained")
                        .as_ref()
                        .expect("preparation succeeded")
                        .definition
                        .digest()
                        .clone()
                };
                let identity = canonical_direct_start_digest_with_observation_v1(
                    &command,
                    workspace,
                    &digest,
                    response.return_observation(),
                )
                .map_err(|_| DirectStoreErrorV1::InvalidInput {
                    reason: "invalid successor start identity",
                })?;
                Ok(DirectMutationRequestV1::new(
                    key.clone(),
                    identity,
                    command.command_name(),
                    request.request_id.as_str(),
                    DirectOutcomeScopeV1::Workspace,
                )?)
            },
            |transaction| -> Result<_, DirectExecutionErrorV1> {
                resolver
                    .revalidate(&resolved, transaction.binding())
                    .map_err(DirectWorkspaceResolutionErrorV1::from)?;
                let source = source
                    .borrow_mut()
                    .take()
                    .expect("prepared base was retained")?;
                confirm(input, &source)?;
                validate_title(input)?;
                revalidate(&native, transaction.binding(), &input.source, &source)?;
                let prior_row = transaction
                    .successor_state(admit_candidate_definition)
                    .map_err(candidate_error)?;
                let prior = prior_row.as_ref().map(|(prior, _)| prior);
                let disposition = prior
                    .map(|prior| {
                        let fence = prior.fence();
                        transaction
                            .successor_disposition(&fence.session, fence.revision)
                            .map(|record| record.is_some())
                    })
                    .transpose()
                    .map_err(candidate_error)?
                    .unwrap_or(false);
                let policy = choose_policy(&request.command, prior, disposition, false)?;
                let normalized =
                    admit_canonical_definition(source.definition.canonical_json().as_str())
                        .map_err(|_| {
                            DirectExecutionErrorV1::SuccessorSource(
                                DirectSuccessorSourceErrorV1::InvalidAdmittedDefinition,
                            )
                        })?;
                let SuccessorDefinition::Base(base) = normalized.definition() else {
                    return Err(DirectExecutionErrorV1::InvalidRequest);
                };
                let now = transaction.now();
                let session_id = ids.next_session_id();
                let state = ExecutionSessionV3::prepared(
                    session_id.clone(),
                    base.clone(),
                    source.definition.digest().clone(),
                    now,
                );
                let sources = CandidateSources::from([(
                    source.definition.digest().clone(),
                    normalized.canonical_json().as_str().to_owned(),
                )]);
                install_prepared(
                    transaction,
                    &request.command,
                    prior_row.as_ref().zip(policy),
                    &state,
                    &sources,
                )?;
                let mut body = prepared_body(
                    &source,
                    prior.zip(policy),
                    Some(&state),
                    command.command_name(),
                    response,
                )?;
                let sequence = transaction.next_workspace_sequence()?;
                if response.return_observation() {
                    let observation = presentation::public_observation(
                        workspace,
                        &input.task_title,
                        &state,
                        &sources,
                        sequence,
                        podway_protocol::ContractGenerationV1::Generation2,
                        presentation::ObservationActivityV1::Current {
                            terminal_disposition: false,
                        },
                    )?;
                    body = json!({
                        "schema": presentation::MUTATION_OBSERVATION_V2,
                        "workspace_sequence": sequence,
                        "mutation_result": body,
                        "observation": observation,
                    });
                }
                let session = DirectSessionOutputV1 {
                    id: session_id,
                    title: input.task_title.clone(),
                    lifecycle: SessionLifecycle::Prepared,
                    revision_before: podway_core::Revision::ZERO,
                    revision_after: podway_core::Revision::ZERO,
                };
                start_response(
                    &request.request_id,
                    command.command_name(),
                    now,
                    workspace,
                    &resolved,
                    sequence,
                    Some(session),
                    body,
                    response,
                )
            },
        )
        .map_err(podway_store::direct_store::DirectMutationFailureV1::into_error)?;
    Ok(result.outcome().response().clone())
}

fn install_prepared(
    transaction: &mut DirectTransactionV1<'_, '_, impl FnOnce() -> UnixMillis>,
    command: &DirectStartCommandV1,
    prior: Option<(&(ExecutionSessionV3, CandidateSources), Policy)>,
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
) -> Result<(), DirectExecutionErrorV1> {
    let title = &command.input().task_title;
    if let Some(((prior, retained_sources), policy)) = prior {
        let fence = prior.fence();
        match policy {
            Policy::Delete => transaction
                .delete_successor_session(&fence.session, fence.revision)
                .map_err(candidate_error)?,
            Policy::Archive => {
                transaction
                    .archive_successor_session(&fence.session, fence.revision)
                    .map_err(candidate_error)?;
            }
            Policy::Dispose => {
                let DirectStartCommandV1::Replace(replace) = command else {
                    return Err(DirectExecutionErrorV1::InvalidRequest);
                };
                let SessionDeletionModeV1::DisposeAndArchive { disposition, actor } = &replace.mode
                else {
                    return Err(DirectExecutionErrorV1::InvalidRequest);
                };
                let actor = actor
                    .clone()
                    .map(ActorAttributionV2::new)
                    .transpose()
                    .map_err(DirectExecutionErrorV1::Domain)?;
                let disposition = make_bound_disposition(
                    &fence.session,
                    fence.revision,
                    disposition,
                    actor,
                    transaction.now(),
                )?;
                transaction
                    .dispose_successor_session(&disposition)
                    .map_err(candidate_error)?;
                transaction
                    .archive_successor_session(&fence.session, fence.revision)
                    .map_err(candidate_error)?;
            }
            Policy::Supersede => {
                let DirectStartCommandV1::Replace(replace) = command else {
                    return Err(DirectExecutionErrorV1::InvalidRequest);
                };
                let SessionDeletionModeV1::Supersede { reason, actor } = &replace.mode else {
                    return Err(DirectExecutionErrorV1::InvalidRequest);
                };
                let actor = actor
                    .clone()
                    .map(ActorAttributionV2::new)
                    .transpose()
                    .map_err(DirectExecutionErrorV1::Domain)?;
                let now = transaction.now();
                let mut cancelled = prior.clone();
                cancelled
                    .cancel(
                        &fence,
                        ReasonV2::new(reason).map_err(DirectExecutionErrorV1::Domain)?,
                        now,
                    )
                    .map_err(DirectExecutionErrorV1::Domain)?;
                transaction
                    .write_successor_state(fence.revision, &cancelled, retained_sources)
                    .map_err(candidate_error)?;
                let disposition = TerminalDispositionV2::superseded(
                    fence.session,
                    cancelled.fence().revision,
                    reason,
                    state.fence().session,
                    actor,
                    now,
                )
                .map_err(DirectExecutionErrorV1::Domain)?;
                return transaction
                    .archive_superseded_and_create_successor(&disposition, title, state, sources)
                    .map_err(candidate_error);
            }
        }
    }
    transaction
        .create_successor_state(title, state, sources)
        .map_err(candidate_error)
}

fn prepare(
    native: &NativeDirectWorkspaceV1,
    binding: &WorkspaceBindingV1,
    source: &SessionStartSourceV1,
) -> Result<PreparedBase, DirectExecutionErrorV1> {
    match source {
        SessionStartSourceV1::Preset { preset } => {
            let pair = podway_presets::lookup_v3(preset)
                .ok_or(DirectExecutionErrorV1::Start(StartError::PresetNotFound))?
                .validate()
                .map_err(|_| {
                    DirectExecutionErrorV1::SuccessorSource(
                        DirectSuccessorSourceErrorV1::InvalidAdmittedDefinition,
                    )
                })?;
            Ok(PreparedBase {
                definition: pair.base,
                source_bytes: None,
                default_group: Some(pair.default_group.digest().clone()),
            })
        }
        SessionStartSourceV1::Procedure { procedure } => {
            let bytes = native
                .procedure_bytes(binding, procedure)
                .map_err(|error| DirectExecutionErrorV1::Start(StartError::Source(error)))?;
            let definition = admit_base(procedure, &bytes)?;
            Ok(PreparedBase {
                definition,
                source_bytes: Some(raw_digest(&bytes)),
                default_group: None,
            })
        }
    }
}

fn admit_base(
    path: &str,
    bytes: &[u8],
) -> Result<ValidatedSuccessorDefinition, DirectExecutionErrorV1> {
    use DirectSuccessorSourceErrorV1 as E;
    let format = procedure_document_format(path)
        .map_err(|_| DirectExecutionErrorV1::SuccessorSource(E::UnsupportedFormat))?;
    let definition = parse_successor_base_document(bytes, format)
        .map_err(|error| DirectExecutionErrorV1::SuccessorSource(source_error(error)))?;
    validate_successor_definition(definition)
        .map_err(|_| DirectExecutionErrorV1::SuccessorSource(E::InvalidDefinition))
}

fn confirm(input: &SessionStartV1, source: &PreparedBase) -> Result<(), DirectExecutionErrorV1> {
    let digest = source.definition.digest();
    if let Some(expected) = &input.expected_procedure_digest {
        if expected != digest {
            return Err(DirectExecutionErrorV1::Start(
                StartError::ProcedureDigestMismatch {
                    expected: expected.clone(),
                    actual: digest.clone(),
                },
            ));
        }
    } else if !input.dry_run && source.source_bytes.is_some() {
        return Err(DirectExecutionErrorV1::Start(
            StartError::DigestConfirmationRequired {
                procedure_digest: digest.clone(),
            },
        ));
    }
    Ok(())
}

fn revalidate(
    native: &NativeDirectWorkspaceV1,
    binding: &WorkspaceBindingV1,
    source: &SessionStartSourceV1,
    prepared: &PreparedBase,
) -> Result<(), DirectExecutionErrorV1> {
    if let SessionStartSourceV1::Procedure { procedure } = source {
        let bytes = native
            .procedure_bytes(binding, procedure)
            .map_err(|error| DirectExecutionErrorV1::Start(StartError::Source(error)))?;
        if prepared.source_bytes.as_ref() != Some(&raw_digest(&bytes)) {
            let actual = admit_base(procedure, &bytes)?;
            if actual.digest() != prepared.definition.digest() {
                return Err(DirectExecutionErrorV1::Start(
                    StartError::ProcedureDigestMismatch {
                        expected: prepared.definition.digest().clone(),
                        actual: actual.digest().clone(),
                    },
                ));
            }
        }
    }
    Ok(())
}

fn choose_policy(
    command: &DirectStartCommandV1,
    state: Option<&ExecutionSessionV3>,
    disposed: bool,
    dry_run: bool,
) -> Result<Option<Policy>, DirectExecutionErrorV1> {
    let fence = state.map(ExecutionSessionV3::fence);
    let current = state
        .zip(fence.as_ref())
        .map(|(state, fence)| StartCurrent {
            session_id: &fence.session,
            revision: fence.revision,
            lifecycle: state.lifecycle(),
        });
    if let DirectStartCommandV1::Start(command) = command
        && let Some(expected) = &command.preconditions
    {
        let current = current.ok_or_else(|| {
            DirectExecutionErrorV1::Domain(DomainError::SessionIdentityMismatch {
                expected: expected.expected_session_id.clone(),
                actual: None,
            })
        })?;
        require_start_identity(current, expected)?;
    }
    current_policy(command, current, disposed, dry_run)
}

fn prepared_body(
    source: &PreparedBase,
    prior: Option<(&ExecutionSessionV3, Policy)>,
    created: Option<&ExecutionSessionV3>,
    command: &str,
    response: SuccessorMutationResponseV1,
) -> Result<Value, DirectExecutionErrorV1> {
    let SuccessorDefinition::Base(base) = source.definition.definition() else {
        return Err(DirectExecutionErrorV1::InvalidRequest);
    };
    let mut result = json!({"schema":"podway.successor-start-fixture-result/v1", "procedure_schema":"podway.procedure/v3",
        "procedure_digest": source.definition.digest(), "default_group_digest": source.default_group,
        "dry_run": created.is_none(), "session_state":"prepared", "goal_tracking":base.goal_tracking().is_some(),
        "goal_defined":false, "active_attempt":null, "goal_revision":null, "entry_graph_node_id":base.graph().entry()});
    if matches!(response, SuccessorMutationResponseV1::Public { .. }) {
        result["schema"] = json!("podway.successor-session-start-result/v1");
        result["operation"] = json!(command);
    }
    if let Some((prior, policy)) = prior {
        let fence = prior.fence();
        result["existing_session"] = json!({"session_id":fence.session,"session_revision":fence.revision,
        "lifecycle": lifecycle(prior.lifecycle()),"action": match policy {
            Policy::Delete => "delete", Policy::Archive => "archive", Policy::Supersede => "supersede_archive", Policy::Dispose => "dispose_archive",
        }});
    }
    if let Some(state) = created {
        let fence = state.fence();
        result["session_id"] = json!(fence.session);
        result["revision"] = json!(fence.revision);
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn start_response(
    request: &RequestIdV1,
    command: &str,
    now: UnixMillis,
    workspace: &WorkspaceId,
    resolved: &ResolvedWorkspaceV1,
    sequence: u64,
    session: Option<DirectSessionOutputV1>,
    result: Value,
    response: SuccessorMutationResponseV1,
) -> Result<CanonicalExecutionJsonV1, DirectExecutionErrorV1> {
    let mut value = serde_json::to_value(OutputEnvelopeInputV4 {
        request_id: request.clone(),
        command: CommandNameV1::new(command)?,
        generated_at: Rfc3339MillisV1::from_unix_millis(now.get())?,
        workspace: Some(WorkspaceOutputV1::new(
            workspace.clone(),
            resolved
                .worktree()
                .roots()
                .worktree_root()
                .display()
                .as_str(),
            sequence,
        )?),
        session,
        result: result
            .as_object()
            .expect("prepared start response is an object")
            .clone(),
        warnings: vec![],
    })
    .map_err(|_| DirectStoreErrorV1::Unreadable)?;
    value["schema"] = json!(podway_protocol::OUTPUT_SCHEMA_V4);
    if matches!(response, SuccessorMutationResponseV1::Public { .. }) {
        value = podway_protocol::OutputEnvelopeV4::from_value(value)?.into_value();
        if value.get("session").is_some() {
            podway_protocol::validate_direct_outcome_lookup_headroom_v1(&value)
                .map_err(|_| SuccessorExecutionError::OutcomeLookupBudget)?;
        }
    }
    Ok(canonical_response(&value)?)
}

fn candidate_error(error: CandidateStoreError) -> DirectExecutionErrorV1 {
    match error {
        CandidateStoreError::Direct(error) => error.into(),
        CandidateStoreError::Item(error) => DirectStoreErrorV1::State(error).into(),
        CandidateStoreError::Domain(error) => DirectExecutionErrorV1::Domain(error),
        CandidateStoreError::Io(error) => DirectStoreErrorV1::from(error).into(),
        CandidateStoreError::Sql(error) => DirectStoreErrorV1::from(error).into(),
        _ => DirectStoreErrorV1::Unreadable.into(),
    }
}

fn source_error(error: ConfigError) -> DirectSuccessorSourceErrorV1 {
    use DirectSuccessorSourceErrorV1 as E;
    match error {
        ConfigError::InvalidSchema { actual, .. } => E::UnsupportedSchema {
            known_schema: match actual.as_str() {
                podway_core::PROCEDURE_SCHEMA_V2 => Some(podway_core::PROCEDURE_SCHEMA_V2),
                EXECUTION_GROUP_SCHEMA_V1 => Some(EXECUTION_GROUP_SCHEMA_V1),
                _ => None,
            },
        },
        ConfigError::InputTooLarge { .. }
        | ConfigError::InputTooDeep { .. }
        | ConfigError::InputTooComplex { .. } => E::ResourceLimit,
        ConfigError::CoreAdmission { .. } => E::InvalidDefinition,
        _ => E::InvalidDocument,
    }
}

fn raw_digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(bytes)))
        .expect("SHA-256 is a valid digest")
}

fn lifecycle(state: SessionLifecycle) -> &'static str {
    match state {
        SessionLifecycle::Prepared => "prepared",
        SessionLifecycle::Running => "running",
        SessionLifecycle::Completed => "completed",
        SessionLifecycle::Cancelled => "cancelled",
    }
}
