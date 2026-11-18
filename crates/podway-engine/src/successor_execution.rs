//! Typed Engine operations for isolated successor Store integration fixtures.

use crate::direct_execution::DirectIdSourceV1;
use podway_config::execution_groups::{
    SuccessorDefinition, ValidatedSuccessorDefinition, parse_successor_document,
    validate_group_interface, validate_successor_definition,
};
use podway_config::{ConfigError, ProcedureDocumentFormat};
use podway_core::execution_groups::*;
use podway_core::{
    ActionOutcomeV2, AttemptId, BoundUnitV2, CheckResultInputBasisV2, DomainError,
    GoalRevisionNumberV2, GraphMutationErrorV2, Revision, SessionId, Sha256Digest, UnixMillis,
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

mod current;
mod history;
mod mutations;
pub use current::*;
pub use history::*;
pub use mutations::*;

#[derive(Debug)]
pub enum SuccessorExecutionError {
    Config(ConfigError),
    Store(CandidateStoreError),
    Domain(DomainError),
    MissingAttempt {
        actual: AttemptId,
    },
    GoalRevisionMismatch {
        expected: Option<GoalRevisionNumberV2>,
        actual: Option<GoalRevisionNumberV2>,
    },
    SelectedActivationMismatch {
        expected: Option<GroupExecutionIdV1>,
        actual: Option<GroupExecutionIdV1>,
    },
    ItemMutation(GraphMutationErrorV2),
    Protocol(ProtocolError),
    Invalid(&'static str),
    InvalidHistoryPageLimit,
    InvalidHistoryPageToken,
    StaleHistoryPageToken,
    HistoryGroupNotFound,
    HistoryDefinitionMismatch,
    HistoryAttemptNotFound,
    HistoryItemNotFound,
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
impl From<GraphMutationErrorV2> for SuccessorExecutionError {
    fn from(error: GraphMutationErrorV2) -> Self {
        match error {
            GraphMutationErrorV2::Domain(error) => Self::Domain(error),
            error => Self::ItemMutation(error),
        }
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
    pub cursor: SuccessorCursor,
}

/// Base suffix attempts have a Base scope even while a specific group remains selected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuccessorCursor {
    pub execution: ExecutionCursorFenceV1,
    pub selected_group: Option<GroupExecutionIdV1>,
}

impl SuccessorCursor {
    pub fn from_state(state: &ExecutionSessionV3) -> Self {
        Self {
            execution: state.fence(),
            selected_group: selected_group_id(state).cloned(),
        }
    }
}

fn selected_group_id(state: &ExecutionSessionV3) -> Option<&GroupExecutionIdV1> {
    state.selected().map(|group| group.identity().execution())
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
        let (state, sources) = store.snapshot(
            &mutation.cursor.execution.session,
            admit_candidate_definition,
        )?;
        let base = admit_canonical_definition(source(&sources, state.base_digest())?)?;
        validate_group_interface(&base, &normalized)?;
        Ok(base.digest().clone())
    })();
    store.mutate(&mutation.workspace, &exact_request, |transaction| {
        let (mut state, mut sources) = transaction.load(admit_candidate_definition)?;
        verify_cursor(&state, &mutation.cursor)?;
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
            session: mutation.cursor.execution.session.clone(),
            base_digest: state.base_digest().clone(),
            definition_digest: request.expected_definition.clone(),
            anchor: anchor.target().node().clone(),
            goal_revision: mutation.cursor.execution.goal_revision,
            plan: request.plan,
            predecessor: request.predecessor,
            working_copy_basis: request.working_copy_basis,
            prior_work: request.prior_work,
        })?;
        let before = anchor.target().clone();
        current::verify_delegated_completion(transaction.workspace(), &state)?;
        let now = clock();
        state.activate(
            &mutation.cursor.execution,
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
        verify_cursor(&state, &request.cursor)?;
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
        current::verify_delegated_completion(transaction.workspace(), &state)?;
        let now = clock();
        state.complete(&request.cursor.execution, Some(ids.next_attempt_id()), now)?;
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

/// Close the current selection through an explicitly guarded ordinary Decision.
pub fn replan_group(
    store: &mut CandidateExecutionStore,
    request: SuccessorMutationRequest,
    feedback: ReplanFeedbackV1,
    clock: impl FnOnce() -> UnixMillis,
    ids: &impl DirectIdSourceV1,
) -> Result<DirectMutationResultV1> {
    let exact_request = outcome_request(
        &request,
        "group.replan",
        json!({
            "decision": feedback.decision(),
            "option": feedback.option(),
            "reason": feedback.reason(),
            "working_copy_basis": feedback.working_copy_basis(),
            "feedback": feedback.feedback(),
        }),
    )?;
    store.mutate(&request.workspace, &exact_request, |transaction| {
        let (mut state, sources) = transaction.load(admit_candidate_definition)?;
        verify_cursor(&state, &request.cursor)?;
        let decision = feedback.decision().clone();
        let now = clock();
        state.replan(
            &request.cursor.execution,
            feedback,
            ids.next_attempt_id(),
            now,
        )?;
        let active = active_target(&state)?;
        let (execution, closure, feedback) = state
            .group_history()
            .last()
            .and_then(|execution| {
                let closure = execution.closure()?;
                Some((execution, closure, closure.feedback.as_ref()?))
            })
            .filter(|(_, closure, _)| closure.source == decision)
            .ok_or(SuccessorExecutionError::Invalid(
                "candidate replan did not retain closure feedback",
            ))?;
        let record = state
            .attempt_history()
            .iter()
            .rev()
            .nth(1)
            .filter(|attempt| attempt.target() == &decision)
            .and_then(GroupAttemptStateV1::decision)
            .ok_or(SuccessorExecutionError::Invalid(
                "candidate replan did not retain the decision",
            ))?;
        let result = json!({
            "schema": "podway.successor-group-replan-fixture-result/v1",
            "revision_before": request.cursor.execution.revision,
            "revision": state.fence().revision,
            "closed_attempt": decision,
            "active_attempt": active,
            "execution_id": execution.identity().execution(),
            "group_outcome": execution.outcome(),
            "group_terminal_attempt": execution.terminal_attempt(),
            "decision": {
                "option": record.option,
                "reason": record.reason,
                "guard_values": record.guard_values.iter().map(|guard| json!({
                    "source": guard.source,
                    "item": guard.item.item(),
                    "revision": guard.item.revision(),
                    "value_digest": guard.item.value_digest(),
                })).collect::<Vec<_>>(),
            },
            "closure": {
                "cause": closure.cause,
                "source": closure.source,
                "at_unix_ms": closure.at.get(),
                "working_copy_basis": feedback.working_copy_basis(),
                "feedback": feedback.feedback(),
            },
            "counters": combined_counters(&state, None),
            "guidance": current_guidance(&state, &sources, active)?,
        });
        persist_transition(
            transaction,
            &exact_request,
            &state,
            &sources,
            request.cursor.execution.revision,
            result,
            now,
        )
    })
}

fn verify_cursor(state: &ExecutionSessionV3, cursor: &SuccessorCursor) -> Result<()> {
    state
        .verify_cursor(&cursor.execution)
        .map_err(|error| match error {
            ExecutionCursorMismatchV1::Domain(error) => SuccessorExecutionError::Domain(error),
            ExecutionCursorMismatchV1::MissingAttempt { actual } => {
                SuccessorExecutionError::MissingAttempt { actual }
            }
            ExecutionCursorMismatchV1::GoalRevision { expected, actual } => {
                SuccessorExecutionError::GoalRevisionMismatch { expected, actual }
            }
        })?;
    verify_selection(state, cursor.selected_group.as_ref())
}

fn verify_selection(
    state: &ExecutionSessionV3,
    expected: Option<&GroupExecutionIdV1>,
) -> Result<()> {
    let actual = selected_group_id(state);
    if actual != expected {
        return Err(SuccessorExecutionError::SelectedActivationMismatch {
            expected: expected.cloned(),
            actual: actual.cloned(),
        });
    }
    Ok(())
}

fn outcome_request(
    request: &SuccessorMutationRequest,
    command: &str,
    body: Value,
) -> Result<DirectMutationRequestV1> {
    MutationIdentity {
        workspace: &request.workspace,
        key: &request.key,
        request_id: &request.request_id,
        session: &request.cursor.execution.session,
    }
    .prepare(command, cursor_json(&request.cursor), body)
}

fn cursor_json(cursor: &SuccessorCursor) -> Value {
    json!({
        "session": cursor.execution.session,
        "session_revision": cursor.execution.revision,
        "attempt": cursor.execution.attempt,
        "goal_revision": cursor.execution.goal_revision.map(|revision| revision.get()),
        "selected_group": cursor.selected_group,
    })
}

struct MutationIdentity<'a> {
    workspace: &'a WorkspaceId,
    key: &'a IdempotencyKeyV1,
    request_id: &'a RequestIdV1,
    session: &'a SessionId,
}

impl MutationIdentity<'_> {
    fn prepare(
        &self,
        command: &str,
        fences: Value,
        body: Value,
    ) -> Result<DirectMutationRequestV1> {
        let identity = canonicalize_json_v1(&json!({
            "command": command,
            "workspace": self.workspace,
            "session": self.session,
            "fences": fences,
            "body": body,
        }))
        .map_err(|_| {
            SuccessorExecutionError::Invalid("candidate request identity is not canonical")
        })?;
        if identity.len() > MAX_DIRECT_INPUT_BYTES_V1 {
            return Err(SuccessorExecutionError::Invalid(
                "candidate request exceeds the direct input bound",
            ));
        }
        Ok(DirectMutationRequestV1::new(
            self.key.clone(),
            canonical_digest(&identity)?,
            command,
            self.request_id.as_str(),
            DirectOutcomeScopeV1::Session(self.session.clone()),
        )?)
    }
}

fn canonical_digest(canonical: &str) -> Result<Sha256Digest> {
    Ok(Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(canonical.as_bytes())
    ))?)
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
    let active = active_target(state)?;
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
        "revision_before": request.cursor.execution.revision,
        "revision": state.fence().revision,
        "closed_attempt": closed,
        "active_attempt": active,
        "execution_id": selected.identity().execution(),
        "group_outcome": selected.outcome(),
        "group_terminal_attempt": selected.terminal_attempt(),
        "counters": combined_counters(state, Some((group, selected.identity().execution()))),
        "guidance": current_guidance(state, sources, active)?,
    });
    persist_transition(
        transaction,
        identity,
        state,
        sources,
        request.cursor.execution.revision,
        result,
        now,
    )
}

