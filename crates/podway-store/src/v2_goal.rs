//! Procedure v2 session-goal persistence and immutable assessment history.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use podway_core::{
    ActorAttributionV2, AttemptId, AttemptLifecycle, AttemptValidityV2, CriterionAssessmentModeV2,
    CriterionAssessmentReasonV2, CriterionAssessmentResultV2, CriterionCitationV2, CriterionId,
    CriterionStatusV2, GoalAssessmentRecordV2, GoalCriterionV2, GoalDefinitionV2, GoalOutcome,
    GoalRevisionNumberV2, GoalRevisionReasonV2, GoalRevisionRecordV2, GoalStatementV2, GraphNodeId,
    ItemId, OptionId, SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2,
    UnixMillis, canonicalize_json_v1,
};
use rusqlite::{Connection, Transaction, params};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::v2_memory::{WorkflowGoalTransitionV2, WorkflowMemoryStateV2};
use crate::v2_state::{AttemptMetadataV2, ProcedureSnapshotV2};
use crate::{
    RusqliteErrorContextV1, StoreErrorV1, StoreRecordKindV1, StoreValueErrorV1,
    map_rusqlite_error_v1,
};

fn invalid(reason: &'static str) -> StoreValueErrorV1 {
    StoreValueErrorV1::InvalidProcedureV2State { reason }
}

fn corrupt() -> StoreErrorV1 {
    StoreErrorV1::CorruptStateV1 {
        record: StoreRecordKindV1::Session,
    }
}

fn record_error(error: rusqlite::Error) -> StoreErrorV1 {
    map_rusqlite_error_v1(
        error,
        RusqliteErrorContextV1::Record(StoreRecordKindV1::Session),
    )
}

fn sqlite_u64(value: u64, field: &'static str) -> Result<i64, StoreErrorV1> {
    i64::try_from(value)
        .map_err(|_| StoreErrorV1::InvalidStateV1(StoreValueErrorV1::IntegerOutOfRange { field }))
}

