use super::*;
use podway_engine::successor_execution::{
    DirectGroupActivationV1, DirectGroupSourceV1, DirectSuccessorMutationV1, SuccessorCursor,
    SuccessorCursorMutation,
};
use podway_protocol::{
    GroupDocumentFormatV1, GroupSourceInputV1, SuccessorMutationContextV1,
    decode_group_activate_input_v1, decode_group_replan_input_v1, decode_scoped_criterion_input_v1,
};

enum ScopedStdinOperation {
    Activate,
    Replan,
    AssessCriteria,
    RecordMany,
    DelegationAccept,
}

/// Document-owned fences stay inside the closed input. This path performs no status preflight.
pub(super) fn execute_scoped_stdin(
    cli: &Cli,
    started: Instant,
    generation: protocol::ContractGenerationV1,
) -> Result<Option<RunResult>, LocalFailure> {
    let operation = match &cli.command {
        Command::Group {
            command: GroupCommand::Activate { .. },
        } => ScopedStdinOperation::Activate,
        Command::Group {
            command: GroupCommand::Replan { .. },
        } => ScopedStdinOperation::Replan,
        Command::Goal {
            command: GoalCommand::AssessCriteria(_),
        } => ScopedStdinOperation::AssessCriteria,
        Command::Record(_) => ScopedStdinOperation::RecordMany,
        Command::Delegation {
            command: DelegationCommand::Accept(_),
        } => ScopedStdinOperation::DelegationAccept,
        _ => return Ok(None),
    };
    let route = cli.command.canonical_route();
    cli.reject_stdin_conflicting_fences()?;
    if cli.with_observation
        && !matches!(
            operation,
            ScopedStdinOperation::AssessCriteria | ScopedStdinOperation::RecordMany
        )
    {
        return Err(LocalFailure::request_invalid(
            "--with-observation is not accepted for this scoped stdin command",
        )
        .with_command(route));
    }
    let bytes = read_bounded_successor_input(route)?;
    let input_failure = |error| successor_input_failure(error, route, cli.json);
    let (command, context) = match operation {
        ScopedStdinOperation::Activate => {
            let input = decode_group_activate_input_v1(&bytes).map_err(input_failure)?;
            let group = match input.source {
                GroupSourceInputV1::Default {} => DirectGroupSourceV1::CatalogDefault,
                GroupSourceInputV1::Document { source, format } => DirectGroupSourceV1::Document {
                    source,
                    format: match format {
                        GroupDocumentFormatV1::Yaml => podway_config::ProcedureDocumentFormat::Yaml,
                        GroupDocumentFormatV1::Json => podway_config::ProcedureDocumentFormat::Json,
                    },
                },
            };
            (
                Direct::SuccessorMutation(DirectSuccessorMutationV1::Activate {
                    cursor: admitted_cursor(&input.context),
                    input: DirectGroupActivationV1 {
                        group,
                        expected_definition: input.expected_definition,
                        plan: input.plan,
                        predecessor: input.predecessor,
                        working_copy_basis: input.working_copy_basis,
                        prior_work: input.prior_work,
                    },
                }),
                input.context,
            )
        }
        ScopedStdinOperation::Replan => {
            let input = decode_group_replan_input_v1(&bytes).map_err(input_failure)?;
            (
                Direct::SuccessorMutation(DirectSuccessorMutationV1::Replan {
                    cursor: admitted_cursor(&input.context),
                    feedback: input.feedback,
                }),
                input.context,
            )
        }
        ScopedStdinOperation::AssessCriteria => {
            let input = decode_scoped_criterion_input_v1(&bytes).map_err(input_failure)?;
            (
                Direct::SuccessorMutation(DirectSuccessorMutationV1::Cursor {
                    actor: None,
                    cursor: admitted_cursor(&input.context),
                    operation: SuccessorCursorMutation::AssessCriteria {
                        entries: input.entries,
                        actor: input.actor,
                    },
                }),
                input.context,
            )
        }
        ScopedStdinOperation::DelegationAccept => {
            let input = protocol::decode_scoped_delegation_accept_input_v1(&bytes)
                .map_err(input_failure)?;
            let context = input.context.clone();
            (Direct::SuccessorDelegationAccept(input), context)
        }
        ScopedStdinOperation::RecordMany => {
            let input =
                protocol::decode_scoped_item_record_many_input_v1(&bytes).map_err(input_failure)?;
            (
                Direct::SuccessorWrite(
                    podway_engine::successor_execution::DirectSuccessorWriteV1::Atomic {
                        cursor: admitted_cursor(&input.context),
                        operations: input.operations,
                    },
                ),
                input.context,
            )
        }
    };
    let target = workspace_target(cli.worktree.clone())?;
    let mut invocation =
        Invocation::new(&target, cli.timeout.unwrap_or(5_000), started, generation)?;
    let key = protocol_boundary::IdempotencyKeyV1::new(context.idempotency_key.as_str().to_owned())
        .map_err(invalid_v2_value)?;
    let response = invocation.execute(
        command,
        Some(context.workspace_uuid),
        Some(key),
        cli.with_observation,
    )?;
    Ok(Some(direct_response_result(&cli.command, response)))
}

fn successor_input_failure(error: protocol::SliceErrorV1, route: &str, json: bool) -> LocalFailure {
    if !json
        && let protocol::SliceErrorV1::InvalidSuccessorDocument {
            mut message,
            pointer: Some(pointer),
        } = error
    {
        const MAX_HUMAN_DIAGNOSTIC_SCALARS: usize = 65_536;

        let prefix_len = message.len();
        message.push_str(" at ");
        // Escape backslashes too, so a literal escape sequence stays distinct
        // from an escaped control character in the human diagnostic.
        for character in pointer.chars() {
            if character == '\\' || character.is_control() {
                message.extend(character.escape_debug());
            } else {
                message.push(character);
            }
        }
        // Count the stderr prefix and newline as well as the escaped message.
        if "error: \n".len() + message.chars().count() > MAX_HUMAN_DIAGNOSTIC_SCALARS {
            message.truncate(prefix_len);
            message.push_str(" (escaped RFC 6901 pointer exceeds the error message length limit)");
        }
        return LocalFailure::request_invalid(message).with_command(route);
    }
    invalid_v2_value(error).with_command(route)
}

fn admitted_cursor(context: &SuccessorMutationContextV1) -> SuccessorCursor {
    SuccessorCursor {
        execution: context.cursor.execution(),
        selected_group: context.cursor.selected_group.clone(),
    }
}

fn read_bounded_successor_input(route: &str) -> Result<Vec<u8>, LocalFailure> {
    let limit = protocol::MAX_DIRECT_INPUT_BYTES_V1;
    let mut bytes = Vec::with_capacity(8 * 1024);
    io::stdin()
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            LocalFailure::request_invalid("cannot read scoped stdin").with_command(route)
        })?;
    if bytes.len() > limit {
        return Err(LocalFailure::catalog(
            "REQUEST_TOO_LARGE",
            "scoped stdin exceeds the direct input bound",
            route,
        ));
    }
    Ok(bytes)
}
