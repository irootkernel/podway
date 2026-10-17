//! Gated fence preflight; the sole future READS facts replacement boundary.

use std::{
    io::{self, IsTerminal},
    ops::ControlFlow,
};

use podway_core::{AttemptId, GoalRevisionNumberV2, Revision, SessionId, WorkspaceId};
use serde_json::{Map, Value};

use super::{
    ArchiveCommand, Cli, Command, ExplicitPreconditions, GoalCommand, LocalFailure, RunResult,
    direct, invalid_v2_value, item_id, preflight_error,
};
use podway_engine::{
    direct_api::{self as protocol_boundary, DirectCommandV1 as Direct},
    direct_execution::DirectReadCommandV1 as ReadCommand,
    v2_read_service::GraphStatusTierV2,
};

#[derive(Clone, Debug)]
pub(super) struct StatusFacts {
    pub(super) session_id: SessionId,
    pub(super) session_revision: Revision,
    pub(super) attempt_id: Option<AttemptId>,
    pub(super) item_revisions: Vec<(String, Revision)>,
    pub(super) goal_revision: Option<GoalRevisionNumberV2>,
    pub(super) lifecycle: String,
}

#[derive(Clone, Debug)]
pub(super) struct StatusPreflight {
    pub(super) transport_workspace_id: WorkspaceId,
    pub(super) facts: StatusFacts,
}

impl StatusPreflight {
    pub(super) fn from_direct(
        output: &podway_protocol::OutputEnvelopeV4,
    ) -> Result<Self, LocalFailure> {
        let value = output.as_value();
        let result = value["result"]
            .as_object()
            .ok_or_else(|| LocalFailure::response_invalid("status result is invalid"))?;
        let workspace_id = value["workspace"]["uuid"]
            .as_str()
            .and_then(|id| WorkspaceId::new(id.to_owned()).ok())
            .ok_or_else(|| {
                LocalFailure::response_invalid("status response omitted workspace identity")
            })?;
        Self::from_parts(result, workspace_id)
    }

    pub(super) fn from_parts(
        result: &Map<String, Value>,
        transport_workspace_id: WorkspaceId,
    ) -> Result<Self, LocalFailure> {
        if !matches!(
            result.get("schema").and_then(Value::as_str),
            Some("podway.status-result/v5")
        ) {
            return Err(LocalFailure::response_invalid(
                "status preflight did not return the standard Procedure v2 status result",
            ));
        }
        let session = result
            .get("session")
            .and_then(Value::as_object)
            .ok_or_else(|| LocalFailure::response_invalid("status response omitted session"))?;
        let session_id = session
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .and_then(|value| SessionId::new(value).ok())
            .ok_or_else(|| LocalFailure::response_invalid("status session ID is invalid"))?;
        let session_revision = session
            .get("revision")
            .and_then(Value::as_u64)
            .map(Revision::new)
            .ok_or_else(|| LocalFailure::response_invalid("status revision is invalid"))?;
        let lifecycle = session
            .get("lifecycle")
            .and_then(Value::as_str)
            .filter(|value| matches!(*value, "prepared" | "running" | "completed" | "cancelled"))
            .map(str::to_owned)
            .ok_or_else(|| LocalFailure::response_invalid("status lifecycle is invalid"))?;
        let attempt_id = result
            .get("current")
            .and_then(Value::as_object)
            .and_then(|current| current.get("attempt"))
            .and_then(Value::as_object)
            .and_then(|attempt| attempt.get("attempt_id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .and_then(|value| AttemptId::new(value).ok());
        let item_revisions = result
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let item = item.as_object()?;
                Some((
                    item.get("item_id")?.as_str()?.to_owned(),
                    Revision::new(item.get("revision")?.as_u64()?),
                ))
            })
            .collect();
        let goal_revision = result
            .get("goal_revision")
            .and_then(Value::as_u64)
            .map(GoalRevisionNumberV2::new);
        Ok(Self {
            transport_workspace_id,
            facts: StatusFacts {
                session_id,
                session_revision,
                attempt_id,
                item_revisions,
                goal_revision,
                lifecycle,
            },
        })
    }
}
fn needs_facts(command: &Command, fences: &ExplicitPreconditions) -> bool {
    if command.is_item_mutation() {
        return fences.session_id.is_none()
            || fences.attempt_id.is_none()
            || fences.item_revision.is_none();
    }
    if matches!(
        command,
        Command::Archive {
            command: Some(ArchiveCommand::Purge { .. })
        }
    ) {
        return false;
    }
    if !command.needs_preflight() || matches!(command, Command::Record(_)) {
        return false;
    }
    if fences.session_id.is_none() || fences.session_revision.is_none() {
        return true;
    }
    match command {
        Command::Start(_)
        | Command::Begin(_)
        | Command::Disposition { .. }
        | Command::Reset(_)
        | Command::Archive { command: None }
        | Command::Rework(_)
        | Command::Goal {
            command: GoalCommand::Define(_),
        } => false,
        Command::Goal {
            command: GoalCommand::Revise(_),
        } => fences.goal_revision.is_none(),
        Command::Goal {
            command: GoalCommand::AssessCriterion(_),
        } => fences.attempt_id.is_none() || fences.goal_revision.is_none(),
        _ => fences.attempt_id.is_none(),
    }
}

