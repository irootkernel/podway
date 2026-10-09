//! Procedure, goal and disposition mutations use the same direct transaction boundary.

use std::time::Instant;

use podway_core::{
    ActorAttributionV2, CriterionAssessmentReasonV2, CriterionAssessmentResultV2,
    CriterionCitationV2, CriterionStatusV2, DomainError, GoalRevisionNumberV2,
    GoalRevisionReasonV2, GoalStatementV2, ReasonV2, SessionLifecycle, Sha256Digest,
    TerminalDispositionV2, WorkspaceId,
};
use podway_git::{GitResolverContractV1, WorktreeSelectorV1};
use podway_protocol::{
    OutputEnvelopeV4, ProcedureV2MutationCommandV1 as C, RequestIdV1, Rfc3339MillisV1,
    TerminalDispositionInputV1, canonical_procedure_v2_command_identity_v1,
};
use podway_store::{
    GraphMutationErrorV2, GraphSessionStateV2, IdempotencyKeyV1,
    direct_store::{DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1},
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::{
    direct_execution::{
        DirectBeginRequestV1, DirectExecutionErrorV1, DirectIdSourceV1, begin_session_v1,
    },
    direct_mutation::{check_attempt_fence, check_session_fences, graph_success},
    execution::{ExecutionClockV1, goal_definition_from_wire_v2},
    v2_read_service::decision_readback_projection,
    workspace::DirectWorkspaceResolverV1,
};

#[derive(Clone, Debug)]
pub struct DirectProcedureRequestV1 {
    pub selector: WorktreeSelectorV1,
    pub expected_workspace_id: Option<WorkspaceId>,
    pub request_id: RequestIdV1,
    pub key: IdempotencyKeyV1,
    pub command: C,
    pub deadline: Instant,
}

pub fn execute_procedure_v1<R: GitResolverContractV1>(
    resolver: &DirectWorkspaceResolverV1<R>,
    request: DirectProcedureRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> Result<OutputEnvelopeV4, DirectExecutionErrorV1> {
    if let C::SessionBegin(input) = request.command {
        return begin_session_v1(
            resolver,
            DirectBeginRequestV1 {
                selector: request.selector,
                expected_workspace_id: request.expected_workspace_id,
                request_id: request.request_id,
                key: request.key,
                input,
                deadline: request.deadline,
            },
            clock,
            ids,
        );
    }
    let (resolved, store, binding) = resolver.resolve_existing_with_binding(
        request.selector,
        request.expected_workspace_id.as_ref(),
        request.deadline,
    )?;
    let command = &request.command;
    let canonical = canonical_procedure_v2_command_identity_v1(
        command,
        resolved.store_identity().workspace_uuid(),
    )
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "invalid procedure semantic identity",
    })?;
    let digest = Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
        .expect("SHA-256 produces a canonical digest");
    let (session, revision, attempt, goal_revision) = match command {
        C::SessionTerminalDisposition(input) => (
            &input.preconditions.expected_session_id,
            input.preconditions.expected_session_revision,
            None,
            None,
        ),
        C::GoalDefine(input) => (
            &input.preconditions.expected_session_id,
            input.preconditions.expected_session_revision,
            None,
            None,
        ),
        C::SessionRework(input) => (
            &input.preconditions.expected_session_id,
            input.preconditions.expected_session_revision,
            input.preconditions.expected_attempt_id.as_ref(),
            None,
        ),
        C::GoalRevise(input) => (
            &input.preconditions.expected_session_id,
            input.preconditions.expected_session_revision,
            input.preconditions.expected_attempt_id.as_ref(),
            Some(input.preconditions.expected_goal_revision),
        ),
        C::SessionDecide(input) => (
            &input.preconditions.expected_session_id,
            input.preconditions.expected_session_revision,
            Some(&input.preconditions.expected_attempt_id),
            input.expected_goal_revision,
        ),
        C::GoalAssessCriterion(input) => (
            &input.preconditions.expected_session_id,
            input.preconditions.expected_session_revision,
            Some(&input.preconditions.expected_attempt_id),
            Some(input.expected_goal_revision),
        ),
        C::SessionBegin(_) => unreachable!("begin was dispatched above"),
    };
    let mutation = DirectMutationRequestV1::new(
        request.key,
        digest,
        command.command_name(),
        request.request_id.as_str(),
        DirectOutcomeScopeV1::Session(session.clone()),
    )?;
    let prepared = store.prepare_graph(request.deadline)?;
    let native = resolver.native_workspace(resolved.clone());
    let artifacts = if matches!(command, C::SessionDecide(_)) {
        prepared
            .as_ref()
            .map(|prior| native.prepare_required_artifacts_v2(&binding, prior.graph()))
            .transpose()
            .map(|artifacts| artifacts.unwrap_or_default())
    } else {
        Ok(Vec::new())
    };
    let assessment = if let C::GoalAssessCriterion(input) = command {
        Some(
            (|| -> Result<CriterionAssessmentResultV2, DirectStoreErrorV1> {
                if input.evidence.len().saturating_add(input.items.len())
                    > podway_core::MAX_CRITERION_CITATIONS_V2
                {
                    return Err(GraphMutationErrorV2::Domain(DomainError::InvalidState {
                        reason: "a criterion result carries at most four citations",
                    })
                    .into());
                }
                let status = input.status.parse::<CriterionStatusV2>().map_err(|_| {
                    DirectStoreErrorV1::InvalidInput {
                        reason: "invalid criterion status",
                    }
                })?;
                if input
                    .evidence
                    .iter()
                    .enumerate()
                    .any(|(index, value)| input.evidence[..index].contains(value))
                    || input
                        .items
                        .iter()
                        .enumerate()
                        .any(|(index, value)| input.items[..index].contains(value))
                {
                    return Err(DirectStoreErrorV1::InvalidInput {
                        reason: "duplicate criterion citation",
                    });
                }
                CriterionAssessmentResultV2::new(
                    input.criterion_id.clone(),
                    status,
                    CriterionAssessmentReasonV2::new(input.reason.clone())
                        .map_err(GraphMutationErrorV2::Domain)?,
                    input
                        .evidence
                        .iter()
                        .cloned()
                        .map(CriterionCitationV2::Evidence)
                        .chain(input.items.iter().cloned().map(CriterionCitationV2::Item))
                        .collect(),
                )
                .map_err(|error| DirectStoreErrorV1::Domain(GraphMutationErrorV2::Domain(error)))
            })(),
        )
    } else {
        None
    };
    let mut execution_error = None;
    let result = store.mutate(&mutation, request.deadline, || clock.now(), |transaction| {
        let current = transaction.use_prepared_graph(prepared.as_ref())?;
        let current = current.as_ref().ok_or_else(|| GraphMutationErrorV2::Domain(DomainError::SessionIdentityMismatch {
            expected: session.clone(), actual: None,
        }))?;
        let prior = current.as_ref();
        check_session_fences(prior, session, revision)?;
        if matches!(command, C::SessionRework(_)) && attempt.is_some()
            && prior.trace().active_attempt().is_none() {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "terminal rework must not carry an attempt fence",
            });
        }
        if let Some(attempt) = attempt { check_attempt_fence(prior, attempt)?; }
        if matches!(command, C::SessionRework(_) | C::GoalRevise(_))
            && attempt.is_none() && prior.trace().active_attempt().is_some() {
            return Err(DirectStoreErrorV1::InvalidInput { reason: "running rework requires an attempt fence" });
        }
        if matches!(command, C::GoalRevise(_) | C::GoalAssessCriterion(_))
            && !prior.snapshot().goal_tracking() {
            return Err(GraphMutationErrorV2::GoalTrackingNotEnabled.into());
        }
        if matches!(command, C::SessionDecide(_)) && goal_revision.is_none()
            && prior.trace().active_attempt().is_some_and(|active| {
                prior.snapshot().graph_nodes().iter().any(|node|
                    node.graph_node_id() == active.graph_node_id() && node.goal_assessment())
            }) {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "goal-assessment decisions require a goal revision precondition",
            });
        }
        if let Some(expected) = goal_revision {
            let actual = prior.goal_state().current_revision().ok_or(GraphMutationErrorV2::SessionGoalMissing)?;
            if actual.get() != expected {
                return Err(GraphMutationErrorV2::GoalRevisionStale { expected: GoalRevisionNumberV2::new(expected), actual }.into());
            }
        }
        resolver.revalidate(&resolved, transaction.binding()).map_err(|error| {
            execution_error = Some(DirectExecutionErrorV1::from(error));
            DirectStoreErrorV1::InvalidInput { reason: "workspace revalidation failed" }
        })?;
        if matches!(command, C::SessionDecide(_)) {
            let artifacts = artifacts.map_err(|error| {
                execution_error = Some(error);
                DirectStoreErrorV1::InvalidInput { reason: "required artifact preparation failed" }
            })?;
            if let Err(error) = native.verify_required_artifacts_v2(transaction.binding(), prior, &artifacts) {
                execution_error = Some(error);
                return Err(DirectStoreErrorV1::InvalidInput { reason: "required artifact changed" });
            }
        }
        let (next, body, disposition) = match command {
            C::GoalDefine(input) => {
                let statement = GoalStatementV2::new(input.goal.clone()).map_err(GraphMutationErrorV2::Domain)?;
                let criteria = goal_definition_from_wire_v2(&input.criteria).map_err(GraphMutationErrorV2::Domain)?;
                let actor = actor(&input.actor)?;
                let outcome = prior.define_goal_v2(revision, statement, criteria, actor, transaction.now())?;
                let mut body = project_goal_revision(outcome.revision())?;
                body["schema"] = json!("podway.goal-definition-result/v2");
                (outcome.into_state(), body, None)
            }
            C::GoalRevise(input) => {
                let statement = GoalStatementV2::new(input.goal.clone()).map_err(GraphMutationErrorV2::Domain)?;
                let criteria = goal_definition_from_wire_v2(&input.criteria).map_err(GraphMutationErrorV2::Domain)?;
                let reason = GoalRevisionReasonV2::new(input.reason.clone()).map_err(GraphMutationErrorV2::Domain)?;
                let actor = actor(&input.actor)?;
                let fresh = ids.next_attempt_id();
                let outcome = prior.revise_goal_v2(revision, attempt, GoalRevisionNumberV2::new(input.preconditions.expected_goal_revision),
                    statement, criteria, input.target_graph_node_id.clone(), fresh, reason, actor, input.reactivate, transaction.now())?;
                let mut body = project_goal_revision(outcome.revision())?;
                body["schema"] = json!("podway.goal-revision-result/v2");
                (outcome.into_state(), body, None)
            }
            C::GoalAssessCriterion(input) => {
                let result = assessment.expect("criterion assessment is prepared outside the writer")?;
                let actor = actor(&input.actor)?;
                let outcome = prior.assess_goal_criterion_v2(revision, &input.preconditions.expected_attempt_id,
                    GoalRevisionNumberV2::new(input.expected_goal_revision), result, actor, transaction.now())?;
                let result = outcome.assessment().result();
                let mut body = json!({"schema":"podway.criterion-assessment-result/v2", "graph_node_id":outcome.graph_node_id(),
                    "attempt_id":outcome.attempt_id(), "goal_revision":outcome.goal_revision().get(), "mode":result.mode().as_str(),
                    "result":{"criterion_id":result.criterion_id(), "status":result.status().as_str(), "reason":result.reason().as_str(),
                        "citations":result.citations().iter().map(|citation| match citation {
                            CriterionCitationV2::Evidence(node) => json!({"reference_graph_node_id":node}),
                            CriterionCitationV2::Item(item) => json!({"local_item_id":item}),
                        }).collect::<Vec<_>>()}, "complete":outcome.complete()});
                if let Some(determined) = outcome.determined_outcome() { body["determined_outcome"] = json!(determined.as_str()); }
                (outcome.into_state(), body, None)
            }
            C::SessionRework(input) => {
                let reason = ReasonV2::new(input.reason.clone()).map_err(GraphMutationErrorV2::Domain)?;
                let actor = actor(&input.actor)?;
                let outcome = prior.manual_rework_v2(revision, attempt, input.target_graph_node_id.clone(), ids.next_attempt_id(),
                    reason, actor, transaction.now())?;
                let record = outcome.record();
                let body = json!({"schema":"podway.rework-result/v2", "from_graph_node_id":record.from_node(),
                    "to_graph_node_id":record.to_node(), "target_attempt_id":record.target_attempt_id(),
                    "reason":record.reason().as_str(), "reactivated":record.reactivated()});
                (outcome.into_state(), body, None)
            }
            C::SessionDecide(input) => {
                let reason = ReasonV2::new(input.reason.clone()).map_err(GraphMutationErrorV2::Domain)?;
                let actor = actor(&input.actor)?;
                let outcome = prior.decide_active_route_with_goal_revision_v2(revision, &input.preconditions.expected_attempt_id,
                    input.option_id.clone(), ids.next_attempt_id(), input.expected_goal_revision.map(GoalRevisionNumberV2::new),
                    Some(reason), actor, transaction.now())?;
                let record = outcome.decision_record();
                let record_value = decision_readback_projection(outcome.state(), record).map_err(|_| DirectStoreErrorV1::Unreadable)?;
                let body = json!({"schema":"podway.decision-result/v2", "graph_node_id":record.graph_node_id(),
                    "attempt_id":record.attempt_id(), "attempt_number":record.attempt_number().get(), "option_id":record.selected_option(),
                    "effect":record.route_effect().as_str(), "target_graph_node_id":record.route_target(), "target_attempt_id":outcome.to_attempt_id(),
                    "session_state":"running", "record":record_value});
                (outcome.into_state(), body, None)
            }
            C::SessionTerminalDisposition(input) => {
                if !matches!(prior.trace().lifecycle(), SessionLifecycle::Completed | SessionLifecycle::Cancelled) {
                    execution_error = Some(DirectExecutionErrorV1::SessionNotTerminal);
                    return Err(DirectStoreErrorV1::InvalidInput { reason: "disposition requires a terminal session" });
                }
                let actor = actor(&input.actor)?;
                // Validate bounded caller text before obtaining the mutation timestamp.
                crate::direct_mutation::validate_disposition_input(&input.disposition)?;
                let now = transaction.now();
                let disposition = make_disposition(prior, &input.disposition, actor, now)?;
                let mut public = json!({"session_revision":revision, "recorded_at":timestamp(now)?});
                match &input.disposition {
                    TerminalDispositionInputV1::HandedOff { summary, reference } => {
                        public["kind"] = json!("handed_off"); public["summary"] = json!(summary); public["reference"] = json!(reference);
                    }
                    TerminalDispositionInputV1::NotRequired { reason } => {
                        public["kind"] = json!("not_required"); public["reason"] = json!(reason);
                    }
                }
                if let Some(actor) = &input.actor { public["actor"] = json!(actor); }
                let body = json!({"schema":"podway.terminal-disposition-result/v2", "session_id":session,
                    "session_revision":revision, "disposition":public});
                (prior.clone(), body, Some(disposition))
            }
            C::SessionBegin(_) => unreachable!("begin was dispatched above"),
        };
        let mut body = body.as_object().expect("procedure result is an object").clone();
        if disposition.is_none() { body.insert("revision".into(), json!(next.trace().revision())); }
        let response = graph_success(transaction, &resolved, request.request_id, command.command_name(), prior, &next, body)?;
        if let Some(disposition) = disposition { transaction.record_disposition(&disposition)?; }
        else { transaction.replace_graph(prior.workspace_revision(), prior.trace().revision(), &next)?; }
        Ok(response)
    });
    if let Some(error) = execution_error {
        return Err(error);
    }
    let result = result?;
    serde_json::from_str(result.outcome().response().as_str())
        .map_err(|_| DirectStoreErrorV1::Unreadable.into())
}

