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

#[cfg(test)]
mod reads001_parser_surface_tests {
    //! READS-001 characterization of the fence-preflight parser's rejection
    //! surface with unit-crafted maps. These branches are not producible by
    //! the real engine's success envelope absent an engine bug: they are the
    //! recorded replacement obligations for READS-004, kept separate from
    //! input-reachable failures that flow through `preflight_error`.
    use super::*;
    use serde_json::json;

    fn workspace() -> WorkspaceId {
        WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap()
    }

    #[test]
    fn crafted_schema_tag_rejection_surface() {
        let mut result = json!({
            "schema": "podway.status-result/v4",
            "session": {"id": "00000000-0000-4000-8000-000000000002", "revision": 1, "lifecycle": "running"},
        });
        assert!(StatusPreflight::from_parts(result.as_object().unwrap(), workspace()).is_err());
        result["schema"] = json!("podway.status-result/v5");
        assert!(StatusPreflight::from_parts(result.as_object().unwrap(), workspace()).is_ok());
    }

    #[test]
    fn crafted_session_shape_rejection_surface() {
        let base = json!({
            "schema": "podway.status-result/v5",
            "session": {"id": "00000000-0000-4000-8000-000000000002", "revision": 1, "lifecycle": "running"},
        });
        let mut missing_session = base.clone();
        missing_session.as_object_mut().unwrap().remove("session");
        assert!(
            StatusPreflight::from_parts(missing_session.as_object().unwrap(), workspace()).is_err()
        );
        let mut bad_id = base.clone();
        bad_id["session"]["id"] = json!("not-a-session-id");
        assert!(StatusPreflight::from_parts(bad_id.as_object().unwrap(), workspace()).is_err());
        let mut bad_revision = base.clone();
        bad_revision["session"]["revision"] = json!(-1);
        assert!(
            StatusPreflight::from_parts(bad_revision.as_object().unwrap(), workspace()).is_err()
        );
        let mut bad_lifecycle = base.clone();
        bad_lifecycle["session"]["lifecycle"] = json!("paused");
        assert!(
            StatusPreflight::from_parts(bad_lifecycle.as_object().unwrap(), workspace()).is_err()
        );
    }

    #[test]
    fn crafted_item_and_goal_fact_extraction() {
        let result = json!({
            "schema": "podway.status-result/v5",
            "session": {"id": "00000000-0000-4000-8000-000000000002", "revision": 7, "lifecycle": "running"},
            "current": {"attempt": {"attempt_id": "00000000-0000-4000-8000-000000000003"}},
            "items": [{"item_id": "summary", "revision": 4}],
            "goal_revision": 2,
        });
        let preflight =
            StatusPreflight::from_parts(result.as_object().unwrap(), workspace()).unwrap();
        assert_eq!(preflight.facts.session_revision, Revision::new(7));
        assert_eq!(
            preflight.facts.attempt_id.as_ref().map(AttemptId::as_str),
            Some("00000000-0000-4000-8000-000000000003")
        );
        assert_eq!(
            preflight.facts.item_revisions,
            vec![("summary".to_owned(), Revision::new(4))]
        );
        assert_eq!(
            preflight.facts.goal_revision,
            Some(GoalRevisionNumberV2::new(2))
        );
        // Optional sections absent: extraction stays permissive there.
        let minimal = json!({
            "schema": "podway.status-result/v5",
            "session": {"id": "00000000-0000-4000-8000-000000000002", "revision": 1, "lifecycle": "prepared"},
        });
        let preflight =
            StatusPreflight::from_parts(minimal.as_object().unwrap(), workspace()).unwrap();
        assert!(preflight.facts.attempt_id.is_none());
        assert!(preflight.facts.item_revisions.is_empty());
        assert!(preflight.facts.goal_revision.is_none());
    }
}
