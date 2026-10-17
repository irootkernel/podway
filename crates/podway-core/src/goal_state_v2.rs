//! Pure session goals, assessments and immutable goal-history invariants.
use crate::ProcedureStateErrorV2;
use crate::graph_v2::{AttemptMetadataV2, ProcedureSnapshotV2};
use crate::workflow_v2::{WorkflowGoalTransitionV2, WorkflowMemoryStateV2};
use crate::{
    ActorAttributionV2, AttemptId, AttemptLifecycle, AttemptValidityV2,
    CriterionAssessmentResultV2, CriterionCitationV2, GoalAssessmentRecordV2, GoalOutcome,
    GoalRevisionNumberV2, GoalRevisionReasonV2, GoalRevisionRecordV2, GraphNodeId, OptionId,
    SessionLifecycle, SessionTraceV2, UnixMillis,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
fn invalid(reason: &'static str) -> ProcedureStateErrorV2 {
    ProcedureStateErrorV2::new(reason)
}
/// One persisted criterion result plus row-owned attribution and timestamp metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CriterionAssessmentStateV2 {
    result: CriterionAssessmentResultV2,
    actor: Option<ActorAttributionV2>,
    recorded_at: UnixMillis,
}

impl CriterionAssessmentStateV2 {
    pub fn new(
        result: CriterionAssessmentResultV2,
        actor: Option<ActorAttributionV2>,
        recorded_at: UnixMillis,
    ) -> Self {
        Self {
            result,
            actor,
            recorded_at,
        }
    }

    pub fn result(&self) -> &CriterionAssessmentResultV2 {
        &self.result
    }
    pub fn actor(&self) -> Option<&ActorAttributionV2> {
        self.actor.as_ref()
    }
    pub const fn recorded_at(&self) -> UnixMillis {
        self.recorded_at
    }
}

/// Attempt-local criterion state, retained after retry or staleness for read-back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptCriterionAssessmentStateV2 {
    attempt_id: AttemptId,
    goal_revision: GoalRevisionNumberV2,
    results: Vec<CriterionAssessmentStateV2>,
}

impl AttemptCriterionAssessmentStateV2 {
    pub fn new(
        attempt_id: AttemptId,
        goal_revision: GoalRevisionNumberV2,
        results: Vec<CriterionAssessmentStateV2>,
    ) -> Result<Self, ProcedureStateErrorV2> {
        if goal_revision < GoalRevisionNumberV2::FIRST
            || results.is_empty()
            || results
                .iter()
                .map(|state| state.result().criterion_id())
                .collect::<BTreeSet<_>>()
                .len()
                != results.len()
        {
            return Err(invalid(
                "Procedure v2 criterion assessment identity is invalid",
            ));
        }
        if let Some(first) = results.first()
            && results
                .iter()
                .any(|state| state.result().mode() != first.result().mode())
        {
            return Err(invalid("Procedure v2 criterion assessment modes are mixed"));
        }
        Ok(Self {
            attempt_id,
            goal_revision,
            results,
        })
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }
    pub const fn goal_revision(&self) -> GoalRevisionNumberV2 {
        self.goal_revision
    }
    pub fn results(&self) -> &[CriterionAssessmentStateV2] {
        &self.results
    }
}

/// Complete optional session-goal state and immutable assessment history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalStateV2 {
    current_revision: Option<GoalRevisionNumberV2>,
    revisions: Vec<GoalRevisionRecordV2>,
    attempt_assessments: Vec<AttemptCriterionAssessmentStateV2>,
    assessments: Vec<GoalAssessmentRecordV2>,
}

impl GoalStateV2 {
    pub fn empty() -> Self {
        Self {
            current_revision: None,
            revisions: Vec::new(),
            attempt_assessments: Vec::new(),
            assessments: Vec::new(),
        }
    }