pub(super) fn fill_facts(
    command: &Command,
    fences: &mut ExplicitPreconditions,
    preflight: &StatusPreflight,
) {
    fences
        .workspace_id
        .get_or_insert_with(|| preflight.transport_workspace_id.clone());
    fences
        .session_id
        .get_or_insert_with(|| preflight.facts.session_id.clone());
    fences
        .session_revision
        .get_or_insert(preflight.facts.session_revision);
    if !matches!(
        command,
        Command::Start(_)
            | Command::Begin(_)
            | Command::Disposition { .. }
            | Command::Reset(_)
            | Command::Archive { command: None }
            | Command::Goal {
                command: GoalCommand::Define(_)
            }
    ) && fences.attempt_id.is_none()
    {
        fences.attempt_id = preflight.facts.attempt_id.clone();
    }
    if command.is_item_mutation() {
        fences.session_revision = None;
        if fences.item_revision.is_none() {
            fences.item_revision = item_id(command).and_then(|selected| {
                preflight
                    .facts
                    .item_revisions
                    .iter()
                    .find_map(|(id, revision)| (id == selected).then_some(*revision))
                    // An absent active item still belongs to the engine's
                    // ITEM_NOT_FOUND contract, rather than a missing CLI fence.
                    .or(Some(Revision::new(0)))
            });
        }
    }
    if matches!(
        command,
        Command::Decide(_)
            | Command::Goal {
                command: GoalCommand::Revise(_) | GoalCommand::AssessCriterion(_)
            }
    ) && fences.goal_revision.is_none()
    {
        fences.goal_revision = preflight.facts.goal_revision;
    }
}

pub(super) fn prepare(
    invocation: &direct::Invocation,
    cli: &Cli,
    fences: &mut ExplicitPreconditions,
) -> Result<ControlFlow<RunResult, Option<StatusPreflight>>, LocalFailure> {
    if !needs_facts(&cli.command, fences) {
        return Ok(ControlFlow::Continue(None));
    }
    let route = cli.command.canonical_route();
    let status = invocation.execute(
        Direct::Read {
            command: ReadCommand::Status {
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            expected_session_id: fences.session_id.clone(),
        },
        fences.workspace_id.clone(),
        None,
    )?;
    let output = match status {
        protocol_boundary::DirectResponseV1::Success(output) => output,
        protocol_boundary::DirectResponseV1::Failure(error) => {
            return Ok(ControlFlow::Break(preflight_error(&cli.command, error)?));
        }
    };
    let facts = StatusPreflight::from_direct(&output)?;
    fill_facts(&cli.command, fences, &facts);
    if facts.facts.lifecycle == "prepared"
        && (cli.command.is_item_mutation()
            || matches!(
                &cli.command,
                Command::Complete
                    | Command::Skip { .. }
                    | Command::Retry { .. }
                    | Command::Block { .. }
                    | Command::Unblock { .. }
                    | Command::Cancel { .. }
                    | Command::Decide(_)
                    | Command::Goal {
                        command: GoalCommand::Revise(_) | GoalCommand::AssessCriterion(_)
                    }
            ))
    {
        // Prepared state has no attempt or goal fence to autofill. Reject it
        // without manufacturing identities, after checking caller-owned fences.
        if let Some(key) = &cli.idempotency_key {
            protocol_boundary::IdempotencyKeyV1::new(key.clone()).map_err(invalid_v2_value)?;
        } else if cli.json || !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(LocalFailure::request_invalid(
                "automation mutations require --idempotency-key",
            ));
        }
        if let Some(expected) = fences.session_revision
            && expected != facts.facts.session_revision
        {
            return Err(LocalFailure::session_revision_conflict(
                route,
                expected,
                facts.facts.session_revision,
            ));
        }
        return Err(LocalFailure::catalog(
            "SESSION_NOT_RUNNING",
            "the command requires a running session",
            route,
        ));
    }
    Ok(ControlFlow::Continue(Some(facts)))
}
