use super::*;
use crate::{
    ActorAttributionV2, CriterionAssessmentModeV2, CriterionAssessmentReasonV2, CriterionId,
    CriterionStatusV2, validate_criterion_citation_count_v2,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SuccessorCriterionCitationV1 {
    Evidence(EvidenceSourceV3),
    Item(ItemId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopedCriterionCitationRejectionV1 {
    UnresolvedEvidence,
    UnrecordedItem,
    Duplicate,
    NotApplicable,
}

impl ScopedCriterionCitationRejectionV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnresolvedEvidence => "unresolved_evidence",
            Self::UnrecordedItem => "unrecorded_item",
            Self::Duplicate => "duplicate",
            Self::NotApplicable => "not_applicable",
        }
    }
}

#[derive(Debug)]
pub struct ScopedCriterionCitationErrorV1 {
    pub decision: ScopedAttemptV1,
    pub criterion: CriterionId,
    pub citation: SuccessorCriterionCitationV1,
    pub rejection: ScopedCriterionCitationRejectionV1,
    pub citable_evidence: Vec<(EvidenceSourceV3, ScopedAttemptV1)>,
    pub citable_items: Vec<ItemId>,
}

#[derive(Debug)]
pub enum SuccessorCriterionErrorV1 {
    Domain(DomainError),
    Citation(Box<ScopedCriterionCitationErrorV1>),
}