    pub fn new(
        current_revision: Option<GoalRevisionNumberV2>,
        revisions: Vec<GoalRevisionRecordV2>,
        attempt_assessments: Vec<AttemptCriterionAssessmentStateV2>,
        assessments: Vec<GoalAssessmentRecordV2>,
    ) -> Result<Self, ProcedureStateErrorV2> {
        if revisions
            .iter()
            .map(GoalRevisionRecordV2::revision)
            .collect::<BTreeSet<_>>()
            .len()
            != revisions.len()
            || attempt_assessments
                .iter()
                .map(AttemptCriterionAssessmentStateV2::attempt_id)
                .collect::<BTreeSet<_>>()
                .len()
                != attempt_assessments.len()
            || assessments
                .iter()
                .map(GoalAssessmentRecordV2::decision_attempt_id)
                .collect::<BTreeSet<_>>()
                .len()
                != assessments.len()
        {
            return Err(invalid(
                "Procedure v2 goal history identities must be unique",
            ));
        }
        if assessments
            .windows(2)
            .any(|pair| pair[0].decision_trace() >= pair[1].decision_trace())
        {
            return Err(invalid(
                "Procedure v2 goal assessments are not in trace order",
            ));
        }
        Ok(Self {
            current_revision,
            revisions,
            attempt_assessments,
            assessments,
        })
    }

    pub const fn current_revision(&self) -> Option<GoalRevisionNumberV2> {
        self.current_revision
    }
    pub fn revisions(&self) -> &[GoalRevisionRecordV2] {
        &self.revisions
    }
    pub fn attempt_assessments(&self) -> &[AttemptCriterionAssessmentStateV2] {
        &self.attempt_assessments
    }
    pub fn assessments(&self) -> &[GoalAssessmentRecordV2] {
        &self.assessments
    }

    pub fn latest_fresh_assessment<'a>(
        &'a self,
        trace: &SessionTraceV2,
    ) -> Option<&'a GoalAssessmentRecordV2> {
        let current = self.current_revision?;
        self.assessments.iter().rev().find(|assessment| {
            assessment.goal_revision() == current
                && trace.attempts().iter().any(|attempt| {
                    attempt.attempt_id() == assessment.decision_attempt_id()
                        && attempt.validity() == AttemptValidityV2::Valid
                })
        })
    }

    pub(crate) fn goal_rework_target_attempt_ids(
        &self,
        trace: &SessionTraceV2,
    ) -> BTreeSet<AttemptId> {
        self.revisions
            .iter()
            .filter(|revision| revision.revision() > GoalRevisionNumberV2::FIRST)
            .filter_map(|revision| {
                trace
                    .attempts()
                    .iter()
                    .find(|attempt| attempt.trace() == revision.binding_trace())
            })
            .map(|attempt| attempt.attempt_id().clone())
            .collect()
    }

    pub(crate) fn reactivated_goal_rework_target_attempt_ids(
        &self,
        trace: &SessionTraceV2,
    ) -> BTreeSet<AttemptId> {
        self.revisions
            .iter()
            .filter(|revision| revision.revision() > GoalRevisionNumberV2::FIRST)
            .filter(|revision| revision.reactivated())
            .filter_map(|revision| {
                trace
                    .attempts()
                    .iter()
                    .find(|attempt| attempt.trace() == revision.binding_trace())
            })
            .map(|attempt| attempt.attempt_id().clone())
            .collect()
    }
}

impl Default for GoalStateV2 {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Clone, Debug)]
struct GoalSnapshotModelV2 {
    outcome_maps: BTreeMap<GraphNodeId, BTreeMap<OptionId, GoalOutcome>>,
    manual_targets: BTreeSet<GraphNodeId>,
    dominance_edges: BTreeMap<GraphNodeId, Vec<GraphNodeId>>,
    terminal_nodes: BTreeSet<GraphNodeId>,
}

