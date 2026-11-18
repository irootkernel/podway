//! Immutable identity and provenance records. Their contents are caller assertions, not proof of work.
use super::{MAX_PRIOR_WORK_DISPOSITIONS_V1, bounded_text, invalid};
use crate::{
    AttemptId, CheckResultInputBasisV2, DomainError, GoalRevisionNumberV2, GraphNodeId, ItemId,
    OptionId, Revision, SessionId, Sha256Digest,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Distinct activations have distinct IDs even when their immutable document digest matches.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GroupExecutionIdV1(SessionId);
impl GroupExecutionIdV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        SessionId::new(value)
            .map(Self)
            .map_err(|_| DomainError::InvalidUuid {
                field: "GroupExecutionIdV1",
            })
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
impl TryFrom<String> for GroupExecutionIdV1 {
    type Error = DomainError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<GroupExecutionIdV1> for String {
    fn from(value: GroupExecutionIdV1) -> Self {
        value.0.into_inner()
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionScopeV1 {
    Base,
    Group {
        group_execution_id: GroupExecutionIdV1,
        definition_digest: Sha256Digest,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopedAttemptV1 {
    session: SessionId,
    base_digest: Sha256Digest,
    scope: ExecutionScopeV1,
    node: GraphNodeId,
    attempt: AttemptId,
}
impl ScopedAttemptV1 {
    pub const fn new(
        session: SessionId,
        base_digest: Sha256Digest,
        scope: ExecutionScopeV1,
        node: GraphNodeId,
        attempt: AttemptId,
    ) -> Self {
        Self {
            session,
            base_digest,
            scope,
            node,
            attempt,
        }
    }
    pub fn session(&self) -> &SessionId {
        &self.session
    }
    pub fn base_digest(&self) -> &Sha256Digest {
        &self.base_digest
    }
    pub fn scope(&self) -> &ExecutionScopeV1 {
        &self.scope
    }
    pub fn node(&self) -> &GraphNodeId {
        &self.node
    }
    pub fn attempt(&self) -> &AttemptId {
        &self.attempt
    }
}
/// Delegation packages/receipts bind this whole value, not just a local node or definition digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopedDelegationBindingV1 {
    target: ScopedAttemptV1,
    package_digest: Sha256Digest,
}
impl ScopedDelegationBindingV1 {
    pub const fn new(target: ScopedAttemptV1, package_digest: Sha256Digest) -> Self {
        Self {
            target,
            package_digest,
        }
    }
    pub fn target(&self) -> &ScopedAttemptV1 {
        &self.target
    }
    pub fn package_digest(&self) -> &Sha256Digest {
        &self.package_digest
    }
    pub fn applies_to(&self, target: &ScopedAttemptV1, package_digest: &Sha256Digest) -> bool {
        &self.target == target && &self.package_digest == package_digest
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanItemBindingV1 {
    item: ItemId,
    revision: Revision,
    value_digest: Sha256Digest,
}
impl PlanItemBindingV1 {
    pub fn new(
        item: ItemId,
        revision: Revision,
        value_digest: Sha256Digest,
    ) -> Result<Self, DomainError> {
        if revision == Revision::ZERO {
            return Err(invalid("recorded plan item revision must be positive"));
        }
        Ok(Self {
            item,
            revision,
            value_digest,
        })
    }
    pub fn item(&self) -> &ItemId {
        &self.item
    }
    pub const fn revision(&self) -> Revision {
        self.revision
    }
    pub fn value_digest(&self) -> &Sha256Digest {
        &self.value_digest
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanBindingV1 {
    design_attempt: AttemptId,
    item_set_digest: Sha256Digest,
    items: Vec<PlanItemBindingV1>,
}
impl PlanBindingV1 {
    pub fn new(
        design_attempt: AttemptId,
        item_set_digest: Sha256Digest,
        items: Vec<PlanItemBindingV1>,
    ) -> Result<Self, DomainError> {
        if items.len() > crate::MAX_ITEMS_PER_DEFINITION_V2
            || items
                .iter()
                .map(|i| i.item())
                .collect::<BTreeSet<_>>()
                .len()
                != items.len()
        {
            return Err(invalid("invalid plan item binding collection"));
        }
        Ok(Self {
            design_attempt,
            item_set_digest,
            items,
        })
    }
    pub fn design_attempt(&self) -> &AttemptId {
        &self.design_attempt
    }
    pub fn item_set_digest(&self) -> &Sha256Digest {
        &self.item_set_digest
    }
    pub fn items(&self) -> &[PlanItemBindingV1] {
        &self.items
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PriorWorkChoiceV1 {
    Retain,
    Revalidate,
    Modify,
    Revert,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoricalItemV1 {
    attempt: AttemptId,
    item: ItemId,
    value_digest: Sha256Digest,
}
impl HistoricalItemV1 {
    pub const fn new(attempt: AttemptId, item: ItemId, value_digest: Sha256Digest) -> Self {
        Self {
            attempt,
            item,
            value_digest,
        }
    }
    pub fn attempt(&self) -> &AttemptId {
        &self.attempt
    }
    pub fn item(&self) -> &ItemId {
        &self.item
    }
    pub fn value_digest(&self) -> &Sha256Digest {
        &self.value_digest
    }
}
/// A retained whole attempt or an exact final item slot; neither is current evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistoricalReferenceV1 {
    Attempt(AttemptId),
    Item(HistoricalItemV1),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriorWorkDispositionV1 {
    group: GroupExecutionIdV1,
    item: Option<HistoricalReferenceV1>,
    choice: PriorWorkChoiceV1,
    rationale: String,
}
impl PriorWorkDispositionV1 {
    pub fn new(
        group: GroupExecutionIdV1,
        item: Option<HistoricalReferenceV1>,
        choice: PriorWorkChoiceV1,
        rationale: String,
    ) -> Result<Self, DomainError> {
        bounded_text(&rationale, 2000)?;
        Ok(Self {
            group,
            item,
            choice,
            rationale,
        })
    }
    pub fn group(&self) -> &GroupExecutionIdV1 {
        &self.group
    }
    pub fn item(&self) -> Option<&HistoricalReferenceV1> {
        self.item.as_ref()
    }
    pub const fn choice(&self) -> PriorWorkChoiceV1 {
        self.choice
    }
    pub fn rationale(&self) -> &str {
        &self.rationale
    }
}
/// All fields binding an activation are immutable. State admission validates retained references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationIdentityV1 {
    execution: GroupExecutionIdV1,
    session: SessionId,
    base_digest: Sha256Digest,
    definition_digest: Sha256Digest,
    anchor: GraphNodeId,
    goal_revision: Option<GoalRevisionNumberV2>,
    plan: PlanBindingV1,
    predecessor: Option<GroupExecutionIdV1>,
    working_copy_basis: CheckResultInputBasisV2,
    prior_work: Vec<PriorWorkDispositionV1>,
}
/// Closed constructor input; the immutable record never exposes a mutable input reference.
pub struct ActivationIdentityInputV1 {
    pub execution: GroupExecutionIdV1,
    pub session: SessionId,
    pub base_digest: Sha256Digest,
    pub definition_digest: Sha256Digest,
    pub anchor: GraphNodeId,
    pub goal_revision: Option<GoalRevisionNumberV2>,
    pub plan: PlanBindingV1,
    pub predecessor: Option<GroupExecutionIdV1>,
    pub working_copy_basis: CheckResultInputBasisV2,
    pub prior_work: Vec<PriorWorkDispositionV1>,
}
impl ActivationIdentityV1 {
    pub fn new(v: ActivationIdentityInputV1) -> Result<Self, DomainError> {
        if v.goal_revision
            .is_some_and(|revision| revision == GoalRevisionNumberV2::ZERO)
        {
            return Err(invalid("activation goal revision must be positive"));
        }
        if v.predecessor.as_ref() == Some(&v.execution) {
            return Err(invalid("activation predecessor cannot reference itself"));
        }
        if v.prior_work.iter().any(|d| d.group() == &v.execution) {
            return Err(invalid("prior work cannot reference the new activation"));
        }
        if v.prior_work.len() > MAX_PRIOR_WORK_DISPOSITIONS_V1
            || v.predecessor.is_none() != v.prior_work.is_empty()
        {
            return Err(invalid(
                "activation history requires one to 128 prior-work dispositions after the first group",
            ));
        }
        Ok(Self {
            execution: v.execution,
            session: v.session,
            base_digest: v.base_digest,
            definition_digest: v.definition_digest,
            anchor: v.anchor,
            goal_revision: v.goal_revision,
            plan: v.plan,
            predecessor: v.predecessor,
            working_copy_basis: v.working_copy_basis,
            prior_work: v.prior_work,
        })
    }
    pub fn execution(&self) -> &GroupExecutionIdV1 {
        &self.execution
    }
    pub fn session(&self) -> &SessionId {
        &self.session
    }
    pub fn base_digest(&self) -> &Sha256Digest {
        &self.base_digest
    }
    pub fn definition_digest(&self) -> &Sha256Digest {
        &self.definition_digest
    }
    pub fn anchor(&self) -> &GraphNodeId {
        &self.anchor
    }
    pub const fn goal_revision(&self) -> Option<GoalRevisionNumberV2> {
        self.goal_revision
    }
    pub fn plan(&self) -> &PlanBindingV1 {
        &self.plan
    }
    pub fn predecessor(&self) -> Option<&GroupExecutionIdV1> {
        self.predecessor.as_ref()
    }
    pub fn working_copy_basis(&self) -> &CheckResultInputBasisV2 {
        &self.working_copy_basis
    }
    pub fn prior_work(&self) -> &[PriorWorkDispositionV1] {
        &self.prior_work
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupExecutionOutcomeV1 {
    Running,
    Completed,
    Abandoned,
    TerminatedByCancel,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionClosureCauseV1 {
    Replan,
    GoalRevision,
    Reactivation,
    Completion,
    Cancel,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssessmentStageV1 {
    NotScored,
    Lightweight,
    Scored,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssessmentVerdictV1 {
    Success,
    Failure,
    Inconclusive,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssessmentRecordV1 {
    source: ScopedAttemptV1,
    verdict: AssessmentVerdictV1,
    stage: AssessmentStageV1,
    score: u8,
    evidence: Vec<PlanItemBindingV1>,
}
impl AssessmentRecordV1 {
    pub fn new(
        source: ScopedAttemptV1,
        verdict: AssessmentVerdictV1,
        stage: AssessmentStageV1,
        score: u8,
        evidence: Vec<PlanItemBindingV1>,
    ) -> Result<Self, DomainError> {
        if score > 100
            || evidence.is_empty()
            || evidence.len() > 16
            || evidence
                .iter()
                .map(|i| i.item())
                .collect::<BTreeSet<_>>()
                .len()
                != evidence.len()
        {
            return Err(invalid(
                "assessment score or selected evidence exceeds bounds",
            ));
        }
        Ok(Self {
            source,
            verdict,
            stage,
            score,
            evidence,
        })
    }
    pub fn source(&self) -> &ScopedAttemptV1 {
        &self.source
    }
    pub const fn verdict(&self) -> AssessmentVerdictV1 {
        self.verdict
    }
    pub const fn stage(&self) -> AssessmentStageV1 {
        self.stage
    }
    pub const fn score(&self) -> u8 {
        self.score
    }
    pub fn evidence(&self) -> &[PlanItemBindingV1] {
        &self.evidence
    }
}
/// Entry ordinals are facts, not predicates or a quota. The pure transition owner increments them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionEntryV1 {
    target: ScopedAttemptV1,
    selected_group: GroupExecutionIdV1,
    ordinal: u64,
}
impl DecisionEntryV1 {
    pub fn new(
        target: ScopedAttemptV1,
        selected_group: GroupExecutionIdV1,
        ordinal: u64,
    ) -> Result<Self, DomainError> {
        if ordinal == 0 {
            return Err(invalid("decision entry ordinals start at one"));
        }
        if let ExecutionScopeV1::Group {
            group_execution_id, ..
        } = target.scope()
            && group_execution_id != &selected_group
        {
            return Err(invalid("decision entry scope crosses selected groups"));
        }
        Ok(Self {
            target,
            selected_group,
            ordinal,
        })
    }
    pub fn target(&self) -> &ScopedAttemptV1 {
        &self.target
    }
    pub fn selected_group(&self) -> &GroupExecutionIdV1 {
        &self.selected_group
    }
    pub const fn ordinal(&self) -> u64 {
        self.ordinal
    }
}
/// Bounded feedback shared by replan, goal revision and reactivation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupClosureFeedbackV1 {
    reason: String,
    working_copy_basis: CheckResultInputBasisV2,
    feedback: String,
}

impl GroupClosureFeedbackV1 {
    pub fn new(
        reason: String,
        working_copy_basis: CheckResultInputBasisV2,
        feedback: String,
    ) -> Result<Self, DomainError> {
        bounded_text(&reason, 2000)?;
        bounded_text(&feedback, 4000)?;
        Ok(Self {
            reason,
            working_copy_basis,
            feedback,
        })
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
    pub fn working_copy_basis(&self) -> &CheckResultInputBasisV2 {
        &self.working_copy_basis
    }
    pub fn feedback(&self) -> &str {
        &self.feedback
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplanFeedbackV1 {
    decision: ScopedAttemptV1,
    option: OptionId,
    closure: GroupClosureFeedbackV1,
}
impl ReplanFeedbackV1 {
    pub fn new(
        decision: ScopedAttemptV1,
        option: OptionId,
        reason: String,
        working_copy_basis: CheckResultInputBasisV2,
        feedback: String,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            decision,
            option,
            closure: GroupClosureFeedbackV1::new(reason, working_copy_basis, feedback)?,
        })
    }
    pub fn decision(&self) -> &ScopedAttemptV1 {
        &self.decision
    }
    pub fn option(&self) -> &OptionId {
        &self.option
    }
    pub fn reason(&self) -> &str {
        self.closure.reason()
    }
    pub fn working_copy_basis(&self) -> &CheckResultInputBasisV2 {
        self.closure.working_copy_basis()
    }
    pub fn feedback(&self) -> &str {
        self.closure.feedback()
    }
    pub(super) fn into_closure(self) -> GroupClosureFeedbackV1 {
        self.closure
    }
}
