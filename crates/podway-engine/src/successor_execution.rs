//! Typed Engine operations for isolated successor Store integration fixtures.

use crate::direct_execution::DirectIdSourceV1;
use podway_config::execution_groups::{
    SuccessorDefinition, ValidatedSuccessorDefinition, parse_successor_document,
    validate_group_interface, validate_successor_definition,
};
use podway_config::{ConfigError, ProcedureDocumentFormat};
use podway_core::execution_groups::*;
use podway_core::{
    ActionOutcomeV2, BoundUnitV2, CheckResultInputBasisV2, DomainError, Sha256Digest, UnixMillis,
    WorkspaceId, canonicalize_json_v1,
};
use podway_protocol::{
    CommandNameV1, DirectSessionOutputV1, MAX_DIRECT_INPUT_BYTES_V1, OUTPUT_SCHEMA_V4,
    OutputEnvelopeInputV4, ProtocolError, RequestIdV1, Rfc3339MillisV1, WorkspaceOutputV1,
    validate_direct_outcome_lookup_headroom_v1,
};
use podway_store::direct_store::{
    DirectMutationRequestV1, DirectMutationResultV1, DirectOutcomeScopeV1, DirectStoreErrorV1,
};
use podway_store::successor_store::{
    CandidateDefinition, CandidateExecutionStore, CandidateExecutionTransaction, CandidateSources,
    CandidateStoreError, CandidateSuccess,
};
use podway_store::{CanonicalExecutionJsonV1, IdempotencyKeyV1};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug)]
pub enum SuccessorExecutionError {
    Config(ConfigError),
    Store(CandidateStoreError),
    Domain(DomainError),
    Protocol(ProtocolError),
    Invalid(&'static str),
    ResponseBudget,
    OutcomeLookupBudget,
}

impl From<ConfigError> for SuccessorExecutionError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}
impl From<CandidateStoreError> for SuccessorExecutionError {
    fn from(error: CandidateStoreError) -> Self {
        Self::Store(error)
    }
}
impl From<DirectStoreErrorV1> for SuccessorExecutionError {
    fn from(error: DirectStoreErrorV1) -> Self {
        Self::Store(error.into())
    }
}
impl From<DomainError> for SuccessorExecutionError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}
impl From<ProtocolError> for SuccessorExecutionError {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

type Result<T> = std::result::Result<T, SuccessorExecutionError>;

/// Caller-owned exact fences. The response UUID is allocated separately from semantic inputs.
pub struct SuccessorMutationRequest {
    pub workspace: WorkspaceId,
    pub key: IdempotencyKeyV1,
    pub request_id: RequestIdV1,
    pub cursor: ExecutionCursorFenceV1,
}

pub struct GroupActivationRequest {
    pub mutation: SuccessorMutationRequest,
    pub document: String,
    pub format: ProcedureDocumentFormat,
    pub expected_definition: Sha256Digest,
    pub plan: PlanBindingV1,
    pub predecessor: Option<GroupExecutionIdV1>,
    pub working_copy_basis: CheckResultInputBasisV2,
    pub prior_work: Vec<PriorWorkDispositionV1>,
}

/// Prepare the Config-admitted canonical definition and source-side boundary before the writer.
/// A prepared error against the observed base applies only if this is a new mutation.
pub fn activate_group(
    store: &mut CandidateExecutionStore,
    request: GroupActivationRequest,
    clock: impl FnOnce() -> UnixMillis,
    ids: &impl DirectIdSourceV1,
) -> Result<DirectMutationResultV1> {
    if request.prior_work.len() > MAX_PRIOR_WORK_DISPOSITIONS_V1 {
        return Err(DomainError::BoundExceeded {
            field: "prior work dispositions",
            actual: request.prior_work.len() as u64,
            maximum: MAX_PRIOR_WORK_DISPOSITIONS_V1 as u64,
            unit: BoundUnitV2::Items,
        }
        .into());
    }
    let prepared = validate_successor_definition(parse_successor_document(
        request.document.as_bytes(),
        request.format,
    )?)?;
    if prepared.digest() != &request.expected_definition {
        return Err(SuccessorExecutionError::Invalid(
            "prepared group definition digest differs",
        ));
    }
    let canonical = prepared.canonical_json().as_str();
    // Retained definitions are reconstructed from normalized canonical bytes, including map order.
    let normalized = admit_canonical_definition(canonical)?;
    let SuccessorDefinition::Group(group) = normalized.definition() else {
        return Err(SuccessorExecutionError::Invalid(
            "activation expects a group definition",
        ));
    };

    let mutation = &request.mutation;
    let exact_request = outcome_request(
        mutation,
        "group.activate",
        json!({
            "group": serde_json::from_str::<Value>(canonical).expect("validated canonical group"),
            "expected_definition": request.expected_definition,
            "plan": {
                "design_attempt": request.plan.design_attempt(),
                "item_set_digest": request.plan.item_set_digest(),
                "items": request.plan.items().iter().map(|item| json!({
                    "item": item.item(), "revision": item.revision(), "value_digest": item.value_digest(),
                })).collect::<Vec<_>>(),
            },
            "predecessor": request.predecessor,
            "working_copy_basis": request.working_copy_basis,
            "prior_work": request.prior_work.iter().map(|work| json!({
                "group": work.group(), "item": work.item(),
                "choice": work.choice(), "rationale": work.rationale(),
            })).collect::<Vec<_>>(),
        }),
    )?;
    let prepared_base = (|| -> Result<Sha256Digest> {
        let (state, sources) =
            store.snapshot(&mutation.cursor.session, admit_candidate_definition)?;
        let base = admit_canonical_definition(source(&sources, state.base_digest())?)?;
        validate_group_interface(&base, &normalized)?;
        Ok(base.digest().clone())
    })();
    store.mutate(&mutation.workspace, &exact_request, |transaction| {
        let (mut state, mut sources) = transaction.load(admit_candidate_definition)?;
        verify_cursor(&state, mutation)?;
        let anchor = state
            .active_attempt()
            .ok_or(SuccessorExecutionError::Invalid(
                "candidate activation has no current attempt",
            ))?;
        if prepared_base? != *state.base_digest()
            || anchor.target().scope() != &ExecutionScopeV1::Base
            || anchor.target().node() != state.base_definition().boundary().anchor()
        {
            return Err(SuccessorExecutionError::Invalid(
                "candidate activation source differs",
            ));
        }
        let identity = ActivationIdentityV1::new(ActivationIdentityInputV1 {
            execution: GroupExecutionIdV1::new(ids.next_session_id().into_inner())?,
            session: mutation.cursor.session.clone(),
            base_digest: state.base_digest().clone(),
            definition_digest: request.expected_definition.clone(),
            anchor: anchor.target().node().clone(),
            goal_revision: mutation.cursor.goal_revision,
            plan: request.plan,
            predecessor: request.predecessor,
            working_copy_basis: request.working_copy_basis,
            prior_work: request.prior_work,
        })?;
        let before = anchor.target().clone();
        let now = clock();
        state.activate(
            &mutation.cursor,
            identity,
            group.clone(),
            normalized.digest(),
            ids.next_attempt_id(),
            now,
        )?;
        sources.insert(normalized.digest().clone(), canonical.to_owned());
        finish_transition(
            transaction,
            mutation,
            &exact_request,
            &state,
            &sources,
            &before,
            now,
        )
    })
}

/// Return only from the current selected group's declared terminal to the base successor.
pub fn return_from_group(
    store: &mut CandidateExecutionStore,
    request: SuccessorMutationRequest,
    clock: impl FnOnce() -> UnixMillis,
    ids: &impl DirectIdSourceV1,
) -> Result<DirectMutationResultV1> {
    let exact_request = outcome_request(&request, "group.return", json!({}))?;
    store.mutate(&request.workspace, &exact_request, |transaction| {
        let (mut state, sources) = transaction.load(admit_candidate_definition)?;
        verify_cursor(&state, &request)?;
        let current = state
            .active_attempt()
            .ok_or(SuccessorExecutionError::Invalid(
                "candidate return has no current attempt",
            ))?
            .target();
        let ExecutionScopeV1::Group {
            group_execution_id,
            definition_digest,
        } = current.scope()
        else {
            return Err(SuccessorExecutionError::Invalid(
                "candidate return requires a group attempt",
            ));
        };
        let selected = state
            .selected()
            .filter(|selected| {
                selected.identity().execution() == group_execution_id
                    && selected.identity().definition_digest() == definition_digest
                    && selected.outcome() == GroupExecutionOutcomeV1::Running
            })
            .ok_or(SuccessorExecutionError::Invalid(
                "candidate return requires selected running group",
            ))?;
        let group = state
            .group_definition(selected.identity().definition_digest())
            .ok_or(SuccessorExecutionError::Invalid(
                "candidate return group definition is absent",
            ))?;
        if !group.graph().nodes().iter().any(|node| {
            node.id() == current.node()
                && matches!(
                    node.kind(),
                    PlacementKindV3::Action {
                        outcome: ActionOutcomeV2::Terminal,
                        ..
                    },
                )
        }) {
            return Err(SuccessorExecutionError::Invalid(
                "candidate return requires the group terminal placement",
            ));
        }
        let before = current.clone();
        let now = clock();
        state.complete(&request.cursor, Some(ids.next_attempt_id()), now)?;
        finish_transition(
            transaction,
            &request,
            &exact_request,
            &state,
            &sources,
            &before,
            now,
        )
    })
}

fn verify_cursor(state: &ExecutionSessionV3, request: &SuccessorMutationRequest) -> Result<()> {
    if state.fence() != request.cursor {
        return Err(SuccessorExecutionError::Invalid(
            "candidate cursor fence differs",
        ));
    }
    Ok(())
}

fn outcome_request(
    request: &SuccessorMutationRequest,
    command: &str,
    body: Value,
) -> Result<DirectMutationRequestV1> {
    let identity = canonicalize_json_v1(&json!({
        "command": command,
        "workspace": request.workspace,
        "session": request.cursor.session,
        "session_revision": request.cursor.revision,
        "attempt": request.cursor.attempt,
        "goal_revision": request.cursor.goal_revision.map(|revision| revision.get()),
        "body": body,
    }))
    .map_err(|_| SuccessorExecutionError::Invalid("candidate request identity is not canonical"))?;
    if identity.len() > MAX_DIRECT_INPUT_BYTES_V1 {
        return Err(SuccessorExecutionError::Invalid(
            "candidate request exceeds the direct input bound",
        ));
    }
    let digest = Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(identity.as_bytes())))?;
    Ok(DirectMutationRequestV1::new(
        request.key.clone(),
        digest,
        command,
        request.request_id.as_str(),
        DirectOutcomeScopeV1::Session(request.cursor.session.clone()),
    )?)
}

