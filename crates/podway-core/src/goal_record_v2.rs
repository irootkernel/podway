//! Pure Procedure v2 goal-tracking record values: session goal revisions, criterion assessment
//! results, and final goal assessment records.
//!
//! These additive immutable values compose the V2MOD-001 bounded goal values (`GoalStatementV2`,
//! `GoalDefinitionV2`, `GoalRevisionReasonV2`, `CriterionAssessmentReasonV2`, `GoalOutcome`) with
//! the V2MOD-002 monotonic identities and the V2MOD-003 resolved-evidence snapshot and actor
//! attribution. They follow ADR-0016 and dossier sections 4.5, 7.2, 7.3, and 7.4. Criterion
//! assessment state is attempt-local while active; these are its immutable record forms. Podway
//! validates identity, freshness, mode, citation target, and shape; it never judges the semantic
//! truth of a criterion result (dossier section 7.3).

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use crate::procedure_v2::{
    CriterionAssessmentReasonV2, GoalDefinitionV2, GoalOutcome, GoalRevisionReasonV2,
    GoalStatementV2,
};
use crate::record_v2::{ActorAttributionV2, ResolvedEvidenceSetV2};
use crate::session_v2::{GoalRevisionNumberV2, TraceSequenceV2};
use crate::{AttemptId, CriterionId, DomainError, GraphNodeId, ItemId, UnixMillis};

const MIN_GOAL_ASSESSMENT_RESULTS: usize = 1;
const MAX_GOAL_ASSESSMENT_RESULTS: usize = 16;
const MAX_CRITERION_CITATIONS: usize = 4;

const fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}

/// An immutable session goal revision record (dossier sections 4.5 and 7.2). Revisions are
/// monotonically numbered. The first revision carries no predecessor, reason, or rework target and
/// never reactivates a session; every later revision requires all three. Each revision records its
/// binding trace sequence — the active attempt it binds (define) or the fresh target attempt that
/// its rework transaction activates (revise). Earlier revisions are never mutated or deleted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalRevisionRecordV2 {
    revision: GoalRevisionNumberV2,
    predecessor: Option<GoalRevisionNumberV2>,
    statement: GoalStatementV2,
    criteria: GoalDefinitionV2,
    reason: Option<GoalRevisionReasonV2>,
    rework_to: Option<GraphNodeId>,
    reactivated: bool,
    actor: Option<ActorAttributionV2>,
    binding_trace: TraceSequenceV2,
    created_at: UnixMillis,
}

impl GoalRevisionRecordV2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        revision: GoalRevisionNumberV2,
        predecessor: Option<GoalRevisionNumberV2>,
        statement: GoalStatementV2,
        criteria: GoalDefinitionV2,
        reason: Option<GoalRevisionReasonV2>,
        rework_to: Option<GraphNodeId>,
        reactivated: bool,
        actor: Option<ActorAttributionV2>,
        binding_trace: TraceSequenceV2,
        created_at: UnixMillis,
    ) -> Result<Self, DomainError> {
        if revision < GoalRevisionNumberV2::FIRST {
            return Err(invalid("goal revision must be nonzero"));
        }
        if binding_trace < TraceSequenceV2::FIRST {
            return Err(invalid("goal revision binding trace must be nonzero"));
        }
        let is_first = revision == GoalRevisionNumberV2::FIRST;
        if is_first {
            if predecessor.is_some() || reason.is_some() || rework_to.is_some() {
                return Err(invalid(
                    "the first goal revision carries no predecessor, reason, or rework target",
                ));
            }
            if reactivated {
                return Err(invalid(
                    "the first goal revision cannot reactivate a session",
                ));
            }
        } else if predecessor.is_none() || reason.is_none() || rework_to.is_none() {
            return Err(invalid(
                "a later goal revision requires a predecessor, reason, and rework target",
            ));
        }
        Ok(Self {
            revision,
            predecessor,
            statement,
            criteria,
            reason,
            rework_to,
            reactivated,
            actor,
            binding_trace,
            created_at,
        })
    }

    pub const fn revision(&self) -> GoalRevisionNumberV2 {
        self.revision
    }

    pub fn predecessor(&self) -> Option<GoalRevisionNumberV2> {
        self.predecessor
    }

    pub fn statement(&self) -> &GoalStatementV2 {
        &self.statement
    }

    pub fn criteria(&self) -> &GoalDefinitionV2 {
        &self.criteria
    }

    pub fn reason(&self) -> Option<&GoalRevisionReasonV2> {
        self.reason.as_ref()
    }

    pub fn rework_to(&self) -> Option<&GraphNodeId> {
        self.rework_to.as_ref()
    }

    pub const fn reactivated(&self) -> bool {
        self.reactivated
    }

    pub fn actor(&self) -> Option<&ActorAttributionV2> {
        self.actor.as_ref()
    }

    pub const fn binding_trace(&self) -> TraceSequenceV2 {
        self.binding_trace
    }

    pub const fn created_at(&self) -> UnixMillis {
        self.created_at
    }
}