fn persisted_u64(value: i64) -> Result<u64, StoreErrorV1> {
    u64::try_from(value).map_err(|_| corrupt())
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
    ) -> Result<Self, StoreValueErrorV1> {
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
    ) -> Result<Self, StoreValueErrorV1> {
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
    fn parse(snapshot: &ProcedureSnapshotV2) -> Result<Self, StoreValueErrorV1> {
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
                .collect::<Result<BTreeMap<_, _>, StoreValueErrorV1>>()?;
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

pub(crate) fn validate_goal_state_v2(
    snapshot: &ProcedureSnapshotV2,
    trace: &SessionTraceV2,
    metadata: &[AttemptMetadataV2],
    workflow: &WorkflowMemoryStateV2,
    state: &GoalStateV2,
) -> Result<(), StoreValueErrorV1> {
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
                                    podway_core::ResolvedEvidenceReferenceV2::Resolved(_)
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

pub(crate) fn validate_goal_state_successor_v2<'a>(
    previous_trace: &'a SessionTraceV2,
    previous_workflow: &WorkflowMemoryStateV2,
    previous: &'a GoalStateV2,
    next_trace: &'a SessionTraceV2,
    next_workflow: &WorkflowMemoryStateV2,
    next: &'a GoalStateV2,
) -> Result<WorkflowGoalTransitionV2<'a>, StoreValueErrorV1> {
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
        let active = previous_trace
            .active_attempt()
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

pub(crate) fn insert_goal_state_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    workflow: &WorkflowMemoryStateV2,
    state: &GoalStateV2,
) -> Result<(), StoreErrorV1> {
    for revision in state.revisions() {
        insert_goal_revision_v2(transaction, session_id, revision)?;
    }
    for attempt in state.attempt_assessments() {
        for result in attempt.results() {
            insert_criterion_result_v2(transaction, session_id, attempt, result)?;
        }
    }
    for assessment in state.assessments() {
        insert_goal_assessment_v2(transaction, session_id, workflow, assessment)?;
    }
    Ok(())
}

pub(crate) fn replace_goal_state_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    previous: &GoalStateV2,
    next_workflow: &WorkflowMemoryStateV2,
    next: &GoalStateV2,
) -> Result<(), StoreErrorV1> {
    for revision in &next.revisions()[previous.revisions().len()..] {
        insert_goal_revision_v2(transaction, session_id, revision)?;
    }
    for attempt in next.attempt_assessments() {
        let old_ids = previous
            .attempt_assessments()
            .iter()
            .find(|old| old.attempt_id() == attempt.attempt_id())
            .map(|old| {
                old.results()
                    .iter()
                    .map(|state| state.result().criterion_id())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        for result in attempt
            .results()
            .iter()
            .filter(|state| !old_ids.contains(state.result().criterion_id()))
        {
            insert_criterion_result_v2(transaction, session_id, attempt, result)?;
        }
    }
    for assessment in &next.assessments()[previous.assessments().len()..] {
        insert_goal_assessment_v2(transaction, session_id, next_workflow, assessment)?;
    }
    Ok(())
}

fn insert_goal_revision_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    revision: &GoalRevisionRecordV2,
) -> Result<(), StoreErrorV1> {
    transaction.execute(
        "INSERT INTO v2_goal_revisions (session_id, goal_revision, predecessor_revision, statement, reason, \
         rework_to_graph_node_id, reactivated, actor, binding_trace_sequence, created_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            session_id.as_str(),
            sqlite_u64(revision.revision().get(), "Procedure v2 goal revision")?,
            revision.predecessor().map(|value| sqlite_u64(value.get(), "Procedure v2 goal predecessor")).transpose()?,
            revision.statement().as_str(), revision.reason().map(GoalRevisionReasonV2::as_str),
            revision.rework_to().map(GraphNodeId::as_str), i64::from(revision.reactivated()),
            revision.actor().map(ActorAttributionV2::as_str),
            sqlite_u64(revision.binding_trace().get(), "Procedure v2 goal binding trace")?,
            sqlite_u64(revision.created_at().get(), "Procedure v2 goal timestamp")?,
        ],
    ).map_err(record_error)?;
    for (ordinal, criterion) in revision.criteria().criteria().iter().enumerate() {
        transaction.execute(
            "INSERT INTO v2_goal_criteria (session_id, goal_revision, criterion_id, criterion_ordinal, statement) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id.as_str(), sqlite_u64(revision.revision().get(), "Procedure v2 goal revision")?,
                criterion.id().as_str(), i64::try_from(ordinal).map_err(|_| corrupt())?, criterion.statement()],
        ).map_err(record_error)?;
    }
    Ok(())
}

fn insert_criterion_result_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    attempt: &AttemptCriterionAssessmentStateV2,
    state: &CriterionAssessmentStateV2,
) -> Result<(), StoreErrorV1> {
    let result = state.result();
    transaction.execute(
        "INSERT INTO v2_criterion_assessment_results (attempt_id, session_id, goal_revision, criterion_id, \
         status, mode, reason, actor, recorded_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![attempt.attempt_id().as_str(), session_id.as_str(),
            sqlite_u64(attempt.goal_revision().get(), "Procedure v2 criterion goal revision")?,
            result.criterion_id().as_str(), result.status().as_str(), result.mode().as_str(),
            result.reason().as_str(), state.actor().map(ActorAttributionV2::as_str),
            sqlite_u64(state.recorded_at().get(), "Procedure v2 criterion timestamp")?],
    ).map_err(record_error)?;
    for (ordinal, citation) in result.citations().iter().enumerate() {
        let (kind, source, item) = match citation {
            CriterionCitationV2::Evidence(source) => ("evidence", Some(source.as_str()), None),
            CriterionCitationV2::Item(item) => ("item", None, Some(item.as_str())),
        };
        transaction.execute(
            "INSERT INTO v2_criterion_citations (attempt_id, criterion_id, citation_ordinal, citation_kind, \
             source_graph_node_id, item_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![attempt.attempt_id().as_str(), result.criterion_id().as_str(),
                i64::try_from(ordinal).map_err(|_| corrupt())?, kind, source, item],
        ).map_err(record_error)?;
    }
    Ok(())
}

