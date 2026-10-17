//! Procedure v2 session-goal persistence and immutable assessment history.

use std::collections::BTreeSet;
use std::str::FromStr;

use podway_core::{
    ActorAttributionV2, AttemptId, CriterionAssessmentModeV2, CriterionAssessmentReasonV2,
    CriterionAssessmentResultV2, CriterionCitationV2, CriterionId, CriterionStatusV2,
    GoalAssessmentRecordV2, GoalCriterionV2, GoalDefinitionV2, GoalOutcome, GoalRevisionNumberV2,
    GoalRevisionReasonV2, GoalRevisionRecordV2, GoalStatementV2, GraphNodeId, ItemId, SessionId,
    Sha256Digest, TraceSequenceV2, UnixMillis, canonicalize_json_v1,
};
use rusqlite::{Connection, Transaction, params};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::{
    RusqliteErrorContextV1, StoreErrorV1, StoreRecordKindV1, StoreValueErrorV1,
    map_rusqlite_error_v1,
};
use podway_core::WorkflowMemoryStateV2;
pub use podway_core::goal_state_v2::{
    AttemptCriterionAssessmentStateV2, CriterionAssessmentStateV2, GoalStateV2,
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
            "SELECT (SELECT COUNT(*) FROM v2_goal_revisions WHERE session_id = ?1), \
             (SELECT COUNT(*) FROM v2_goal_criteria WHERE session_id = ?1), \
             (SELECT COUNT(*) FROM v2_criterion_assessment_results WHERE session_id = ?1), \
             (SELECT COUNT(*) FROM v2_criterion_citations AS citation \
                JOIN v2_criterion_assessment_results AS result \
                  ON result.attempt_id = citation.attempt_id \
                 AND result.criterion_id = citation.criterion_id \
               WHERE result.session_id = ?1), \
             (SELECT COUNT(*) FROM v2_goal_assessments WHERE session_id = ?1)",
            [session_id.as_str()],
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