/// The recorded status of one criterion assessment (dossier section 7.3). Statuses are homogeneous
/// within one assessment attempt: `satisfied` or `unsatisfied` fix assessment mode, and
/// `not_applicable` fixes applicability mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CriterionStatusV2 {
    Satisfied,
    Unsatisfied,
    NotApplicable,
}

impl CriterionStatusV2 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Satisfied => "satisfied",
            Self::Unsatisfied => "unsatisfied",
            Self::NotApplicable => "not_applicable",
        }
    }
}

impl FromStr for CriterionStatusV2 {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "satisfied" => Ok(Self::Satisfied),
            "unsatisfied" => Ok(Self::Unsatisfied),
            "not_applicable" => Ok(Self::NotApplicable),
            _ => Err(invalid("unknown criterion assessment status")),
        }
    }
}

impl fmt::Display for CriterionStatusV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One citation recorded against a criterion result (dossier section 7.3). A citation names either a
/// resolved evidence reference of the active decision attempt, by its source graph node id, or a
/// decision-local item persisted on that attempt. A `not_applicable` result must not cite either.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CriterionCitationV2 {
    Evidence(GraphNodeId),
    Item(ItemId),
}

impl CriterionCitationV2 {
    pub fn as_evidence(&self) -> Option<&GraphNodeId> {
        match self {
            Self::Evidence(node) => Some(node),
            Self::Item(_) => None,
        }
    }

    pub fn as_item(&self) -> Option<&ItemId> {
        match self {
            Self::Evidence(_) => None,
            Self::Item(item) => Some(item),
        }
    }
}

/// The homogeneous assessment mode fixed by the first recorded criterion result of an assessment
/// attempt (dossier section 7.3). Assessment mode admits `satisfied` or `unsatisfied`; applicability
/// mode admits `not_applicable` only.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CriterionAssessmentModeV2 {
    Assessment,
    Applicability,
}

impl CriterionAssessmentModeV2 {
    pub const fn from_status(status: CriterionStatusV2) -> Self {
        match status {
            CriterionStatusV2::Satisfied | CriterionStatusV2::Unsatisfied => Self::Assessment,
            CriterionStatusV2::NotApplicable => Self::Applicability,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Assessment => "assessment",
            Self::Applicability => "applicability",
        }
    }
}

/// One immutable criterion assessment result (dossier section 7.3). The reason is always required.
/// A result carries at most four citations; a `not_applicable` result carries none, because a
/// superseded criterion makes no supported claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CriterionAssessmentResultV2 {
    criterion_id: CriterionId,
    status: CriterionStatusV2,
    reason: CriterionAssessmentReasonV2,
    citations: Vec<CriterionCitationV2>,
}

impl CriterionAssessmentResultV2 {
    pub fn new(
        criterion_id: CriterionId,
        status: CriterionStatusV2,
        reason: CriterionAssessmentReasonV2,
        citations: Vec<CriterionCitationV2>,
    ) -> Result<Self, DomainError> {
        if citations.len() > MAX_CRITERION_CITATIONS {
            return Err(invalid("a criterion result carries at most four citations"));
        }
        if status == CriterionStatusV2::NotApplicable && !citations.is_empty() {
            return Err(invalid(
                "a not_applicable criterion result must not cite evidence",
            ));
        }
        Ok(Self {
            criterion_id,
            status,
            reason,
            citations,
        })
    }

