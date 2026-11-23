//! Scoped cursor construction for the native CLI.

use super::*;
use podway_core::execution_groups::{
    EvidenceSourceV3, ExecutionCursorFenceV1, ExecutionGoalChangeV3, ExecutionGoalV3,
    GroupClosureFeedbackV1, GroupExecutionIdV1, ScopedAttemptV1,
};
use podway_engine::successor_execution::{
    DirectSuccessorMutationV1, SuccessorCursor, SuccessorCursorMutation,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorWire {
    session: SessionId,
    session_revision: Revision,
    attempt: Option<ScopedAttemptV1>,
    goal_revision: Option<u64>,
    selected_group: Option<GroupExecutionIdV1>,
}

fn invalid_cursor() -> LocalFailure {
    LocalFailure::request_invalid("--if-cursor must be the complete cursor object from observe")
}

pub(super) fn decode_cursor_argument(text: &str) -> Result<SuccessorCursor, LocalFailure> {
    if text.len() > 4096 {
        return Err(invalid_cursor());
    }
    // The typed parse rejects duplicate fields before the generic schema pass.
    let _: CursorWire = serde_json::from_str(text).map_err(|_| invalid_cursor())?;
    let document: Value = serde_json::from_str(text).map_err(|_| invalid_cursor())?;
    decode_cursor(&document)
}

pub(super) fn decode_cursor(value: &Value) -> Result<SuccessorCursor, LocalFailure> {
    if !podway_protocol::validate_daemon_free_document_v1("podway.successor-cursor/v1", value) {
        return Err(invalid_cursor());
    }
    let wire: CursorWire = serde_json::from_value(value.clone()).map_err(|_| invalid_cursor())?;
    if wire
        .attempt
        .as_ref()
        .is_some_and(|attempt| attempt.session() != &wire.session)
    {
        return Err(invalid_cursor());
    }
    Ok(SuccessorCursor {
        execution: ExecutionCursorFenceV1 {
            session: wire.session,
            revision: wire.session_revision,
            attempt: wire.attempt,
            goal_revision: wire.goal_revision.map(GoalRevisionNumberV2::new),
        },
        selected_group: wire.selected_group,
    })
}

fn goal(statement: &str, criteria: &[String]) -> Result<ExecutionGoalV3, LocalFailure> {
    Ok(ExecutionGoalV3 {
        statement: GoalStatementV2::new(statement.to_owned()).map_err(invalid_v2_value)?,
        criteria: GoalDefinitionV2::new(parse_criteria(criteria)?).map_err(invalid_v2_value)?,
    })
}

fn reason(value: &str) -> Result<ReasonV2, LocalFailure> {
    validate_reason_cli(value, "--reason")?;
    ReasonV2::new(value.to_owned()).map_err(invalid_v2_value)
}

pub(super) fn is_cursor_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Begin(_)
            | Command::Complete
            | Command::Decide(_)
            | Command::Rework(_)
            | Command::Retry { .. }
            | Command::Skip { .. }
            | Command::Cancel { .. }
            | Command::Block { .. }
            | Command::Unblock { .. }
            | Command::Goal {
                command: GoalCommand::Define(_)
                    | GoalCommand::Revise(_)
                    | GoalCommand::AssessCriterion(_)
            }
            | Command::Group {
                command: GroupCommand::Return
            }
    )
}