fn insert_goal_assessment_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    workflow: &WorkflowMemoryStateV2,
    assessment: &GoalAssessmentRecordV2,
) -> Result<(), StoreErrorV1> {
    let decision = workflow
        .decisions()
        .iter()
        .find(|decision| decision.attempt_id() == assessment.decision_attempt_id())
        .ok_or_else(|| {
            StoreErrorV1::InvalidStateV1(invalid("Procedure v2 goal assessment decision is absent"))
        })?;
    let digest = goal_assessment_digest_v2(session_id, decision, assessment)
        .map_err(StoreErrorV1::InvalidStateV1)?;
    transaction.execute(
        "INSERT INTO v2_goal_assessments (decision_attempt_id, session_id, goal_revision, decision_trace_sequence, \
         outcome, mode, selected_option_id, route_effect, route_target_graph_node_id, actor, recorded_at_ms, record_digest) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![assessment.decision_attempt_id().as_str(), session_id.as_str(),
            sqlite_u64(assessment.goal_revision().get(), "Procedure v2 assessment goal revision")?,
            sqlite_u64(assessment.decision_trace().get(), "Procedure v2 assessment trace")?,
            assessment.outcome().as_str(), assessment.mode().as_str(), decision.selected_option().as_str(),
            decision.route_effect().as_str(), decision.route_target().as_str(),
            assessment.actor().map(ActorAttributionV2::as_str),
            sqlite_u64(assessment.recorded_at().get(), "Procedure v2 assessment timestamp")?, digest.as_str()],
    ).map_err(record_error)?;
    Ok(())
}

fn goal_assessment_digest_v2(
    session_id: &SessionId,
    decision: &podway_core::DecisionRecordV2,
    assessment: &GoalAssessmentRecordV2,
) -> Result<Sha256Digest, StoreValueErrorV1> {
    let results = assessment.criterion_results().iter().map(|result| {
        let citations = result.citations().iter().map(|citation| match citation {
            CriterionCitationV2::Evidence(source) => json!({"kind":"evidence","source_graph_node_id":source.as_str()}),
            CriterionCitationV2::Item(item) => json!({"item_id":item.as_str(),"kind":"item"}),
        }).collect::<Vec<_>>();
        json!({"citations":citations,"criterion_id":result.criterion_id().as_str(),"reason":result.reason().as_str(),"status":result.status().as_str()})
    }).collect::<Vec<_>>();
    let evidence = assessment.evidence().references().iter().map(|reference| match reference {
        podway_core::ResolvedEvidenceReferenceV2::Resolved(value) => json!({
            "items_digest":value.items_digest().as_str(),"resolved_at_ms":value.resolved_at().get(),
            "source_attempt_id":value.source_attempt_id().as_str(),"source_attempt_number":value.source_attempt_number().get(),
            "source_graph_node_id":value.source_node().as_str(),"state":"resolved"}),
        podway_core::ResolvedEvidenceReferenceV2::Skipped(value) => json!({
            "items_digest":value.items_digest().as_str(),"resolved_at_ms":value.resolved_at().get(),
            "source_attempt_id":value.source_attempt_id().as_str(),"source_attempt_number":value.source_attempt_number().get(),
            "source_graph_node_id":value.source_node().as_str(),"state":"skipped"}),
        podway_core::ResolvedEvidenceReferenceV2::Unresolved { source_node } =>
            json!({"source_graph_node_id":source_node.as_str(),"state":"unresolved"}),
    }).collect::<Vec<_>>();
    let value = json!({
        "actor":assessment.actor().map(ActorAttributionV2::as_str),
        "criterion_results":results,
        "decision_attempt_id":assessment.decision_attempt_id().as_str(),
        "decision_graph_node_id":assessment.decision_graph_node_id().as_str(),
        "decision_trace_sequence":assessment.decision_trace().get(),
        "evidence":evidence,
        "goal_revision":assessment.goal_revision().get(),
        "mode":assessment.mode().as_str(),
        "outcome":assessment.outcome().as_str(),
        "recorded_at_ms":assessment.recorded_at().get(),
        "route_effect":decision.route_effect().as_str(),
        "route_target_graph_node_id":decision.route_target().as_str(),
        "selected_option_id":decision.selected_option().as_str(),
        "session_id":session_id.as_str(),
    });
    let canonical = canonicalize_json_v1(&value)
        .map_err(|_| invalid("Procedure v2 goal assessment is not canonicalizable"))?;
    Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
        .map_err(|_| invalid("Procedure v2 goal assessment digest is invalid"))
}

