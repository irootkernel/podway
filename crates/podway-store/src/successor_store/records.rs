//! SQLite row payload codecs. The candidate uses the existing item-value encoding.

use super::{Result, corrupt};
use podway_core::execution_groups::*;
use podway_core::workflow_v2::parse_item_type;
use podway_core::{
    ActorAttributionV2, AttemptId, AttemptLifecycle, AttemptValidityV2, BlockerId, BlockerState,
    BlockerStateV2, CheckResultInputBasisV2, CriterionAssessmentReasonV2, CriterionId,
    GoalCriterionV2, GoalDefinitionV2, GoalOutcome, GoalRevisionNumberV2, GoalStatementV2,
    GraphNodeId, ItemId, ItemSlotStateV2, OptionId, ReasonV2, Revision, SessionId,
    SessionLifecycle, Sha256Digest, UnixMillis, recorded_value_digest_v2,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(remote = "SessionLifecycle", rename_all = "snake_case")]
pub(super) enum Lifecycle {
    Prepared,
    Running,
    Completed,
    Cancelled,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "AttemptLifecycle", rename_all = "snake_case")]
enum AttemptState {
    Active,
    Completed,
    Skipped,
    Abandoned,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "AttemptValidityV2", rename_all = "snake_case")]
enum Validity {
    Valid,
    Stale,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "BlockerState", rename_all = "snake_case")]