fn admit_canonical_definition(
    canonical: &str,
) -> std::result::Result<ValidatedSuccessorDefinition, ConfigError> {
    validate_successor_definition(parse_successor_document(
        canonical.as_bytes(),
        ProcedureDocumentFormat::Json,
    )?)
}

fn admit_candidate_definition(
    canonical: &str,
) -> std::result::Result<CandidateDefinition, CandidateStoreError> {
    let admitted = admit_canonical_definition(canonical)
        .map_err(|_| CandidateStoreError::Corrupt("candidate definition fails Config admission"))?;
    if admitted.canonical_json().as_str() != canonical {
        return Err(CandidateStoreError::Corrupt(
            "candidate definition is not normalized",
        ));
    }
    Ok(match admitted.definition() {
        SuccessorDefinition::Base(base) => CandidateDefinition::Base(base.clone()),
        SuccessorDefinition::Group(group) => CandidateDefinition::Group(group.clone()),
    })
}

fn source<'a>(sources: &'a CandidateSources, digest: &Sha256Digest) -> Result<&'a str> {
    sources
        .get(digest)
        .map(String::as_str)
        .ok_or(SuccessorExecutionError::Invalid(
            "candidate definition bytes are absent",
        ))
}

fn finish_transition(
    transaction: &CandidateExecutionTransaction<'_, '_>,
    request: &SuccessorMutationRequest,
    identity: &DirectMutationRequestV1,
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
    closed: &ScopedAttemptV1,
    now: UnixMillis,
) -> Result<CandidateSuccess> {
    let active = state
        .active_attempt()
        .ok_or(SuccessorExecutionError::Invalid(
            "candidate transition did not produce an active successor",
        ))?
        .target();
    let selected = state.selected().ok_or(SuccessorExecutionError::Invalid(
        "candidate transition did not preserve selection",
    ))?;
    let group = state
        .group_definition(selected.identity().definition_digest())
        .ok_or(SuccessorExecutionError::Invalid(
            "candidate selected definition is absent",
        ))?;
    let result = json!({
        "schema": "podway.successor-group-boundary-fixture-result/v1",
        "revision_before": request.cursor.revision,
        "revision": state.fence().revision,
        "closed_attempt": closed,
        "active_attempt": active,
        "execution_id": selected.identity().execution(),
        "group_outcome": selected.outcome(),
        "group_terminal_attempt": selected.terminal_attempt(),
        "counters": combined_counters(state, group, selected.identity().execution()),
        "guidance": current_guidance(state, sources, active)?,
    });
    let result = result
        .as_object()
        .expect("boundary result is an object")
        .clone();
    // The private result is intentionally absent from the public result catalog.
    // Serialize the existing typed output fields without enrolling a new command.
    let mut output = serde_json::to_value(OutputEnvelopeInputV4 {
        request_id: request.request_id.clone(),
        command: CommandNameV1::new(identity.command())?,
        generated_at: Rfc3339MillisV1::from_unix_millis(now.get())?,
        workspace: Some(WorkspaceOutputV1::new(
            request.workspace.clone(),
            transaction.root().to_path_buf().display().to_string(),
            transaction.next_sequence()?,
        )?),
        session: Some(DirectSessionOutputV1 {
            id: request.cursor.session.clone(),
            title: state.base_definition().header().name().to_owned(),
            lifecycle: state.lifecycle(),
            revision_before: request.cursor.revision,
            revision_after: state.fence().revision,
        }),
        result,
        warnings: Vec::new(),
    })
    .map_err(|_| SuccessorExecutionError::ResponseBudget)?;
    output["schema"] = json!(OUTPUT_SCHEMA_V4);
    let canonical =
        canonicalize_json_v1(&output).map_err(|_| SuccessorExecutionError::ResponseBudget)?;
    let response = CanonicalExecutionJsonV1::new(canonical)
        .map_err(|_| SuccessorExecutionError::ResponseBudget)?;
    validate_direct_outcome_lookup_headroom_v1(&output)
        .map_err(|_| SuccessorExecutionError::OutcomeLookupBudget)?;
    transaction.persist(state, sources)?;
    Ok(CandidateSuccess { response, now })
}