fn active_target(state: &ExecutionSessionV3) -> Result<&ScopedAttemptV1> {
    state
        .active_attempt()
        .map(GroupAttemptStateV1::target)
        .ok_or(SuccessorExecutionError::Invalid(
            "candidate transition did not produce an active successor",
        ))
}

fn persist_transition(
    transaction: &CandidateExecutionTransaction<'_, '_>,
    identity: &DirectMutationRequestV1,
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
    revision_before: Revision,
    result: Value,
    now: UnixMillis,
) -> Result<CandidateSuccess> {
    let Value::Object(result) = result else {
        unreachable!("candidate transition results are objects");
    };
    // The private result is intentionally absent from the public result catalog.
    // Serialize the existing typed output fields without enrolling a new command.
    let mut output = serde_json::to_value(OutputEnvelopeInputV4 {
        request_id: RequestIdV1::new(identity.response_id())?,
        command: CommandNameV1::new(identity.command())?,
        generated_at: Rfc3339MillisV1::from_unix_millis(now.get())?,
        workspace: Some(WorkspaceOutputV1::new(
            transaction.workspace().clone(),
            transaction.root().to_path_buf().display().to_string(),
            transaction.next_sequence()?,
        )?),
        session: Some(DirectSessionOutputV1 {
            id: state.fence().session,
            title: state.base_definition().header().name().to_owned(),
            lifecycle: state.lifecycle(),
            revision_before,
            revision_after: state.fence().revision,
        }),
        result,
        warnings: Vec::new(),
    })
    .map_err(|_| SuccessorExecutionError::ResponseBudget)?;
    output["schema"] = json!(OUTPUT_SCHEMA_V4);
    let response = canonical_response(&output)?;
    validate_direct_outcome_lookup_headroom_v1(&output)
        .map_err(|_| SuccessorExecutionError::OutcomeLookupBudget)?;
    if state.fence().revision != revision_before {
        transaction.persist(state, sources)?;
    }
    Ok(CandidateSuccess { response, now })
}