pub(crate) fn load_goal_state_v2(
    connection: &Connection,
    session_id: &SessionId,
    current_revision: Option<GoalRevisionNumberV2>,
    workflow: &WorkflowMemoryStateV2,
) -> Result<GoalStateV2, StoreErrorV1> {
    let revisions = load_goal_revisions_v2(connection, session_id)?;
    let attempt_assessments = load_criterion_assessments_v2(connection, session_id)?;
    let assessments =
        load_goal_assessments_v2(connection, session_id, workflow, &attempt_assessments)?;
    let expected_counts = [
        revisions.len(),
        revisions
            .iter()
            .map(|revision| revision.criteria().criteria().len())
            .sum(),
        attempt_assessments
            .iter()
            .map(|attempt| attempt.results().len())
            .sum(),
        attempt_assessments
            .iter()
            .flat_map(|attempt| attempt.results())
            .map(|state| state.result().citations().len())
            .sum(),
        assessments.len(),
    ];
    let actual: (i64, i64, i64, i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM v2_goal_revisions), \
             (SELECT COUNT(*) FROM v2_goal_criteria), \
             (SELECT COUNT(*) FROM v2_criterion_assessment_results), \
             (SELECT COUNT(*) FROM v2_criterion_citations), \
             (SELECT COUNT(*) FROM v2_goal_assessments)",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(record_error)?;
    if expected_counts
        .iter()
        .zip([actual.0, actual.1, actual.2, actual.3, actual.4])
        .any(|(expected, actual)| i64::try_from(*expected).ok() != Some(actual))
    {
        return Err(corrupt());
    }
    GoalStateV2::new(
        current_revision,
        revisions,
        attempt_assessments,
        assessments,
    )
    .map_err(|_| corrupt())
}