fn combined_counters(
    state: &ExecutionSessionV3,
    group: &ExecutionGroupDefinitionV1,
    selected: &GroupExecutionIdV1,
) -> Vec<Value> {
    let mut base_counts = BTreeMap::new();
    let mut group_counts = BTreeMap::new();
    for attempt in state.attempt_history() {
        let target = attempt.target();
        let counts = match target.scope() {
            ExecutionScopeV1::Base => &mut base_counts,
            ExecutionScopeV1::Group {
                group_execution_id, ..
            } if group_execution_id == selected => &mut group_counts,
            ExecutionScopeV1::Group { .. } => continue,
        };
        let (attempts, ordinal) = counts.entry(target.node()).or_insert((0_usize, None));
        *attempts += 1;
        if attempt
            .selected_decision_entry()
            .is_none_or(|entry| entry.selected_group() == selected)
        {
            *ordinal = attempt.decision_entry();
        }
    }
    [
        ("base", state.base_definition().graph(), base_counts),
        ("group", group.graph(), group_counts),
    ].into_iter().flat_map(|(scope, graph, counts)| {
        graph.nodes().iter().map(move |node| {
            let (attempts, entry) = counts.get(node.id()).copied().unwrap_or_default();
            json!({"scope": scope, "node": node.id(), "attempt_count": attempts, "decision_entry": entry})
        })
    }).collect()
}

fn current_guidance(
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
    active: &ScopedAttemptV1,
) -> Result<Value> {
    let digest = match active.scope() {
        ExecutionScopeV1::Base => state.base_digest(),
        ExecutionScopeV1::Group {
            definition_digest, ..
        } => definition_digest,
    };
    let document: Value = serde_json::from_str(source(sources, digest)?).map_err(|_| {
        SuccessorExecutionError::Invalid("candidate guidance source is not canonical")
    })?;
    let node = document["graph"]["nodes"]
        .as_array()
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|node| node["id"] == active.node().as_str())
        })
        .ok_or(SuccessorExecutionError::Invalid(
            "candidate guidance placement is absent",
        ))?;
    let definition = node["use"]
        .as_str()
        .and_then(|id| document["node_definitions"].get(id))
        .ok_or(SuccessorExecutionError::Invalid(
            "candidate guidance definition is absent",
        ))?;
    Ok(json!({"placement": node, "definition": definition}))
}