impl GoalSnapshotModelV2 {
    fn parse(snapshot: &ProcedureSnapshotV2) -> Result<Self, ProcedureStateErrorV2> {
        let document: Value = serde_json::from_str(snapshot.canonical_json().as_str())
            .map_err(|_| invalid("Procedure v2 goal snapshot metadata is invalid"))?;
        let root = document
            .as_object()
            .ok_or_else(|| invalid("Procedure v2 goal snapshot metadata is invalid"))?;
        let definitions = root
            .get("node_definitions")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("Procedure v2 goal definitions are absent"))?;
        let mut outcome_maps = BTreeMap::new();
        let mut dominance_edges = BTreeMap::new();
        let mut terminal_nodes = BTreeSet::new();
        for node in snapshot
            .graph_nodes()
            .iter()
            .filter(|node| node.goal_assessment())
        {
            let definition = definitions
                .get(node.node_definition_id().as_str())
                .and_then(Value::as_object)
                .ok_or_else(|| invalid("Procedure v2 goal assessment definition is absent"))?;
            let outcomes = definition
                .get("assessment")
                .and_then(Value::as_object)
                .and_then(|assessment| assessment.get("outcomes"))
                .and_then(Value::as_object)
                .ok_or_else(|| invalid("Procedure v2 goal assessment outcomes are absent"))?;
            let values = outcomes
                .iter()
                .map(|(option, outcome)| {
                    Ok((
                        OptionId::new(option.clone())
                            .map_err(|_| invalid("Procedure v2 assessment option is invalid"))?,
                        GoalOutcome::from_str(outcome.as_str().ok_or_else(|| {
                            invalid("Procedure v2 assessment outcome is invalid")
                        })?)
                        .map_err(|_| invalid("Procedure v2 assessment outcome is invalid"))?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, ProcedureStateErrorV2>>()?;
            outcome_maps.insert(node.graph_node_id().clone(), values);
        }
        for node in snapshot.graph_nodes() {
            let placement: Value = serde_json::from_str(node.canonical_placement_json())
                .map_err(|_| invalid("Procedure v2 goal placement metadata is invalid"))?;
            let placement = placement
                .as_object()
                .ok_or_else(|| invalid("Procedure v2 goal placement metadata is invalid"))?;
            if placement.get("terminal").and_then(Value::as_bool) == Some(true) {
                terminal_nodes.insert(node.graph_node_id().clone());
            }
            let mut successors = Vec::new();
            if let Some(target) = placement.get("next").and_then(Value::as_str) {
                successors.push(
                    GraphNodeId::new(target.to_owned())
                        .map_err(|_| invalid("Procedure v2 goal advance target is invalid"))?,
                );
            }
            if let Some(routes) = placement.get("routes").and_then(Value::as_object) {
                for route in routes.values() {
                    let route = route
                        .as_object()
                        .ok_or_else(|| invalid("Procedure v2 goal route metadata is invalid"))?;
                    successors.push(
                        GraphNodeId::new(
                            route
                                .get("to")
                                .and_then(Value::as_str)
                                .ok_or_else(|| {
                                    invalid("Procedure v2 goal route target is invalid")
                                })?
                                .to_owned(),
                        )
                        .map_err(|_| invalid("Procedure v2 goal route target is invalid"))?,
                    );
                }
            }
            dominance_edges.insert(node.graph_node_id().clone(), successors);
        }
        let manual_targets = root
            .get("manual_rework")
            .and_then(Value::as_object)
            .and_then(|value| value.get("allowed_targets"))
            .and_then(Value::as_array)
            .map_or(Ok(BTreeSet::new()), |values| {
                values
                    .iter()
                    .map(|value| {
                        GraphNodeId::new(
                            value
                                .as_str()
                                .ok_or_else(|| {
                                    invalid("Procedure v2 manual goal target is invalid")
                                })?
                                .to_owned(),
                        )
                        .map_err(|_| invalid("Procedure v2 manual goal target is invalid"))
                    })
                    .collect()
            })?;
        Ok(Self {
            outcome_maps,
            manual_targets,
            dominance_edges,
            terminal_nodes,
        })
    }

    fn revision_safe(&self, target: &GraphNodeId) -> bool {
        fn reaches_terminal_without_assessment(
            model: &GoalSnapshotModelV2,
            node: &GraphNodeId,
            visiting: &mut BTreeSet<GraphNodeId>,
        ) -> bool {
            if model.outcome_maps.contains_key(node) {
                return false;
            }
            if model.terminal_nodes.contains(node) {
                return true;
            }
            if !visiting.insert(node.clone()) {
                return false;
            }
            let unsafe_path = model.dominance_edges.get(node).is_some_and(|successors| {
                successors.iter().any(|successor| {
                    reaches_terminal_without_assessment(model, successor, visiting)
                })
            });
            visiting.remove(node);
            unsafe_path
        }
        !reaches_terminal_without_assessment(self, target, &mut BTreeSet::new())
    }
}

pub(crate) fn validate_goal_revision_target_v2(
    snapshot: &ProcedureSnapshotV2,
    target: &GraphNodeId,
) -> Result<(), crate::GraphMutationErrorV2> {
    let model = GoalSnapshotModelV2::parse(snapshot)?;
    if !model.manual_targets.contains(target) {
        return Err(crate::GraphMutationErrorV2::GoalRevisionTargetNotAllowed {
            target_graph_node_id: target.clone(),
        });
    }
    if !model.revision_safe(target) {
        return Err(
            crate::GraphMutationErrorV2::GoalRevisionTargetNotRevisionSafe {
                target_graph_node_id: target.clone(),
            },
        );
    }
    Ok(())
}

pub(crate) fn validate_goal_state_v2(
    snapshot: &ProcedureSnapshotV2,
    trace: &SessionTraceV2,
    metadata: &[AttemptMetadataV2],
    workflow: &WorkflowMemoryStateV2,
    state: &GoalStateV2,
) -> Result<(), ProcedureStateErrorV2> {
    let any_binding = trace
        .attempts()
        .iter()
        .any(|attempt| attempt.goal_revision().is_some());
    if !snapshot.goal_tracking() {
        if state != &GoalStateV2::empty() || any_binding {
            return Err(invalid("Procedure v2 goal state requires snapshot opt-in"));
        }
        return Ok(());
    }
    if state.revisions().is_empty() {
        if state.current_revision().is_some()
            || any_binding
            || !state.attempt_assessments().is_empty()
            || !state.assessments().is_empty()
        {
            return Err(invalid("Procedure v2 empty goal state is inconsistent"));
        }
        if trace.lifecycle() == SessionLifecycle::Completed {
            return Err(invalid(
                "completed Procedure v2 goal tracking requires an assessment",
            ));
        }
        return Ok(());
    }
    let latest = GoalRevisionNumberV2::new(state.revisions().len() as u64);
    if state.current_revision() != Some(latest) {
        return Err(invalid("Procedure v2 current goal revision is not latest"));
    }
    let model = GoalSnapshotModelV2::parse(snapshot)?;
    let metadata_by_id: BTreeMap<_, _> = metadata
        .iter()
        .map(|value| (value.attempt_id(), value))
        .collect();
    for (index, revision) in state.revisions().iter().enumerate() {
        let number = GoalRevisionNumberV2::new(index as u64 + 1);
        if revision.revision() != number
            || revision.predecessor()
                != (index > 0).then(|| GoalRevisionNumberV2::new(index as u64))
        {
            return Err(invalid(
                "Procedure v2 goal revision history is not contiguous",
            ));
        }
        let binding = trace
            .attempts()
            .iter()
            .find(|attempt| attempt.trace() == revision.binding_trace())
            .ok_or_else(|| invalid("Procedure v2 goal revision binding attempt is absent"))?;
        let binding_metadata = metadata_by_id
            .get(binding.attempt_id())
            .ok_or_else(|| invalid("Procedure v2 goal revision binding metadata is absent"))?;
        if binding.goal_revision() != Some(number) {
            return Err(invalid(
                "Procedure v2 goal revision binding is inconsistent",
            ));
        }
        if index == 0 {
            if revision.created_at() < binding_metadata.started_at() {
                return Err(invalid(
                    "Procedure v2 initial goal revision predates its binding attempt",
                ));
            }
            if binding_metadata
                .ended_at()
                .is_some_and(|ended| revision.created_at() > ended)
            {
                return Err(invalid(
                    "Procedure v2 initial goal revision postdates its binding attempt",
                ));
            }
            if trace.attempts().iter().any(|attempt| {
                attempt.trace() > revision.binding_trace() && attempt.goal_revision().is_none()
            }) {
                return Err(invalid(
                    "Procedure v2 attempt after goal definition is unbound",
                ));
            }
        } else {
            let prior_cursor = trace
                .attempts()
                .iter()
                .find(|attempt| {
                    attempt.trace().get().checked_add(1) == Some(revision.binding_trace().get())
                })
                .ok_or_else(|| invalid("Procedure v2 goal revision prior cursor is absent"))?;
            let prior_metadata = metadata_by_id
                .get(prior_cursor.attempt_id())
                .ok_or_else(|| invalid("Procedure v2 goal revision prior metadata is absent"))?;
            let terminal_reactivation = model.terminal_nodes.contains(prior_cursor.graph_node_id())
                && matches!(
                    prior_cursor.lifecycle(),
                    AttemptLifecycle::Completed | AttemptLifecycle::Skipped
                );
            if revision.reactivated() != terminal_reactivation {
                return Err(invalid(
                    "Procedure v2 goal reactivation disagrees with the prior cursor",
                ));
            }
            if revision.reactivated() {
                if prior_metadata
                    .ended_at()
                    .is_none_or(|ended_at| ended_at > revision.created_at())
                {
                    return Err(invalid(
                        "Procedure v2 goal reactivation predates the prior terminal cursor",
                    ));
                }
            } else if prior_cursor.lifecycle() != AttemptLifecycle::Abandoned
                || prior_metadata.ended_at() != Some(revision.created_at())
                || prior_metadata.terminal_reason()
                    != revision.reason().map(GoalRevisionReasonV2::as_str)
            {
                return Err(invalid(
                    "Procedure v2 running goal revision source is inconsistent",
                ));
            }
            if revision.created_at() != binding_metadata.started_at()
                || revision.rework_to() != Some(binding.graph_node_id())
                || !model.manual_targets.contains(binding.graph_node_id())
                || !model.revision_safe(binding.graph_node_id())
            {
                return Err(invalid(
                    "Procedure v2 goal revision rework target is inconsistent",
                ));
            }
        }
    }
    for attempt in trace.attempts() {
        let expected = state
            .revisions()
            .iter()
            .rev()
            .find(|revision| revision.binding_trace() <= attempt.trace())
            .map(GoalRevisionRecordV2::revision);
        if attempt.goal_revision() != expected {
            return Err(invalid("Procedure v2 attempt goal binding is invalid"));
        }
    }
    let revisions: BTreeMap<_, _> = state
        .revisions()
        .iter()
        .map(|record| (record.revision(), record))
        .collect();
    let attempts: BTreeMap<_, _> = trace
        .attempts()
        .iter()
        .map(|attempt| (attempt.attempt_id(), attempt))
        .collect();
    let memories: BTreeMap<_, _> = workflow
        .attempts()
        .iter()
        .map(|memory| (memory.attempt_id(), memory))
        .collect();
    let mut prior_assessment_trace = None;
    for assessment in state.attempt_assessments() {
        let attempt = attempts
            .get(assessment.attempt_id())
            .ok_or_else(|| invalid("Procedure v2 criterion assessment attempt is absent"))?;
        if prior_assessment_trace.is_some_and(|prior| attempt.trace() <= prior) {
            return Err(invalid(
                "Procedure v2 criterion assessment owners are not in trace order",
            ));
        }
        prior_assessment_trace = Some(attempt.trace());
        let node = snapshot
            .graph_node(attempt.graph_node_id())
            .ok_or_else(|| invalid("Procedure v2 criterion assessment node is absent"))?;
        let attempt_metadata = metadata_by_id
            .get(attempt.attempt_id())
            .ok_or_else(|| invalid("Procedure v2 criterion assessment metadata is absent"))?;
        if !node.goal_assessment() || attempt.goal_revision() != Some(assessment.goal_revision()) {
            return Err(invalid(
                "Procedure v2 criterion assessment owner is invalid",
            ));
        }
        let definition = revisions
            .get(&assessment.goal_revision())
            .ok_or_else(|| invalid("Procedure v2 criterion assessment goal revision is absent"))?
            .criteria();
        let criterion_order: BTreeMap<_, _> = definition
            .criteria()
            .iter()
            .enumerate()
            .map(|(index, criterion)| (criterion.id(), index))
            .collect();
        let mut last = None;
        let memory = memories
            .get(assessment.attempt_id())
            .ok_or_else(|| invalid("Procedure v2 criterion assessment memory is absent"))?;
        for result in assessment.results() {
            if result.recorded_at() < attempt_metadata.started_at()
                || attempt_metadata
                    .ended_at()
                    .is_some_and(|ended| result.recorded_at() > ended)
            {
                return Err(invalid(
                    "Procedure v2 criterion assessment timestamp is outside its attempt",
                ));
            }
            let index = *criterion_order
                .get(result.result().criterion_id())
                .ok_or_else(|| {
                    invalid("Procedure v2 criterion is not in the bound goal revision")
                })?;
            if last.is_some_and(|prior| index <= prior) {
                return Err(invalid(
                    "Procedure v2 criterion results are not in definition order",
                ));
            }
            last = Some(index);
            for citation in result.result().citations() {
                let valid = match citation {
                    CriterionCitationV2::Evidence(source) => {
                        memory.evidence().iter().any(|reference| {
                            reference.resolution().source_node() == source
                                && matches!(
                                    reference.resolution(),
                                    crate::ResolvedEvidenceReferenceV2::Resolved(_)
                                )
                        })
                    }
                    CriterionCitationV2::Item(item) => memory
                        .item_slots()
                        .iter()
                        .any(|slot| slot.item_id() == item && slot.value().is_some()),
                };
                if !valid {
                    return Err(invalid("Procedure v2 criterion citation target is invalid"));
                }
            }
        }
    }
    if state
        .assessments()
        .windows(2)
        .any(|pair| pair[0].decision_trace() >= pair[1].decision_trace())
    {
        return Err(invalid(
            "Procedure v2 goal assessments are not in trace order",
        ));
    }
    for assessment in state.assessments() {
        let attempt = attempts
            .get(assessment.decision_attempt_id())
            .ok_or_else(|| invalid("Procedure v2 goal assessment attempt is absent"))?;
        let decision = workflow
            .decisions()
            .iter()
            .find(|record| record.attempt_id() == assessment.decision_attempt_id())
            .ok_or_else(|| invalid("Procedure v2 goal assessment decision is absent"))?;
        let node_outcomes = model
            .outcome_maps
            .get(attempt.graph_node_id())
            .ok_or_else(|| invalid("Procedure v2 goal assessment node contract is absent"))?;
        let expected_outcome = node_outcomes
            .get(decision.selected_option())
            .ok_or_else(|| invalid("Procedure v2 goal assessment option mapping is absent"))?;
        let result_state = state
            .attempt_assessments()
            .iter()
            .find(|value| value.attempt_id() == attempt.attempt_id())
            .ok_or_else(|| invalid("Procedure v2 goal assessment criterion state is absent"))?;
        let persisted_results = result_state
            .results()
            .iter()
            .map(|value| value.result().clone())
            .collect::<Vec<_>>();
        if attempt.lifecycle() != AttemptLifecycle::Completed
            || attempt.goal_revision() != Some(assessment.goal_revision())
            || assessment.decision_graph_node_id() != attempt.graph_node_id()
            || assessment.decision_trace() != attempt.trace()
            || assessment.outcome() != *expected_outcome
            || assessment.criterion_results() != persisted_results
            || assessment.evidence() != decision.evidence()
            || assessment.actor() != decision.actor()
            || assessment.recorded_at() != decision.recorded_at()
        {
            return Err(invalid(
                "Procedure v2 goal assessment does not match its decision",
            ));
        }
        let criteria = revisions[&assessment.goal_revision()].criteria().criteria();
        if assessment.criterion_results().len() != criteria.len()
            || assessment
                .criterion_results()
                .iter()
                .zip(criteria)
                .any(|(result, criterion)| result.criterion_id() != criterion.id())
        {
            return Err(invalid(
                "Procedure v2 goal assessment criterion set is incomplete",
            ));
        }
    }
    for attempt in trace
        .attempts()
        .iter()
        .filter(|attempt| attempt.lifecycle() == AttemptLifecycle::Completed)
    {
        if snapshot
            .graph_node(attempt.graph_node_id())
            .is_some_and(|node| node.goal_assessment())
            && !state
                .assessments()
                .iter()
                .any(|assessment| assessment.decision_attempt_id() == attempt.attempt_id())
        {
            return Err(invalid(
                "completed Procedure v2 goal decision has no assessment",
            ));
        }
    }
    if trace.lifecycle() == SessionLifecycle::Completed
        && state.latest_fresh_assessment(trace).is_none()
    {
        return Err(invalid(
            "completed Procedure v2 session has no fresh goal assessment",
        ));
    }
    Ok(())
}

pub fn validate_goal_state_successor_v2<'a>(
    previous_trace: &'a SessionTraceV2,
    previous_workflow: &WorkflowMemoryStateV2,
    previous: &'a GoalStateV2,
    next_trace: &'a SessionTraceV2,
    next_workflow: &WorkflowMemoryStateV2,
    next: &'a GoalStateV2,
) -> Result<WorkflowGoalTransitionV2<'a>, ProcedureStateErrorV2> {
    if next.revisions().len() < previous.revisions().len()
        || next.revisions().len() > previous.revisions().len() + 1
        || next.attempt_assessments().len() < previous.attempt_assessments().len()
        || next.attempt_assessments().len() > previous.attempt_assessments().len() + 1
        || next.assessments().len() < previous.assessments().len()
        || next.assessments().len() > previous.assessments().len() + 1
        || next.revisions()[..previous.revisions().len()] != *previous.revisions()
        || next.assessments()[..previous.assessments().len()] != *previous.assessments()
    {
        return Err(invalid("Procedure v2 goal history is not append-only"));
    }
    for old in previous.attempt_assessments() {
        let new = next
            .attempt_assessments()
            .iter()
            .find(|value| value.attempt_id() == old.attempt_id())
            .ok_or_else(|| invalid("Procedure v2 criterion history was removed"))?;
        let new_by_id: BTreeMap<_, _> = new
            .results()
            .iter()
            .map(|state| (state.result().criterion_id(), state))
            .collect();
        if new.goal_revision() != old.goal_revision()
            || new.results().len() < old.results().len()
            || new.results().len() > old.results().len() + 1
            || old
                .results()
                .iter()
                .any(|state| new_by_id.get(state.result().criterion_id()).copied() != Some(state))
        {
            return Err(invalid("Procedure v2 criterion history is not append-only"));
        }
    }
    let result_delta: usize = next
        .attempt_assessments()
        .iter()
        .map(|value| value.results().len())
        .sum::<usize>()
        - previous
            .attempt_assessments()
            .iter()
            .map(|value| value.results().len())
            .sum::<usize>();
    if result_delta > 1 {
        return Err(invalid(
            "Procedure v2 criterion successor appends at most one result",
        ));
    }
    if result_delta == 1 {
        let active = previous_trace
            .active_attempt()
            .ok_or_else(|| invalid("Procedure v2 criterion successor has no active attempt"))?;
        if next_trace
            .active_attempt()
            .map(|attempt| attempt.attempt_id())
            != Some(active.attempt_id())
            || previous_workflow != next_workflow
            || next.assessments().len() != previous.assessments().len()
            || next_workflow.decisions().len() != previous_workflow.decisions().len()
        {
            return Err(invalid(
                "Procedure v2 criterion result mutation is not cursor-stable",
            ));
        }
        let changed_owner = next.attempt_assessments().iter().find(|candidate| {
            let old_len = previous
                .attempt_assessments()
                .iter()
                .find(|old| old.attempt_id() == candidate.attempt_id())
                .map_or(0, |old| old.results().len());
            candidate.results().len() == old_len + 1
        });
        if changed_owner.map(AttemptCriterionAssessmentStateV2::attempt_id)
            != Some(active.attempt_id())
        {
            return Err(invalid(
                "Procedure v2 criterion result is not bound to the active attempt",
            ));
        }
    }
    if next.assessments().len() == previous.assessments().len() + 1 {
        let assessment = next.assessments().last().expect("length checked");
        let decision = next_workflow
            .decisions()
            .get(previous_workflow.decisions().len())
            .ok_or_else(|| invalid("Procedure v2 goal assessment lacks its new decision"))?;
        if assessment.decision_attempt_id() != decision.attempt_id() {
            return Err(invalid(
                "Procedure v2 goal assessment is not paired atomically",
            ));
        }
    }
    if next.revisions().len() == previous.revisions().len() {
        if next.current_revision() != previous.current_revision() {
            return Err(invalid(
                "Procedure v2 current goal revision changed without a record",
            ));
        }
        return Ok(WorkflowGoalTransitionV2::None);
    }
    let revision = next.revisions().last().expect("length checked");
    if result_delta != 0
        || next.assessments().len() != previous.assessments().len()
        || next_workflow.decisions().len() != previous_workflow.decisions().len()
        || next_workflow.reworks().len() != previous_workflow.reworks().len()
    {
        return Err(invalid(
            "Procedure v2 goal revision cannot combine unrelated history events",
        ));
    }
    if previous.revisions().is_empty() {
        let active = previous_trace.active_attempt().or_else(|| {
            (previous_trace.lifecycle() == SessionLifecycle::Prepared)
                .then(|| next_trace.active_attempt())
                .flatten()
        });
        let active = active
            .ok_or_else(|| invalid("Procedure v2 initial goal definition has no active attempt"))?;
        if revision.revision() != GoalRevisionNumberV2::FIRST
            || revision.binding_trace() != active.trace()
            || next_trace
                .active_attempt()
                .map(|attempt| attempt.attempt_id())
                != Some(active.attempt_id())
        {
            return Err(invalid(
                "Procedure v2 initial goal definition binding is invalid",
            ));
        }
        return Ok(WorkflowGoalTransitionV2::InitialBinding {
            attempt_id: active.attempt_id(),
        });
    }
    let fresh = next_trace
        .attempts()
        .get(previous_trace.attempts().len())
        .ok_or_else(|| invalid("Procedure v2 goal revision has no fresh target"))?;
    if revision.binding_trace() != fresh.trace()
        || revision.rework_to() != Some(fresh.graph_node_id())
        || fresh.goal_revision() != Some(revision.revision())
        || revision.reactivated() != (previous_trace.lifecycle() == SessionLifecycle::Completed)
    {
        return Err(invalid(
            "Procedure v2 goal revision transition is inconsistent",
        ));
    }
    Ok(WorkflowGoalTransitionV2::Rework {
        target_attempt_id: fresh.attempt_id(),
    })
}

#[cfg(test)]
mod tests;