    pub fn criterion_id(&self) -> &CriterionId {
        &self.criterion_id
    }

    pub const fn status(&self) -> CriterionStatusV2 {
        self.status
    }

    pub fn reason(&self) -> &CriterionAssessmentReasonV2 {
        &self.reason
    }

    pub fn citations(&self) -> &[CriterionCitationV2] {
        &self.citations
    }

    pub const fn mode(&self) -> CriterionAssessmentModeV2 {
        CriterionAssessmentModeV2::from_status(self.status)
    }
}

/// An immutable final goal assessment record (dossier section 7.4), created atomically with the
/// ordinary decision record of a session-goal assessment decision. The criterion results are
/// homogeneous and determine exactly one outcome; the record enforces that outcome–results
/// agreement so a complete assessment set can never record a contradictory outcome. The assessment
/// remains reportable for the session lifetime even after its attempt or a source attempt is stale.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalAssessmentRecordV2 {
    goal_revision: GoalRevisionNumberV2,
    outcome: GoalOutcome,
    criterion_results: Vec<CriterionAssessmentResultV2>,
    evidence: ResolvedEvidenceSetV2,
    actor: Option<ActorAttributionV2>,
    decision_attempt_id: AttemptId,
    decision_graph_node_id: GraphNodeId,
    decision_trace: TraceSequenceV2,
    recorded_at: UnixMillis,
}

impl GoalAssessmentRecordV2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        goal_revision: GoalRevisionNumberV2,
        outcome: GoalOutcome,
        criterion_results: Vec<CriterionAssessmentResultV2>,
        evidence: ResolvedEvidenceSetV2,
        actor: Option<ActorAttributionV2>,
        decision_attempt_id: AttemptId,
        decision_graph_node_id: GraphNodeId,
        decision_trace: TraceSequenceV2,
        recorded_at: UnixMillis,
    ) -> Result<Self, DomainError> {
        validate_assessment_consistency(goal_revision, outcome, &criterion_results)?;
        if decision_trace < TraceSequenceV2::FIRST {
            return Err(invalid("goal assessment decision trace must be nonzero"));
        }
        Ok(Self {
            goal_revision,
            outcome,
            criterion_results,
            evidence,
            actor,
            decision_attempt_id,
            decision_graph_node_id,
            decision_trace,
            recorded_at,
        })
    }

    pub const fn goal_revision(&self) -> GoalRevisionNumberV2 {
        self.goal_revision
    }

    pub const fn outcome(&self) -> GoalOutcome {
        self.outcome
    }

    pub fn criterion_results(&self) -> &[CriterionAssessmentResultV2] {
        &self.criterion_results
    }

    pub fn mode(&self) -> CriterionAssessmentModeV2 {
        // The constructor guarantees a non-empty homogeneous set, so the first result fixes the mode.
        CriterionAssessmentModeV2::from_status(self.criterion_results[0].status())
    }

    pub fn evidence(&self) -> &ResolvedEvidenceSetV2 {
        &self.evidence
    }

    pub fn actor(&self) -> Option<&ActorAttributionV2> {
        self.actor.as_ref()
    }

    pub fn decision_attempt_id(&self) -> &AttemptId {
        &self.decision_attempt_id
    }

    pub fn decision_graph_node_id(&self) -> &GraphNodeId {
        &self.decision_graph_node_id
    }

    pub const fn decision_trace(&self) -> TraceSequenceV2 {
        self.decision_trace
    }

    pub const fn recorded_at(&self) -> UnixMillis {
        self.recorded_at
    }
}