fn load_goal_revisions_v2(
    connection: &Connection,
    session_id: &SessionId,
) -> Result<Vec<GoalRevisionRecordV2>, StoreErrorV1> {
    type RevisionRow = (
        i64,
        Option<i64>,
        String,
        Option<String>,
        Option<String>,
        i64,
        Option<String>,
        i64,
        i64,
    );
    let mut statement = connection.prepare(
        "SELECT goal_revision, predecessor_revision, statement, reason, rework_to_graph_node_id, \
         reactivated, actor, binding_trace_sequence, created_at_ms FROM v2_goal_revisions \
         WHERE session_id = ?1 ORDER BY goal_revision",
    ).map_err(record_error)?;
    let rows = statement
        .query_map([session_id.as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })
        .map_err(record_error)?
        .collect::<Result<Vec<RevisionRow>, _>>()
        .map_err(record_error)?;
    let mut revisions = Vec::with_capacity(rows.len());
    for row in rows {
        let number = GoalRevisionNumberV2::new(persisted_u64(row.0)?);
        let mut criteria_statement = connection
            .prepare(
                "SELECT criterion_id, criterion_ordinal, statement FROM v2_goal_criteria \
             WHERE session_id = ?1 AND goal_revision = ?2 ORDER BY criterion_ordinal",
            )
            .map_err(record_error)?;
        let criteria_rows = criteria_statement
            .query_map(params![session_id.as_str(), row.0], |value| {
                Ok((
                    value.get::<_, String>(0)?,
                    value.get::<_, i64>(1)?,
                    value.get::<_, String>(2)?,
                ))
            })
            .map_err(record_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(record_error)?;
        let mut criteria = Vec::with_capacity(criteria_rows.len());
        for (index, (id, ordinal, statement)) in criteria_rows.into_iter().enumerate() {
            if ordinal != i64::try_from(index).map_err(|_| corrupt())? {
                return Err(corrupt());
            }
            criteria.push(
                GoalCriterionV2::new(CriterionId::new(id).map_err(|_| corrupt())?, statement)
                    .map_err(|_| corrupt())?,
            );
        }
        revisions.push(
            GoalRevisionRecordV2::new(
                number,
                row.1
                    .map(persisted_u64)
                    .transpose()?
                    .map(GoalRevisionNumberV2::new),
                GoalStatementV2::new(row.2).map_err(|_| corrupt())?,
                GoalDefinitionV2::new(criteria).map_err(|_| corrupt())?,
                row.3
                    .map(GoalRevisionReasonV2::new)
                    .transpose()
                    .map_err(|_| corrupt())?,
                row.4
                    .map(GraphNodeId::new)
                    .transpose()
                    .map_err(|_| corrupt())?,
                match row.5 {
                    0 => false,
                    1 => true,
                    _ => return Err(corrupt()),
                },
                row.6
                    .map(ActorAttributionV2::new)
                    .transpose()
                    .map_err(|_| corrupt())?,
                TraceSequenceV2::new(persisted_u64(row.7)?),
                UnixMillis::new(persisted_u64(row.8)?),
            )
            .map_err(|_| corrupt())?,
        );
    }
    Ok(revisions)
}

fn load_criterion_assessments_v2(
    connection: &Connection,
    session_id: &SessionId,
) -> Result<Vec<AttemptCriterionAssessmentStateV2>, StoreErrorV1> {
    type ResultRow = (
        String,
        i64,
        String,
        String,
        String,
        String,
        Option<String>,
        i64,
    );
    let mut statement = connection.prepare(
        "SELECT r.attempt_id, r.goal_revision, r.criterion_id, r.status, r.mode, r.reason, r.actor, r.recorded_at_ms \
         FROM v2_criterion_assessment_results r JOIN v2_attempts a ON a.attempt_id = r.attempt_id \
         JOIN v2_goal_criteria c ON c.session_id = r.session_id AND c.goal_revision = r.goal_revision \
           AND c.criterion_id = r.criterion_id WHERE r.session_id = ?1 \
         ORDER BY a.trace_sequence, c.criterion_ordinal",
    ).map_err(record_error)?;
    let rows = statement
        .query_map([session_id.as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ))
        })
        .map_err(record_error)?
        .collect::<Result<Vec<ResultRow>, _>>()
        .map_err(record_error)?;
    let mut grouped: Vec<(
        AttemptId,
        GoalRevisionNumberV2,
        Vec<CriterionAssessmentStateV2>,
    )> = Vec::new();
    for row in rows {
        let attempt_id = AttemptId::new(row.0).map_err(|_| corrupt())?;
        let goal_revision = GoalRevisionNumberV2::new(persisted_u64(row.1)?);
        let criterion_id = CriterionId::new(row.2).map_err(|_| corrupt())?;
        let status = CriterionStatusV2::from_str(&row.3).map_err(|_| corrupt())?;
        if row.4 != CriterionAssessmentModeV2::from_status(status).as_str() {
            return Err(corrupt());
        }
        let citations = load_citations_v2(connection, &attempt_id, &criterion_id)?;
        let result = CriterionAssessmentResultV2::new(
            criterion_id,
            status,
            CriterionAssessmentReasonV2::new(row.5).map_err(|_| corrupt())?,
            citations,
        )
        .map_err(|_| corrupt())?;
        let state = CriterionAssessmentStateV2::new(
            result,
            row.6
                .map(ActorAttributionV2::new)
                .transpose()
                .map_err(|_| corrupt())?,
            UnixMillis::new(persisted_u64(row.7)?),
        );
        if let Some((_, existing_revision, results)) =
            grouped.last_mut().filter(|(id, _, _)| id == &attempt_id)
        {
            if *existing_revision != goal_revision {
                return Err(corrupt());
            }
            results.push(state);
        } else {
            grouped.push((attempt_id, goal_revision, vec![state]));
        }
    }
    grouped
        .into_iter()
        .map(|(attempt, revision, results)| {
            AttemptCriterionAssessmentStateV2::new(attempt, revision, results)
                .map_err(|_| corrupt())
        })
        .collect()
}