pub(super) fn build_mutation(
    command: &Command,
    cursor: SuccessorCursor,
) -> Result<DirectSuccessorMutationV1, LocalFailure> {
    let actor = match command {
        Command::Begin(args) => args.actor.as_deref(),
        Command::Decide(args) => args.actor.as_deref(),
        Command::Rework(args) => args.actor.as_deref(),
        Command::Goal {
            command: GoalCommand::Define(args),
        } => args.actor.as_deref(),
        Command::Goal {
            command: GoalCommand::Revise(args),
        } => args.actor.as_deref(),
        _ => None,
    }
    .map(podway_core::ActorAttributionV2::new)
    .transpose()
    .map_err(invalid_v2_value)?;
    let operation = match command {
        Command::Begin(args) => SuccessorCursorMutation::Begin(
            args.goal
                .as_deref()
                .map(|text| goal(text, &args.criterion))
                .transpose()?,
        ),
        Command::Complete => SuccessorCursorMutation::Complete,
        Command::Decide(args) => SuccessorCursorMutation::DecideAssessed {
            option: OptionId::new(args.option.clone()).map_err(invalid_v2_value)?,
            reason: reason(&args.reason)?,
        },
        Command::Rework(args) => SuccessorCursorMutation::Rework {
            target: GraphNodeId::new(args.to.clone()).map_err(invalid_v2_value)?,
            reason: reason(&args.reason)?,
        },
        Command::Retry { reason: text } => SuccessorCursorMutation::Retry(reason(text)?),
        Command::Skip { reason: text } => {
            SuccessorCursorMutation::Skip(text.as_deref().map(reason).transpose()?)
        }
        Command::Cancel { reason: text } => SuccessorCursorMutation::Cancel(reason(text)?),
        Command::Block { reason: text } => SuccessorCursorMutation::Block(reason(text)?),
        Command::Unblock { all: true, .. } => SuccessorCursorMutation::UnblockAll,
        Command::Unblock {
            blocker_id: Some(id),
            ..
        } => SuccessorCursorMutation::Unblock(
            podway_core::BlockerId::new(id.clone()).map_err(invalid_v2_value)?,
        ),
        Command::Goal {
            command: GoalCommand::AssessCriterion(args),
        } => single_criterion(args)?,
        Command::Goal {
            command: GoalCommand::Define(args),
        } => SuccessorCursorMutation::DefineGoal(goal(&args.goal, &args.criterion)?),
        Command::Goal {
            command: GoalCommand::Revise(args),
        } => {
            let basis = podway_core::CheckResultInputBasisV2::new(
                args.basis_description.clone(),
                Sha256Digest::new(args.basis_digest.clone()).map_err(invalid_v2_value)?,
            )
            .map_err(invalid_v2_value)?;
            let feedback =
                GroupClosureFeedbackV1::new(args.reason.clone(), basis, args.feedback.clone())
                    .map_err(invalid_v2_value)?;
            SuccessorCursorMutation::ReviseGoal(ExecutionGoalChangeV3 {
                goal: goal(&args.goal, &args.criterion)?,
                target: GraphNodeId::new(args.rework_to.clone()).map_err(invalid_v2_value)?,
                feedback,
                reactivate: args.reactivate,
            })
        }
        Command::Group {
            command: GroupCommand::Return,
        } => {
            return Ok(DirectSuccessorMutationV1::Return(cursor));
        }
        _ => {
            return Err(LocalFailure::request_invalid(
                "requires a supported scoped cursor operation",
            ));
        }
    };
    Ok(DirectSuccessorMutationV1::Cursor {
        cursor,
        operation,
        actor,
    })
}

fn single_criterion(
    args: &super::grammar::GoalAssessCriterionArgs,
) -> Result<SuccessorCursorMutation, LocalFailure> {
    use podway_core::execution_groups::{SuccessorCriterionCitationV1, SuccessorCriterionInputV1};

    let mut citations = Vec::with_capacity(args.evidence.len() + args.item.len());
    for source in &args.evidence {
        citations.push(SuccessorCriterionCitationV1::Evidence(
            parse_evidence_selector(source)?,
        ));
    }
    for item in &args.item {
        citations.push(SuccessorCriterionCitationV1::Item(
            ItemId::new(item.clone()).map_err(invalid_v2_value)?,
        ));
    }
    Ok(SuccessorCursorMutation::AssessCriterion {
        entry: SuccessorCriterionInputV1 {
            criterion: CriterionId::new(args.criterion_id.clone()).map_err(invalid_v2_value)?,
            status: args.status.parse().map_err(invalid_v2_value)?,
            reason: CriterionAssessmentReasonV2::new(args.reason.clone())
                .map_err(invalid_v2_value)?,
            citations,
        },
        actor: args
            .actor
            .as_ref()
            .map(|value| ActorAttributionV2::new(value.clone()))
            .transpose()
            .map_err(invalid_v2_value)?,
    })
}

pub(super) fn parse_evidence_selector(source: &str) -> Result<EvidenceSourceV3, LocalFailure> {
    match source {
        "@plan" => Ok(EvidenceSourceV3::Plan),
        "@execution-result" => Ok(EvidenceSourceV3::ExecutionResult),
        _ => GraphNodeId::new(source.to_owned())
            .map(EvidenceSourceV3::Local)
            .map_err(invalid_v2_value),
    }
}