impl From<DomainError> for SuccessorCriterionErrorV1 {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

/// An admitted citation with an immutable scoped attempt for each external evidence source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RetainedCriterionCitationV1 {
    Evidence {
        source: EvidenceSourceV3,
        target: ScopedAttemptV1,
    },
    Item(ItemId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuccessorCriterionInputV1 {
    pub criterion: CriterionId,
    pub status: CriterionStatusV2,
    pub reason: CriterionAssessmentReasonV2,
    pub citations: Vec<SuccessorCriterionCitationV1>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuccessorCriterionResultV1 {
    pub criterion: CriterionId,
    pub status: CriterionStatusV2,
    pub reason: CriterionAssessmentReasonV2,
    pub citations: Vec<RetainedCriterionCitationV1>,
    pub actor: Option<ActorAttributionV2>,
    pub recorded_at: UnixMillis,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuccessorCriterionSetV1 {
    pub goal_revision: GoalRevisionNumberV2,
    pub results: Vec<SuccessorCriterionResultV1>,
}

impl GroupAttemptStateV1 {
    pub fn criterion_assessments(&self) -> Option<&SuccessorCriterionSetV1> {
        self.criterion_assessments.as_ref()
    }
}

impl ExecutionSessionV3 {
    /// Bind a partial or complete batch to the current goal assessment attempt.
    pub fn assess_criteria(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        entries: Vec<SuccessorCriterionInputV1>,
        actor: Option<ActorAttributionV2>,
        now: UnixMillis,
    ) -> Result<(), SuccessorCriterionErrorV1> {
        let revision = self.check_fence(fence, now)?;
        let (attempt, goal, goal_revision) = self.current_criterion_basis()?;
        if entries.is_empty() || entries.len() > goal.criteria.criteria().len() {
            return Err(invalid("criterion assessment batch must contain 1 to 16 entries").into());
        }
        let mut proposed =
            attempt
                .criterion_assessments
                .clone()
                .unwrap_or_else(|| SuccessorCriterionSetV1 {
                    goal_revision,
                    results: Vec::new(),
                });
        for input in entries {
            validate_criterion_citation_count_v2(input.citations.len())?;
            let mut citations = Vec::with_capacity(input.citations.len());
            for (index, citation) in input.citations.iter().enumerate() {
                use ScopedCriterionCitationRejectionV1 as Rejection;
                let reject = |rejection| {
                    self.reject_criterion_citation(attempt, &input.criterion, citation, rejection)
                };
                if input.status == CriterionStatusV2::NotApplicable {
                    return Err(reject(Rejection::NotApplicable));
                }
                if input.citations[..index].contains(citation) {
                    return Err(reject(Rejection::Duplicate));
                }
                citations.push(match citation {
                    SuccessorCriterionCitationV1::Evidence(source) => {
                        let target = self
                            .current_evidence_source(source)
                            .ok_or_else(|| reject(Rejection::UnresolvedEvidence))?
                            .target()
                            .clone();
                        RetainedCriterionCitationV1::Evidence {
                            source: source.clone(),
                            target,
                        }
                    }
                    SuccessorCriterionCitationV1::Item(item) => {
                        if attempt
                            .slot(item)
                            .and_then(ItemSlotStateV2::value)
                            .is_none()
                        {
                            return Err(reject(Rejection::UnrecordedItem));
                        }
                        RetainedCriterionCitationV1::Item(item.clone())
                    }
                });
            }
            proposed.results.push(SuccessorCriterionResultV1 {
                criterion: input.criterion,
                status: input.status,
                reason: input.reason,
                citations,
                actor: actor.clone(),
                recorded_at: now,
            });
        }
        self.validate_criterion_set(&proposed, attempt, goal, goal_revision, now, true)?;
        proposed.results.sort_by_key(|result| {
            goal.criteria
                .criteria()
                .iter()
                .position(|criterion| criterion.id() == &result.criterion)
                .expect("criterion IDs already admitted")
        });
        self.attempts
            .last_mut()
            .expect("current attempt")
            .criterion_assessments = Some(proposed);
        self.update(revision, now);
        Ok(())
    }

    fn reject_criterion_citation(
        &self,
        attempt: &GroupAttemptStateV1,
        criterion: &CriterionId,
        citation: &SuccessorCriterionCitationV1,
        rejection: ScopedCriterionCitationRejectionV1,
    ) -> SuccessorCriterionErrorV1 {
        let (mut citable_evidence, mut citable_items) = (Vec::new(), Vec::new());
        if rejection != ScopedCriterionCitationRejectionV1::NotApplicable {
            if let Some(placement) = self.current_placement() {
                for reference in placement.evidence() {
                    let source = reference.source();
                    if !citable_evidence.iter().any(|(seen, _)| seen == source)
                        && let Some(attempt) = self.current_evidence_source(source)
                    {
                        citable_evidence.push((source.clone(), attempt.target().clone()));
                    }
                }
            }
            citable_items.extend(
                attempt
                    .slots()
                    .iter()
                    .filter_map(|slot| slot.value().map(|_| slot.item_id().clone())),
            );
        }
        SuccessorCriterionErrorV1::Citation(Box::new(ScopedCriterionCitationErrorV1 {
            decision: attempt.target().clone(),
            criterion: criterion.clone(),
            citation: citation.clone(),
            rejection,
            citable_evidence,
            citable_items,
        }))
    }

    /// Decide through the admitted criteria for a goal checkpoint.
    pub fn decide_assessed(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        option: OptionId,
        reason: ReasonV2,
        successor: AttemptId,
        now: UnixMillis,
    ) -> Result<(), GraphMutationErrorV2> {
        self.check_time(now)?;
        self.decide_assessed_with_effects(fence, option, reason, || now, || successor)
            .map(|_| ())
            .map_err(GroupDecisionErrorV1::into_legacy_graph)
    }

    /// Admit the exact scoped decision and settled evidence before reading the clock or a fresh attempt ID.
    pub fn decide_assessed_with_effects(
        &mut self,
        fence: &ExecutionCursorFenceV1,
        option: OptionId,
        reason: ReasonV2,
        clock: impl FnOnce() -> UnixMillis,
        successor: impl FnOnce() -> AttemptId,
    ) -> Result<UnixMillis, GroupDecisionErrorV1> {
        self.verify_cursor(fence)
            .map_err(ExecutionCursorMismatchV1::into_domain)?;
        let (attempt, placement, _) = self.current_parts()?;
        let (definition, _) = self.ordinary_decision(attempt, placement, &option)?;
        let outcome = if let Some(assessment) = definition.metadata().assessment() {
            let outcome = self.determined_goal_outcome()?.ok_or_else(|| {
                let goal = self
                    .current_goal()
                    .expect("the current criterion basis has a goal");
                let results = attempt
                    .criterion_assessments
                    .as_ref()
                    .map_or(&[][..], |set| set.results.as_slice());
                GraphMutationErrorV2::CriterionResultMissing {
                    missing_criterion_ids: goal
                        .criteria
                        .criteria()
                        .iter()
                        .filter(|criterion| {
                            !results
                                .iter()
                                .any(|result| &result.criterion == criterion.id())
                        })
                        .map(|criterion| criterion.id().clone())
                        .collect(),
                }
            })?;
            let allowed_option_ids: Vec<_> = assessment
                .outcomes()
                .iter()
                .filter(|entry| entry.outcome() == outcome)
                .map(|entry| entry.option_id().clone())
                .collect();
            if !allowed_option_ids.contains(&option) {
                return Err(GraphMutationErrorV2::GoalAssessmentOutcomeNotAllowed {
                    option_id: option,
                    determined_outcome: outcome,
                    allowed_option_ids,
                }
                .into());
            }
            Some(outcome)
        } else {
            None
        };
        self.decide_with_effects(fence, option, reason, outcome, clock, successor)
    }

    /// A complete admissible set is the only public source of a decision outcome.
    pub fn determined_goal_outcome(&self) -> Result<Option<GoalOutcome>, DomainError> {
        let (attempt, goal, goal_revision) = self.current_criterion_basis()?;
        let Some(set) = attempt.criterion_assessments.as_ref() else {
            return Ok(None);
        };
        self.validate_criterion_set(set, attempt, goal, goal_revision, self.updated_at, true)?;
        if set.results.len() != goal.criteria.criteria().len() {
            return Ok(None);
        }
        let outcome = match set
            .results
            .first()
            .map(|result| CriterionAssessmentModeV2::from_status(result.status))
        {
            Some(CriterionAssessmentModeV2::Applicability) => GoalOutcome::Superseded,
            Some(CriterionAssessmentModeV2::Assessment)
                if set
                    .results
                    .iter()
                    .any(|r| r.status == CriterionStatusV2::Unsatisfied) =>
            {
                GoalOutcome::NotAchieved
            }
            Some(CriterionAssessmentModeV2::Assessment) => GoalOutcome::Achieved,
            None => unreachable!("admitted set contains rows"),
        };
        Ok(Some(outcome))
    }

    fn current_criterion_basis(
        &self,
    ) -> Result<(&GroupAttemptStateV1, &ExecutionGoalV3, GoalRevisionNumberV2), DomainError> {
        let (attempt, placement, _) = self.current_parts()?;
        let (_, definitions) = self.definition_for(attempt.target.scope(), None)?;
        let NodeDefinitionV3::Decision(definition) = node_definition(definitions, placement)?
        else {
            return Err(invalid(
                "criterion assessment requires a current goal decision",
            ));
        };
        if attempt.target.scope() != &ExecutionScopeV1::Base
            || definition.metadata().assessment().is_none()
        {
            return Err(invalid(
                "criterion assessment requires a current goal decision",
            ));
        }
        Ok((
            attempt,
            self.current_goal()
                .ok_or_else(|| invalid("criterion assessment requires a current goal"))?,
            self.goal_revision()
                .ok_or_else(|| invalid("criterion assessment requires a current goal revision"))?,
        ))
    }

    fn validate_criterion_set(
        &self,
        set: &SuccessorCriterionSetV1,
        attempt: &GroupAttemptStateV1,
        goal: &ExecutionGoalV3,
        revision: GoalRevisionNumberV2,
        limit: UnixMillis,
        require_current_item: bool,
    ) -> Result<(), DomainError> {
        if set.goal_revision != revision
            || set.results.is_empty()
            || set.results.len() > goal.criteria.criteria().len()
        {
            return Err(invalid(
                "criterion assessment set has an invalid goal identity or size",
            ));
        }
        let mut recorded = BTreeSet::new();
        let mode = CriterionAssessmentModeV2::from_status(set.results[0].status);
        for row in &set.results {
            if !recorded.insert(&row.criterion)
                || !goal
                    .criteria
                    .criteria()
                    .iter()
                    .any(|criterion| criterion.id() == &row.criterion)
            {
                return Err(invalid(
                    "criterion assessment repeats or names an unknown criterion",
                ));
            }
            if CriterionAssessmentModeV2::from_status(row.status) != mode {
                return Err(invalid(
                    "criterion assessment mixes applicability and assessment modes",
                ));
            }
            if row.recorded_at < attempt.opened_at || row.recorded_at > limit {
                return Err(invalid("criterion assessment has an invalid timestamp"));
            }
            validate_criterion_citation_count_v2(row.citations.len())?;
            if row.status == CriterionStatusV2::NotApplicable && !row.citations.is_empty() {
                return Err(invalid(
                    "a not_applicable criterion result must not cite evidence",
                ));
            }
            for (index, citation) in row.citations.iter().enumerate() {
                if row.citations[..index].contains(citation) {
                    return Err(invalid("criterion citations must be unique"));
                }
                match citation {
                    RetainedCriterionCitationV1::Evidence { source, target } => {
                        if self
                            .current_evidence_source(source)
                            .map(GroupAttemptStateV1::target)
                            != Some(target)
                        {
                            return Err(invalid(
                                "criterion citation does not bind the current evidence attempt",
                            ));
                        }
                    }
                    RetainedCriterionCitationV1::Item(item) => {
                        let slot = attempt.slot(item).ok_or_else(|| {
                            invalid("criterion citation names an undeclared item")
                        })?;
                        if slot.revision() == Revision::ZERO
                            || (require_current_item && slot.value().is_none())
                        {
                            return Err(invalid(
                                "criterion citation does not bind a recorded item",
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn admit_retained_criteria(
        &mut self,
        set: &SuccessorCriterionSetV1,
        limit: UnixMillis,
    ) -> Result<(), DomainError> {
        let (attempt, goal, revision) = self.current_criterion_basis()?;
        self.validate_criterion_set(set, attempt, goal, revision, limit, false)?;
        let mut canonical = set.clone();
        canonical.results.sort_by_key(|row| {
            goal.criteria
                .criteria()
                .iter()
                .position(|criterion| criterion.id() == &row.criterion)
                .expect("criterion IDs already admitted")
        });
        if &canonical != set {
            return Err(invalid(
                "criterion assessment set is not in declaration order",
            ));
        }
        if let Some(latest) = canonical.results.iter().map(|row| row.recorded_at).max() {
            self.updated_at = self.updated_at.max(latest);
        }
        self.attempts
            .last_mut()
            .expect("current attempt")
            .criterion_assessments = Some(canonical);
        Ok(())
    }
}