fn load_citations_v2(
    connection: &Connection,
    attempt_id: &AttemptId,
    criterion_id: &CriterionId,
) -> Result<Vec<CriterionCitationV2>, StoreErrorV1> {
    let mut statement = connection.prepare(
        "SELECT citation_ordinal, citation_kind, source_graph_node_id, item_id FROM v2_criterion_citations \
         WHERE attempt_id = ?1 AND criterion_id = ?2 ORDER BY citation_ordinal",
    ).map_err(record_error)?;
    let rows = statement
        .query_map(params![attempt_id.as_str(), criterion_id.as_str()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(record_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(record_error)?;
    rows.into_iter()
        .enumerate()
        .map(|(index, (ordinal, kind, source, item))| {
            if ordinal != i64::try_from(index).map_err(|_| corrupt())? {
                return Err(corrupt());
            }
            match (kind.as_str(), source, item) {
                ("evidence", Some(source), None) => Ok(CriterionCitationV2::Evidence(
                    GraphNodeId::new(source).map_err(|_| corrupt())?,
                )),
                ("item", None, Some(item)) => Ok(CriterionCitationV2::Item(
                    ItemId::new(item).map_err(|_| corrupt())?,
                )),
                _ => Err(corrupt()),
            }
        })
        .collect()
}

fn load_goal_assessments_v2(
    connection: &Connection,
    session_id: &SessionId,
    workflow: &WorkflowMemoryStateV2,
    criterion_states: &[AttemptCriterionAssessmentStateV2],
) -> Result<Vec<GoalAssessmentRecordV2>, StoreErrorV1> {
    type AssessmentRow = (
        String,
        i64,
        i64,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        i64,
        String,
    );
    let mut statement = connection.prepare(
        "SELECT decision_attempt_id, goal_revision, decision_trace_sequence, outcome, mode, selected_option_id, \
         route_effect, route_target_graph_node_id, actor, recorded_at_ms, record_digest FROM v2_goal_assessments \
         WHERE session_id = ?1 ORDER BY decision_trace_sequence",
    ).map_err(record_error)?;
    let rows = statement
        .query_map([session_id.as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
            ))
        })
        .map_err(record_error)?
        .collect::<Result<Vec<AssessmentRow>, _>>()
        .map_err(record_error)?;
    let mut assessments = Vec::with_capacity(rows.len());
    for row in rows {
        let attempt_id = AttemptId::new(row.0).map_err(|_| corrupt())?;
        let decision = workflow
            .decisions()
            .iter()
            .find(|decision| decision.attempt_id() == &attempt_id)
            .ok_or_else(corrupt)?;
        let result_state = criterion_states
            .iter()
            .find(|state| state.attempt_id() == &attempt_id)
            .ok_or_else(corrupt)?;
        let outcome = GoalOutcome::from_str(&row.3).map_err(|_| corrupt())?;
        let results = result_state
            .results()
            .iter()
            .map(|state| state.result().clone())
            .collect::<Vec<_>>();
        let assessment = GoalAssessmentRecordV2::new(
            GoalRevisionNumberV2::new(persisted_u64(row.1)?),
            outcome,
            results,
            decision.evidence().clone(),
            row.8
                .map(ActorAttributionV2::new)
                .transpose()
                .map_err(|_| corrupt())?,
            attempt_id,
            decision.graph_node_id().clone(),
            TraceSequenceV2::new(persisted_u64(row.2)?),
            UnixMillis::new(persisted_u64(row.9)?),
        )
        .map_err(|_| corrupt())?;
        if row.4 != assessment.mode().as_str()
            || row.5 != decision.selected_option().as_str()
            || row.6 != decision.route_effect().as_str()
            || row.7 != decision.route_target().as_str()
            || row.10
                != goal_assessment_digest_v2(session_id, decision, &assessment)
                    .map_err(|_| corrupt())?
                    .as_str()
        {
            return Err(corrupt());
        }
        assessments.push(assessment);
    }
    Ok(assessments)
}