fn actor(value: &Option<String>) -> Result<Option<ActorAttributionV2>, DirectStoreErrorV1> {
    Ok(value
        .clone()
        .map(ActorAttributionV2::new)
        .transpose()
        .map_err(GraphMutationErrorV2::Domain)?)
}

fn timestamp(now: podway_core::UnixMillis) -> Result<Rfc3339MillisV1, DirectStoreErrorV1> {
    Rfc3339MillisV1::from_unix_millis(now.get()).map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "clock exceeds the public timestamp range",
    })
}

fn project_goal_revision(
    record: &podway_core::GoalRevisionRecordV2,
) -> Result<Value, DirectStoreErrorV1> {
    let mut value = json!({"goal_revision":record.revision().get(), "statement":record.statement().as_str(),
        "criteria":record.criteria().criteria().iter().map(|criterion| json!({"criterion_id":criterion.id(),
            "statement":criterion.statement()})).collect::<Vec<_>>(), "recorded_at":timestamp(record.created_at())?});
    if let Some(reason) = record.reason() {
        value["reason"] = json!(reason.as_str());
    }
    if let Some(target) = record.rework_to() {
        value["rework_to"] = json!(target);
        value["reactivated"] = json!(record.reactivated());
    }
    if let Some(actor) = record.actor() {
        value["actor"] = json!(actor.as_str());
    }
    Ok(value)
}

fn make_disposition(
    graph: &GraphSessionStateV2,
    input: &TerminalDispositionInputV1,
    actor: Option<ActorAttributionV2>,
    now: podway_core::UnixMillis,
) -> Result<TerminalDispositionV2, DirectStoreErrorV1> {
    let disposition = match input {
        TerminalDispositionInputV1::HandedOff { summary, reference } => {
            TerminalDispositionV2::handed_off(
                graph.trace().session_id().clone(),
                graph.trace().revision(),
                summary,
                reference,
                actor,
                now,
            )
        }
        TerminalDispositionInputV1::NotRequired { reason } => TerminalDispositionV2::not_required(
            graph.trace().session_id().clone(),
            graph.trace().revision(),
            reason,
            actor,
            now,
        ),
    };
    Ok(disposition.map_err(GraphMutationErrorV2::Domain)?)
}