fn canonical_response(value: &Value) -> Result<CanonicalExecutionJsonV1> {
    let canonical =
        canonicalize_json_v1(value).map_err(|_| SuccessorExecutionError::ResponseBudget)?;
    CanonicalExecutionJsonV1::new(canonical).map_err(|_| SuccessorExecutionError::ResponseBudget)
}

fn combined_counters(
    state: &ExecutionSessionV3,
    selection: Option<(&ExecutionGroupDefinitionV1, &GroupExecutionIdV1)>,
) -> Vec<Value> {
    let mut base_counts = BTreeMap::new();
    let mut group_counts = BTreeMap::new();
    for attempt in state.attempt_history() {
        let target = attempt.target();
        let counts = match target.scope() {
            ExecutionScopeV1::Base => &mut base_counts,
            ExecutionScopeV1::Group {
                group_execution_id, ..
            } if selection.is_some_and(|(_, selected)| group_execution_id == selected) => {
                &mut group_counts
            }
            ExecutionScopeV1::Group { .. } => continue,
        };
        let (attempts, ordinal) = counts.entry(target.node()).or_insert((0_usize, None));
        *attempts += 1;
        if attempt.selected_decision_entry().is_none_or(|entry| {
            selection.is_some_and(|(_, selected)| entry.selected_group() == selected)
        }) {
            *ordinal = attempt.decision_entry();
        }
    }
    let base = ("base", state.base_definition().graph(), base_counts);
    let group = selection.map(|(group, _)| ("group", group.graph(), group_counts));
    std::iter::once(base)
        .chain(group)
        .flat_map(|(scope, graph, counts)| {
            graph.nodes().iter().map(move |node| {
                let (attempts, entry) = counts.get(node.id()).copied().unwrap_or_default();
                json!({
                    "scope": scope, "node": node.id(),
                    "attempt_count": attempts, "decision_entry": entry,
                })
            })
        })
        .collect()
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