fn validate_assessment_consistency(
    goal_revision: GoalRevisionNumberV2,
    outcome: GoalOutcome,
    results: &[CriterionAssessmentResultV2],
) -> Result<(), DomainError> {
    if goal_revision < GoalRevisionNumberV2::FIRST {
        return Err(invalid("goal assessment goal revision must be nonzero"));
    }
    if results.len() < MIN_GOAL_ASSESSMENT_RESULTS || results.len() > MAX_GOAL_ASSESSMENT_RESULTS {
        return Err(invalid(
            "a goal assessment records between one and sixteen criterion results",
        ));
    }
    let mut seen: BTreeSet<&CriterionId> = BTreeSet::new();
    for result in results {
        if !seen.insert(result.criterion_id()) {
            return Err(invalid(
                "goal assessment criterion identifiers must be unique",
            ));
        }
    }
    let mode = CriterionAssessmentModeV2::from_status(results[0].status());
    for result in results {
        if result.mode() != mode {
            return Err(invalid(
                "criterion assessment modes must be homogeneous within one assessment",
            ));
        }
    }
    match outcome {
        GoalOutcome::Achieved => {
            if mode != CriterionAssessmentModeV2::Assessment {
                return Err(invalid("achieved requires assessment mode"));
            }
            if !results
                .iter()
                .all(|result| result.status() == CriterionStatusV2::Satisfied)
            {
                return Err(invalid("achieved requires every criterion satisfied"));
            }
        }
        GoalOutcome::NotAchieved => {
            if mode != CriterionAssessmentModeV2::Assessment {
                return Err(invalid("not_achieved requires assessment mode"));
            }
            if !results
                .iter()
                .any(|result| result.status() == CriterionStatusV2::Unsatisfied)
            {
                return Err(invalid(
                    "not_achieved requires at least one unsatisfied criterion",
                ));
            }
        }
        GoalOutcome::Superseded => {
            if mode != CriterionAssessmentModeV2::Applicability {
                return Err(invalid("superseded requires applicability mode"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procedure_v2::{GoalCriterionV2, GoalDefinitionV2, GoalStatementV2};
    use crate::record_v2::ResolvedEvidenceSetV2;
    use crate::session_v2::{GoalRevisionNumberV2, TraceSequenceV2};
    use crate::{AttemptId, CriterionId, GraphNodeId, ItemId, UnixMillis};

    fn criterion_id(value: &str) -> CriterionId {
        CriterionId::new(value).unwrap()
    }

    fn graph_node(value: &str) -> GraphNodeId {
        GraphNodeId::new(value).unwrap()
    }

    fn attempt_id(n: u64) -> AttemptId {
        AttemptId::new(format!("00000000-0000-0000-0000-{n:012x}")).unwrap()
    }

    fn statement() -> GoalStatementV2 {
        GoalStatementV2::new("Cancellation is deterministic.").unwrap()
    }

    fn criteria() -> GoalDefinitionV2 {
        GoalDefinitionV2::new(vec![
            GoalCriterionV2::new(criterion_id("deterministic"), "One outcome.").unwrap(),
            GoalCriterionV2::new(criterion_id("recoverable"), "Restart preserves it.").unwrap(),
        ])
        .unwrap()
    }

    fn empty_evidence() -> ResolvedEvidenceSetV2 {
        ResolvedEvidenceSetV2::new(Vec::new()).unwrap()
    }

    fn assessment_result(
        criterion: &str,
        status: CriterionStatusV2,
    ) -> CriterionAssessmentResultV2 {
        CriterionAssessmentResultV2::new(
            criterion_id(criterion),
            status,
            crate::procedure_v2::CriterionAssessmentReasonV2::new("Supported by the test run.")
                .unwrap(),
            Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn criterion_status_and_mode_round_trip_strings() {
        assert_eq!(CriterionStatusV2::Satisfied.as_str(), "satisfied");
        assert_eq!(
            "not_applicable".parse::<CriterionStatusV2>().unwrap(),
            CriterionStatusV2::NotApplicable
        );
        assert_eq!(
            "missing".parse::<CriterionStatusV2>().unwrap_err(),
            invalid("unknown criterion assessment status")
        );
        assert_eq!(
            CriterionAssessmentModeV2::from_status(CriterionStatusV2::Unsatisfied).as_str(),
            "assessment"
        );
        assert_eq!(
            CriterionAssessmentModeV2::from_status(CriterionStatusV2::NotApplicable).as_str(),
            "applicability"
        );
    }

    #[test]
    fn criterion_citation_distinguishes_evidence_from_local_item() {
        let evidence = CriterionCitationV2::Evidence(graph_node("test-after-review"));
        let item = CriterionCitationV2::Item(ItemId::new("assessment-note").unwrap());
        assert_eq!(
            evidence.as_evidence(),
            Some(&graph_node("test-after-review"))
        );
        assert!(evidence.as_item().is_none());
        assert_eq!(
            item.as_item(),
            Some(&ItemId::new("assessment-note").unwrap())
        );
        assert!(item.as_evidence().is_none());
    }

    #[test]
    fn criterion_result_bounds_citations_and_forbids_them_for_not_applicable() {
        let four: Vec<CriterionCitationV2> = (0..4)
            .map(|i| CriterionCitationV2::Item(ItemId::new(format!("note-{i}")).unwrap()))
            .collect();
        assert!(
            CriterionAssessmentResultV2::new(
                criterion_id("c"),
                CriterionStatusV2::Satisfied,
                crate::procedure_v2::CriterionAssessmentReasonV2::new("ok").unwrap(),
                four,
            )
            .is_ok()
        );

        let many: Vec<CriterionCitationV2> = (0..5)
            .map(|i| CriterionCitationV2::Item(ItemId::new(format!("note-{i}")).unwrap()))
            .collect();
        assert_eq!(
            CriterionAssessmentResultV2::new(
                criterion_id("c"),
                CriterionStatusV2::Satisfied,
                crate::procedure_v2::CriterionAssessmentReasonV2::new("ok").unwrap(),
                many,
            )
            .unwrap_err(),
            invalid("a criterion result carries at most four citations")
        );

        let not_applicable_with_citation = CriterionAssessmentResultV2::new(
            criterion_id("c"),
            CriterionStatusV2::NotApplicable,
            crate::procedure_v2::CriterionAssessmentReasonV2::new("superseded").unwrap(),
            vec![CriterionCitationV2::Evidence(graph_node("source"))],
        )
        .unwrap_err();
        assert_eq!(
            not_applicable_with_citation,
            invalid("a not_applicable criterion result must not cite evidence")
        );

        let result = assessment_result("c", CriterionStatusV2::Satisfied);
        assert_eq!(result.mode(), CriterionAssessmentModeV2::Assessment);
        assert_eq!(result.citations().len(), 0);
    }

    #[test]
    fn first_goal_revision_has_no_predecessor_reason_or_rework_target() {
        let first = GoalRevisionRecordV2::new(
            GoalRevisionNumberV2::FIRST,
            None,
            statement(),
            criteria(),
            None,
            None,
            false,
            None,
            TraceSequenceV2::FIRST,
            UnixMillis::new(1),
        )
        .unwrap();
        assert_eq!(first.revision(), GoalRevisionNumberV2::FIRST);
        assert!(first.predecessor().is_none());
        assert!(first.reason().is_none());
        assert!(first.rework_to().is_none());
        assert!(!first.reactivated());

        assert_eq!(
            GoalRevisionRecordV2::new(
                GoalRevisionNumberV2::FIRST,
                Some(GoalRevisionNumberV2::ZERO),
                statement(),
                criteria(),
                None,
                None,
                false,
                None,
                TraceSequenceV2::FIRST,
                UnixMillis::new(1),
            )
            .unwrap_err(),
            invalid("the first goal revision carries no predecessor, reason, or rework target")
        );
        assert_eq!(
            GoalRevisionRecordV2::new(
                GoalRevisionNumberV2::FIRST,
                None,
                statement(),
                criteria(),
                None,
                None,
                true,
                None,
                TraceSequenceV2::FIRST,
                UnixMillis::new(1),
            )
            .unwrap_err(),
            invalid("the first goal revision cannot reactivate a session")
        );
    }

    #[test]
    fn later_goal_revision_requires_predecessor_reason_and_rework_target() {
        let later = GoalRevisionRecordV2::new(
            GoalRevisionNumberV2::new(2),
            Some(GoalRevisionNumberV2::FIRST),
            statement(),
            criteria(),
            Some(crate::procedure_v2::GoalRevisionReasonV2::new("Now includes restart.").unwrap()),
            Some(graph_node("implement")),
            true,
            None,
            TraceSequenceV2::new(9),
            UnixMillis::new(20),
        )
        .unwrap();
        assert_eq!(later.revision(), GoalRevisionNumberV2::new(2));
        assert_eq!(later.predecessor(), Some(GoalRevisionNumberV2::FIRST));
        assert!(later.reason().is_some());
        assert_eq!(later.rework_to(), Some(&graph_node("implement")));
        assert!(later.reactivated());

        assert_eq!(
            GoalRevisionRecordV2::new(
                GoalRevisionNumberV2::new(2),
                None,
                statement(),
                criteria(),
                Some(crate::procedure_v2::GoalRevisionReasonV2::new("x").unwrap()),
                Some(graph_node("implement")),
                false,
                None,
                TraceSequenceV2::new(9),
                UnixMillis::new(20),
            )
            .unwrap_err(),
            invalid("a later goal revision requires a predecessor, reason, and rework target")
        );
        assert_eq!(
            GoalRevisionRecordV2::new(
                GoalRevisionNumberV2::ZERO,
                None,
                statement(),
                criteria(),
                None,
                None,
                false,
                None,
                TraceSequenceV2::FIRST,
                UnixMillis::new(1),
            )
            .unwrap_err(),
            invalid("goal revision must be nonzero")
        );
    }

    fn assessment_attempt(
        outcome: GoalOutcome,
        results: Vec<CriterionAssessmentResultV2>,
    ) -> Result<GoalAssessmentRecordV2, DomainError> {
        GoalAssessmentRecordV2::new(
            GoalRevisionNumberV2::FIRST,
            outcome,
            results,
            empty_evidence(),
            None,
            attempt_id(3),
            graph_node("assess-goal"),
            TraceSequenceV2::FIRST,
            UnixMillis::new(7),
        )
    }

    fn assessment_record(
        outcome: GoalOutcome,
        results: Vec<CriterionAssessmentResultV2>,
    ) -> GoalAssessmentRecordV2 {
        assessment_attempt(outcome, results).unwrap()
    }

    #[test]
    fn goal_assessment_achieved_requires_every_criterion_satisfied_in_assessment_mode() {
        let record = assessment_record(
            GoalOutcome::Achieved,
            vec![
                assessment_result("deterministic", CriterionStatusV2::Satisfied),
                assessment_result("recoverable", CriterionStatusV2::Satisfied),
            ],
        );
        assert_eq!(record.outcome(), GoalOutcome::Achieved);
        assert_eq!(record.mode(), CriterionAssessmentModeV2::Assessment);
        assert_eq!(record.criterion_results().len(), 2);
        assert_eq!(record.decision_attempt_id(), &attempt_id(3));
        assert_eq!(record.decision_graph_node_id(), &graph_node("assess-goal"));

        assert_eq!(
            assessment_attempt(
                GoalOutcome::Achieved,
                vec![
                    assessment_result("deterministic", CriterionStatusV2::Satisfied),
                    assessment_result("recoverable", CriterionStatusV2::Unsatisfied),
                ],
            )
            .unwrap_err(),
            invalid("achieved requires every criterion satisfied")
        );
    }

    #[test]
    fn goal_assessment_not_achieved_requires_at_least_one_unsatisfied_criterion() {
        assert!(
            assessment_record(
                GoalOutcome::NotAchieved,
                vec![
                    assessment_result("deterministic", CriterionStatusV2::Satisfied),
                    assessment_result("recoverable", CriterionStatusV2::Unsatisfied),
                ],
            )
            .outcome()
                == GoalOutcome::NotAchieved
        );

        assert_eq!(
            assessment_attempt(
                GoalOutcome::NotAchieved,
                vec![
                    assessment_result("deterministic", CriterionStatusV2::Satisfied),
                    assessment_result("recoverable", CriterionStatusV2::Satisfied),
                ],
            )
            .unwrap_err(),
            invalid("not_achieved requires at least one unsatisfied criterion")
        );
    }

    #[test]
    fn goal_assessment_superseded_requires_applicability_mode_and_no_citations() {
        let record = assessment_record(
            GoalOutcome::Superseded,
            vec![assessment_result(
                "deterministic",
                CriterionStatusV2::NotApplicable,
            )],
        );
        assert_eq!(record.outcome(), GoalOutcome::Superseded);
        assert_eq!(record.mode(), CriterionAssessmentModeV2::Applicability);

        assert_eq!(
            assessment_attempt(
                GoalOutcome::Superseded,
                vec![assessment_result(
                    "deterministic",
                    CriterionStatusV2::Satisfied
                )],
            )
            .unwrap_err(),
            invalid("superseded requires applicability mode")
        );
    }

    #[test]
    fn goal_assessment_rejects_mixed_modes_duplicate_criteria_and_empty_or_oversize_sets() {
        let mixed = vec![
            assessment_result("deterministic", CriterionStatusV2::Satisfied),
            assessment_result("recoverable", CriterionStatusV2::NotApplicable),
        ];
        assert_eq!(
            assessment_attempt(GoalOutcome::NotAchieved, mixed).unwrap_err(),
            invalid("criterion assessment modes must be homogeneous within one assessment")
        );

        let duplicate = vec![
            assessment_result("deterministic", CriterionStatusV2::Satisfied),
            assessment_result("deterministic", CriterionStatusV2::Satisfied),
        ];
        assert_eq!(
            assessment_attempt(GoalOutcome::Achieved, duplicate).unwrap_err(),
            invalid("goal assessment criterion identifiers must be unique")
        );

        assert_eq!(
            assessment_attempt(GoalOutcome::Achieved, Vec::new()).unwrap_err(),
            invalid("a goal assessment records between one and sixteen criterion results")
        );

        let oversize: Vec<CriterionAssessmentResultV2> = (0..17)
            .map(|i| assessment_result(&format!("c-{i}"), CriterionStatusV2::Satisfied))
            .collect();
        assert_eq!(
            assessment_attempt(GoalOutcome::Achieved, oversize).unwrap_err(),
            invalid("a goal assessment records between one and sixteen criterion results")
        );
    }

    #[test]
    fn goal_assessment_accepts_the_sixteen_result_ceiling() {
        let sixteen: Vec<CriterionAssessmentResultV2> = (0..16)
            .map(|i| assessment_result(&format!("c-{i}"), CriterionStatusV2::Satisfied))
            .collect();
        assert!(assessment_attempt(GoalOutcome::Achieved, sixteen).is_ok());
    }

    #[test]
    fn goal_assessment_rejects_zero_goal_revision_and_decision_trace() {
        let valid_results = vec![assessment_result(
            "deterministic",
            CriterionStatusV2::Satisfied,
        )];
        assert_eq!(
            GoalAssessmentRecordV2::new(
                GoalRevisionNumberV2::ZERO,
                GoalOutcome::Achieved,
                valid_results.clone(),
                empty_evidence(),
                None,
                attempt_id(3),
                graph_node("assess-goal"),
                TraceSequenceV2::FIRST,
                UnixMillis::new(7),
            )
            .unwrap_err(),
            invalid("goal assessment goal revision must be nonzero")
        );
        assert_eq!(
            GoalAssessmentRecordV2::new(
                GoalRevisionNumberV2::FIRST,
                GoalOutcome::Achieved,
                valid_results,
                empty_evidence(),
                None,
                attempt_id(3),
                graph_node("assess-goal"),
                TraceSequenceV2::ZERO,
                UnixMillis::new(7),
            )
            .unwrap_err(),
            invalid("goal assessment decision trace must be nonzero")
        );
    }
}
