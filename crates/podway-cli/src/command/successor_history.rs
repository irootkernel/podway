//! Typed selection of current or archived historical records.

use super::*;
use podway_core::execution_groups::{GroupExecutionIdV1, ScopedAttemptV1};
use podway_engine::successor_execution::{
    CandidateSessionLocation, DirectHistoryDefinitionV1, DirectSuccessorHistoryCommandV1 as Query,
    DirectSuccessorHistoryV1,
};

pub(super) fn query(command: &Command) -> Result<DirectSuccessorHistoryV1, LocalFailure> {
    let (page, command) = match command {
        Command::Group {
            command: GroupCommand::List(page),
        } => (page, Query::Groups),
        Command::Group {
            command: GroupCommand::Show { page, execution },
        } => (
            page,
            Query::Group {
                execution: GroupExecutionIdV1::new(execution.clone()).map_err(invalid_v2_value)?,
            },
        ),
        Command::Group {
            command:
                GroupCommand::PriorWork {
                    page,
                    execution,
                    definition_digest,
                },
        } => (
            page,
            Query::PriorWork {
                execution: GroupExecutionIdV1::new(execution.clone()).map_err(invalid_v2_value)?,
                definition: Sha256Digest::new(definition_digest.clone())
                    .map_err(invalid_v2_value)?,
            },
        ),
        Command::History {
            command: HistoryCommand::GoalRevisions(page),
        } => (page, Query::GoalRevisions),
        Command::History {
            command: HistoryCommand::Dag { page, scope },
        } => (
            page,
            Query::Dag {
                definition: definition(scope)?,
            },
        ),
        Command::History {
            command: HistoryCommand::Attempts { page, scope },
        } => (
            page,
            Query::Attempts {
                definition: definition(scope)?,
            },
        ),
        Command::History {
            command: HistoryCommand::Blockers { page, attempt },
        } => (
            page,
            Query::Blockers {
                attempt: scoped_attempt(attempt)?,
            },
        ),
        Command::History {
            command:
                HistoryCommand::Item {
                    page,
                    attempt,
                    item,
                },
        } => (
            page,
            Query::Item {
                attempt: scoped_attempt(attempt)?,
                item: ItemId::new(item.clone()).map_err(invalid_v2_value)?,
            },
        ),
        _ => {
            return Err(LocalFailure::request_invalid(
                "requires a scoped history query",
            ));
        }
    };
    Ok(DirectSuccessorHistoryV1 {
        session: Some(SessionId::new(page.session.clone()).map_err(invalid_v2_value)?),
        location: if page.archived {
            CandidateSessionLocation::Archived
        } else {
            CandidateSessionLocation::Current
        },
        limit: usize::from(page.limit),
        continuation: page.page_token.clone(),
        command,
    })
}

fn definition(args: &HistoryDefinitionArgs) -> Result<DirectHistoryDefinitionV1, LocalFailure> {
    let digest = Sha256Digest::new(args.definition_digest.clone()).map_err(invalid_v2_value)?;
    if let Some(id) = &args.group_execution {
        Ok(DirectHistoryDefinitionV1::Group {
            execution: GroupExecutionIdV1::new(id.clone()).map_err(invalid_v2_value)?,
            digest,
        })
    } else {
        Ok(DirectHistoryDefinitionV1::Base(digest))
    }
}

fn scoped_attempt(value: &str) -> Result<ScopedAttemptV1, LocalFailure> {
    if value.len() > 4096 {
        return Err(LocalFailure::request_invalid(
            "scoped attempt exceeds 4096 bytes",
        ));
    }
    serde_json::from_str(value).map_err(invalid_v2_value)
}