enum BlockerStatus {
    Open,
    Resolved,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Goal {
    statement: String,
    criteria: Vec<(CriterionId, String)>,
}

impl From<&ExecutionGoalV3> for Goal {
    fn from(goal: &ExecutionGoalV3) -> Self {
        Self {
            statement: goal.statement.as_str().to_owned(),
            criteria: goal
                .criteria
                .criteria()
                .iter()
                .map(|criterion| (criterion.id().clone(), criterion.statement().to_owned()))
                .collect(),
        }
    }
}

impl Goal {
    pub(super) fn admit(self) -> Result<ExecutionGoalV3> {
        Ok(ExecutionGoalV3 {
            statement: GoalStatementV2::new(self.statement)?,
            criteria: GoalDefinitionV2::new(
                self.criteria
                    .into_iter()
                    .map(|(id, statement)| GoalCriterionV2::new(id, statement))
                    .collect::<std::result::Result<_, _>>()?,
            )?,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Basis {
    descriptor: String,
    digest: Sha256Digest,
}

impl From<&CheckResultInputBasisV2> for Basis {
    fn from(basis: &CheckResultInputBasisV2) -> Self {
        Self {
            descriptor: basis.descriptor().to_owned(),
            digest: basis.digest().clone(),
        }
    }
}
impl Basis {
    fn admit(self) -> Result<CheckResultInputBasisV2> {
        Ok(CheckResultInputBasisV2::new(self.descriptor, self.digest)?)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemBinding {
    item: ItemId,
    revision: Revision,
    value_digest: Sha256Digest,
}

impl From<&PlanItemBindingV1> for ItemBinding {
    fn from(binding: &PlanItemBindingV1) -> Self {
        Self {
            item: binding.item().clone(),
            revision: binding.revision(),
            value_digest: binding.value_digest().clone(),
        }
    }
}
impl ItemBinding {
    fn admit(self) -> Result<PlanItemBindingV1> {
        Ok(PlanItemBindingV1::new(
            self.item,
            self.revision,
            self.value_digest,
        )?)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Disposition {
    group: GroupExecutionIdV1,
    item: Option<HistoricalReferenceV1>,
    choice: PriorWorkChoiceV1,
    rationale: String,
}

impl From<&PriorWorkDispositionV1> for Disposition {
    fn from(value: &PriorWorkDispositionV1) -> Self {
        Self {
            group: value.group().clone(),
            item: value.item().cloned(),
            choice: value.choice(),
            rationale: value.rationale().to_owned(),
        }
    }
}
impl Disposition {
    fn admit(self) -> Result<PriorWorkDispositionV1> {
        Ok(PriorWorkDispositionV1::new(
            self.group,
            self.item,
            self.choice,
            self.rationale,
        )?)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Feedback {
    reason: String,
    basis: Basis,
    feedback: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Closure {
    cause: SelectionClosureCauseV1,
    source: ScopedAttemptV1,
    at: UnixMillis,
    feedback: Option<Feedback>,
}

impl From<&GroupSelectionClosureV1> for Closure {
    fn from(value: &GroupSelectionClosureV1) -> Self {
        Self {
            cause: value.cause,
            source: value.source.clone(),
            at: value.at,
            feedback: value.feedback.as_ref().map(|feedback| Feedback {
                reason: feedback.reason().to_owned(),
                basis: feedback.working_copy_basis().into(),
                feedback: feedback.feedback().to_owned(),
            }),
        }
    }
}
impl Closure {
    fn admit(self) -> Result<GroupSelectionClosureV1> {
        let feedback = match self.feedback {
            Some(feedback) => Some(GroupClosureFeedbackV1::new(
                feedback.reason,
                feedback.basis.admit()?,
                feedback.feedback,
            )?),
            None => None,
        };
        Ok(GroupSelectionClosureV1 {
            cause: self.cause,
            source: self.source,
            at: self.at,
            feedback,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Activation {
    execution: GroupExecutionIdV1,
    session: SessionId,
    base_digest: Sha256Digest,
    definition_digest: Sha256Digest,
    anchor: GraphNodeId,
    goal_revision: Option<u64>,
    design_attempt: AttemptId,
    item_set_digest: Sha256Digest,
    plan_items: Vec<ItemBinding>,
    predecessor: Option<GroupExecutionIdV1>,
    working_copy_basis: Basis,
    prior_work: Vec<Disposition>,
    outcome: GroupExecutionOutcomeV1,
    terminal_attempt: Option<AttemptId>,
    closure: Option<Closure>,
}

impl From<&GroupExecutionStateV1> for Activation {
    fn from(value: &GroupExecutionStateV1) -> Self {
        let identity = value.identity();
        Self {
            execution: identity.execution().clone(),
            session: identity.session().clone(),
            base_digest: identity.base_digest().clone(),
            definition_digest: identity.definition_digest().clone(),
            anchor: identity.anchor().clone(),
            goal_revision: identity.goal_revision().map(GoalRevisionNumberV2::get),
            design_attempt: identity.plan().design_attempt().clone(),
            item_set_digest: identity.plan().item_set_digest().clone(),
            plan_items: identity.plan().items().iter().map(Into::into).collect(),
            predecessor: identity.predecessor().cloned(),
            working_copy_basis: identity.working_copy_basis().into(),
            prior_work: identity.prior_work().iter().map(Into::into).collect(),
            outcome: value.outcome(),
            terminal_attempt: value.terminal_attempt().cloned(),
            closure: value.closure().map(Into::into),
        }
    }
}

impl Activation {
    pub(super) fn admit(self) -> Result<RetainedActivationV3> {
        Ok(RetainedActivationV3 {
            identity: ActivationIdentityV1::new(ActivationIdentityInputV1 {
                execution: self.execution,
                session: self.session,
                base_digest: self.base_digest,
                definition_digest: self.definition_digest,
                anchor: self.anchor,
                goal_revision: self.goal_revision.map(GoalRevisionNumberV2::new),
                plan: PlanBindingV1::new(
                    self.design_attempt,
                    self.item_set_digest,
                    self.plan_items
                        .into_iter()
                        .map(ItemBinding::admit)
                        .collect::<Result<_>>()?,
                )?,
                predecessor: self.predecessor,
                working_copy_basis: self.working_copy_basis.admit()?,
                prior_work: self
                    .prior_work
                    .into_iter()
                    .map(Disposition::admit)
                    .collect::<Result<_>>()?,
            })?,
            outcome: self.outcome,
            terminal_attempt: self.terminal_attempt,
            closure: self.closure.map(Closure::admit).transpose()?,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GoalChange {
    revision: u64,
    goal: Goal,
    actor: Option<String>,
    source: ScopedAttemptV1,
    successor: ScopedAttemptV1,
    predecessor: Option<GroupExecutionIdV1>,
    closure: Closure,
}

impl From<&GroupGoalRevisionV1> for GoalChange {
    fn from(value: &GroupGoalRevisionV1) -> Self {
        Self {
            revision: value.revision.get(),
            goal: (&value.goal).into(),
            actor: value.actor.as_ref().map(|value| value.as_str().to_owned()),
            source: value.source.clone(),
            successor: value.successor.clone(),
            predecessor: value.predecessor.clone(),
            closure: (&value.closure).into(),
        }
    }
}
impl GoalChange {
    pub(super) fn admit(self) -> Result<GroupGoalRevisionV1> {
        Ok(GroupGoalRevisionV1 {
            revision: GoalRevisionNumberV2::new(self.revision),
            goal: self.goal.admit()?,
            actor: self
                .actor
                .map(podway_core::ActorAttributionV2::new)
                .transpose()?,
            source: self.source,
            successor: self.successor,
            predecessor: self.predecessor,
            closure: self.closure.admit()?,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Slot {
    item: ItemId,
    kind: String,
    revision: Revision,
    value: Option<String>,
    value_digest: Option<Sha256Digest>,
    updated_at: UnixMillis,
}

impl Slot {
    fn capture(value: &ItemSlotStateV2) -> Result<Self> {
        Ok(Self {
            item: value.item_id().clone(),
            kind: value.item_type().wire_name().to_owned(),
            revision: value.revision(),
            value: value
                .value()
                .map(crate::v2_memory::encode_item_value_v2)
                .transpose()?,
            value_digest: value
                .value()
                .map(recorded_value_digest_v2)
                .transpose()
                .map_err(|_| corrupt("unencodable item digest"))?,
            updated_at: value.updated_at(),
        })
    }
    fn admit(self, attempt: &AttemptId, created_at: UnixMillis) -> Result<ItemSlotStateV2> {
        let kind =
            parse_item_type(&self.kind).map_err(|_| corrupt("unknown retained item type"))?;
        let value = self
            .value
            .as_deref()
            .map(|value| crate::v2_memory::decode_item_value_v2(value, kind))
            .transpose()?;
        if value
            .as_ref()
            .map(recorded_value_digest_v2)
            .transpose()
            .map_err(|_| corrupt("unencodable item digest"))?
            != self.value_digest
        {
            return Err(corrupt("retained item digest differs from its value"));
        }
        ItemSlotStateV2::new(
            attempt.clone(),
            self.item,
            kind,
            self.revision,
            value,
            created_at,
            self.updated_at,
        )
        .map_err(|_| corrupt("invalid retained item slot"))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Blocker {
    id: BlockerId,
    reason: String,
    #[serde(with = "BlockerStatus")]
    state: BlockerState,
    created_at: UnixMillis,
    resolved_at: Option<UnixMillis>,
}
impl From<&BlockerStateV2> for Blocker {
    fn from(value: &BlockerStateV2) -> Self {
        Self {
            id: value.blocker_id().clone(),
            reason: value.reason().to_owned(),
            state: value.state(),
            created_at: value.created_at(),
            resolved_at: value.resolved_at(),
        }
    }
}
impl Blocker {
    fn admit(self, attempt: &AttemptId) -> Result<BlockerStateV2> {
        BlockerStateV2::new(
            self.id,
            attempt.clone(),
            self.reason,
            self.state,
            self.created_at,
            self.resolved_at,
        )
        .map_err(|_| corrupt("invalid retained blocker"))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    option: OptionId,
    reason: String,
    guard_values: Vec<(ScopedAttemptV1, ItemBinding)>,
    evidence_sources: Vec<Option<ScopedAttemptV1>>,
    goal_assessment: Option<(u64, String)>,
}
impl From<&GroupDecisionRecordV1> for Decision {
    fn from(value: &GroupDecisionRecordV1) -> Self {
        Self {
            option: value.option.clone(),
            reason: value.reason.clone(),
            guard_values: value
                .guard_values
                .iter()
                .map(|guard| (guard.source.clone(), (&guard.item).into()))
                .collect(),
            evidence_sources: value.evidence_sources.clone(),
            goal_assessment: value
                .goal_assessment
                .map(|(revision, outcome)| (revision.get(), outcome.as_str().to_owned())),
        }
    }
}
impl Decision {
    fn admit(self) -> Result<GroupDecisionRecordV1> {
        if self.evidence_sources.len() > podway_core::MAX_EVIDENCE_REFERENCES_PER_PLACEMENT_V2 {
            return Err(corrupt(
                "retained Decision evidence exceeds its source bound",
            ));
        }
        Ok(GroupDecisionRecordV1 {
            option: self.option,
            reason: self.reason,
            guard_values: self
                .guard_values
                .into_iter()
                .map(|(source, item)| {
                    Ok(GroupGuardValueV1 {
                        source,
                        item: item.admit()?,
                    })
                })
                .collect::<Result<_>>()?,
            evidence_sources: self.evidence_sources,
            goal_assessment: self
                .goal_assessment
                .map(|(revision, outcome)| {
                    outcome
                        .parse::<GoalOutcome>()
                        .map(|outcome| (GoalRevisionNumberV2::new(revision), outcome))
                })
                .transpose()?,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CriterionCitation {
    Local {
        node: GraphNodeId,
        target: ScopedAttemptV1,
    },
    Plan {
        target: ScopedAttemptV1,
    },
    ExecutionResult {
        target: ScopedAttemptV1,
    },
    Item {
        item: ItemId,
    },
}

impl From<&RetainedCriterionCitationV1> for CriterionCitation {
    fn from(value: &RetainedCriterionCitationV1) -> Self {
        match value {
            RetainedCriterionCitationV1::Evidence { source, target } => match source {
                EvidenceSourceV3::Local(node) => Self::Local {
                    node: node.clone(),
                    target: target.clone(),
                },
                EvidenceSourceV3::Plan => Self::Plan {
                    target: target.clone(),
                },
                EvidenceSourceV3::ExecutionResult => Self::ExecutionResult {
                    target: target.clone(),
                },
            },
            RetainedCriterionCitationV1::Item(item) => Self::Item { item: item.clone() },
        }
    }
}

impl From<CriterionCitation> for RetainedCriterionCitationV1 {
    fn from(value: CriterionCitation) -> Self {
        match value {
            CriterionCitation::Local { node, target } => Self::Evidence {
                source: EvidenceSourceV3::Local(node),
                target,
            },
            CriterionCitation::Plan { target } => Self::Evidence {
                source: EvidenceSourceV3::Plan,
                target,
            },
            CriterionCitation::ExecutionResult { target } => Self::Evidence {
                source: EvidenceSourceV3::ExecutionResult,
                target,
            },
            CriterionCitation::Item { item } => Self::Item(item),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CriterionRow {
    criterion: CriterionId,
    status: String,
    reason: String,
    citations: Vec<CriterionCitation>,
    actor: Option<String>,
    recorded_at: UnixMillis,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CriterionSet {
    goal_revision: u64,
    results: Vec<CriterionRow>,
}

impl From<&SuccessorCriterionSetV1> for CriterionSet {
    fn from(value: &SuccessorCriterionSetV1) -> Self {
        Self {
            goal_revision: value.goal_revision.get(),
            results: value
                .results
                .iter()
                .map(|row| CriterionRow {
                    criterion: row.criterion.clone(),
                    status: row.status.as_str().to_owned(),
                    reason: row.reason.as_str().to_owned(),
                    citations: row.citations.iter().map(Into::into).collect(),
                    actor: row.actor.as_ref().map(|actor| actor.as_str().to_owned()),
                    recorded_at: row.recorded_at,
                })
                .collect(),
        }
    }
}

impl CriterionSet {
    fn admit(self) -> Result<SuccessorCriterionSetV1> {
        Ok(SuccessorCriterionSetV1 {
            goal_revision: GoalRevisionNumberV2::new(self.goal_revision),
            results: self
                .results
                .into_iter()
                .map(|row| {
                    Ok(SuccessorCriterionResultV1 {
                        criterion: row.criterion,
                        status: row.status.parse()?,
                        reason: CriterionAssessmentReasonV2::new(row.reason)?,
                        citations: row.citations.into_iter().map(Into::into).collect(),
                        actor: row.actor.map(ActorAttributionV2::new).transpose()?,
                        recorded_at: row.recorded_at,
                    })
                })
                .collect::<Result<_>>()?,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attempt {
    target: ScopedAttemptV1,
    #[serde(with = "AttemptState")]
    lifecycle: AttemptLifecycle,
    #[serde(with = "Validity")]
    validity: AttemptValidityV2,
    opened_at: UnixMillis,
    closed_at: Option<UnixMillis>,
    closed_actor: Option<String>,
    slots: Vec<Slot>,
    blockers: Vec<Blocker>,
    delegation: Option<ScopedDelegationBindingV1>,
    decision_entry: Option<u64>,
    decision_group: Option<GroupExecutionIdV1>,
    decision: Option<Decision>,
    criterion_assessments: Option<CriterionSet>,
    reason: Option<String>,
}

impl Attempt {
    pub(super) fn require_preserved_on_write(
        mut self,
        successor: &GroupAttemptStateV1,
    ) -> Result<()> {
        if self.lifecycle == AttemptLifecycle::Active {
            return Ok(());
        }
        if self.validity == AttemptValidityV2::Valid
            && successor.validity() == AttemptValidityV2::Stale
        {
            self.validity = AttemptValidityV2::Stale;
        }
        if self.admit()? != RetainedAttemptV3::from(successor) {
            return Err(corrupt("closed retained attempt payload differs"));
        }
        Ok(())
    }

    pub(super) fn capture(value: &GroupAttemptStateV1) -> Result<Self> {
        Ok(Self {
            target: value.target().clone(),
            lifecycle: value.lifecycle(),
            validity: value.validity(),
            opened_at: value.opened_at(),
            closed_at: value.closed_at(),
            closed_actor: value.closed_actor().map(|actor| actor.as_str().to_owned()),
            slots: value
                .slots()
                .iter()
                .map(Slot::capture)
                .collect::<Result<_>>()?,
            blockers: value.blockers().iter().map(Into::into).collect(),
            delegation: value.delegation().cloned(),
            decision_entry: value.decision_entry(),
            decision_group: value
                .selected_decision_entry()
                .map(|entry| entry.selected_group().clone()),
            decision: value.decision().map(Into::into),
            criterion_assessments: value.criterion_assessments().map(Into::into),
            reason: value.reason().map(|reason| reason.as_str().to_owned()),
        })
    }
    pub(super) fn admit(self) -> Result<RetainedAttemptV3> {
        let id = self.target.attempt();
        let slots = self
            .slots
            .into_iter()
            .map(|slot| slot.admit(id, self.opened_at))
            .collect::<Result<_>>()?;
        let blockers = self
            .blockers
            .into_iter()
            .map(|blocker| blocker.admit(id))
            .collect::<Result<_>>()?;
        Ok(RetainedAttemptV3 {
            target: self.target,
            lifecycle: self.lifecycle,
            validity: self.validity,
            opened_at: self.opened_at,
            closed_at: self.closed_at,
            closed_actor: self
                .closed_actor
                .map(podway_core::ActorAttributionV2::new)
                .transpose()?,
            slots,
            blockers,
            delegation: self.delegation,
            decision_entry: self.decision_entry,
            decision_group: self.decision_group,
            decision: self.decision.map(Decision::admit).transpose()?,
            criterion_assessments: self
                .criterion_assessments
                .map(CriterionSet::admit)
                .transpose()?,
            reason: self.reason.map(ReasonV2::new).transpose()?,
        })
    }
}
