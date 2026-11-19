//! Interactive start resolution and independent fresh-source previews.

use std::{
    io::{self, IsTerminal, Write},
    path::Path,
};

use podway_core::Sha256Digest;
use serde_json::{Value, json};

use super::{
    Cli, Command, ExistingSessionPolicy, LocalFailure, RunResult, StartTerminalDisposition,
    StatusPreflight, invalid_v2_value, local_result_v2, procedure_config_failure, procedure_format,
    read_worktree_procedure, workspace_target,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExistingStartResolution {
    Continue,
    Replace,
}

pub(super) fn resolve_existing_start(
    cli: &mut Cli,
    preflight: &StatusPreflight,
    terminal_disposed: Option<bool>,
) -> Result<ExistingStartResolution, LocalFailure> {
    let interactive =
        !cli.json && !cli.quiet && io::stdin().is_terminal() && io::stdout().is_terminal();
    let yes = cli.yes;
    let Command::Start(args) = &mut cli.command else {
        unreachable!("existing start resolution requires start")
    };

    match preflight.facts.lifecycle.as_str() {
        "prepared" => match args.on_existing {
            Some(ExistingSessionPolicy::Preserve) => Err(LocalFailure::request_invalid(
                "--on-existing preserve applies only to a running session",
            )),
            Some(ExistingSessionPolicy::Delete) => {
                if args.progress_summary.is_some()
                    || args.supersede_reason.is_some()
                    || args.actor.is_some()
                {
                    return Err(LocalFailure::request_invalid(
                        "deleting a prepared session does not accept progress, reason, or actor fields",
                    ));
                }
                args.delete_confirmed = true;
                Ok(ExistingStartResolution::Replace)
            }
            None if interactive && !args.dry_run => match prompt_start_choice(
                "A prepared session already exists.",
                &[
                    ("1", "Continue the existing task"),
                    ("2", "Delete it permanently and start the new task"),
                ],
            )? {
                Some("2") => {
                    args.on_existing = Some(ExistingSessionPolicy::Delete);
                    args.delete_confirmed = true;
                    Ok(ExistingStartResolution::Replace)
                }
                _ => Ok(ExistingStartResolution::Continue),
            },
            None => Err(LocalFailure::session_start_decision_required(
                "prepared",
                false,
                &["continue", "delete"],
            )),
        },
        "running" => match args.on_existing {
            Some(ExistingSessionPolicy::Preserve) => {
                if args.progress_summary.is_some() {
                    return Err(LocalFailure::request_invalid(
                        "--on-existing preserve does not accept --progress-summary",
                    ));
                }
                if args.supersede_reason.is_none() {
                    return Err(LocalFailure::request_invalid(
                        "--on-existing preserve requires --supersede-reason",
                    ));
                }
                Ok(ExistingStartResolution::Replace)
            }
            Some(ExistingSessionPolicy::Delete) => {
                if args.supersede_reason.is_some() || args.actor.is_some() {
                    return Err(LocalFailure::request_invalid(
                        "--on-existing delete does not accept supersede fields",
                    ));
                }
                if !args.dry_run && (args.progress_summary.is_none() || !yes) {
                    return Err(LocalFailure::request_invalid(
                        "deleting a running session requires --progress-summary and --yes",
                    ));
                }
                args.delete_confirmed = yes;
                Ok(ExistingStartResolution::Replace)
            }
            None if interactive && !args.dry_run => match prompt_start_choice(
                "A running session already exists.",
                &[
                    ("1", "Continue the existing task"),
                    (
                        "2",
                        "Cancel it, preserve it as superseded, and start the new task",
                    ),
                    ("3", "Delete it permanently and start the new task"),
                ],
            )? {
                Some("2") => {
                    let Some(reason) = prompt_required_start_text("Supersede reason")? else {
                        return Ok(ExistingStartResolution::Continue);
                    };
                    let actor = prompt_optional_start_text("Actor (optional)")?;
                    args.on_existing = Some(ExistingSessionPolicy::Preserve);
                    args.supersede_reason = Some(reason);
                    args.actor = actor;
                    Ok(ExistingStartResolution::Replace)
                }
                Some("3") => {
                    let Some(summary) = prompt_required_start_text("Progress summary")? else {
                        return Ok(ExistingStartResolution::Continue);
                    };
                    args.on_existing = Some(ExistingSessionPolicy::Delete);
                    args.progress_summary = Some(summary);
                    args.delete_confirmed = true;
                    Ok(ExistingStartResolution::Replace)
                }
                _ => Ok(ExistingStartResolution::Continue),
            },
            None => Err(LocalFailure::session_start_decision_required(
                "running",
                false,
                &["continue", "preserve", "delete"],
            )),
        },
        "completed" | "cancelled" if terminal_disposed == Some(true) => {
            if args.on_existing.is_some()
                || args.supersede_reason.is_some()
                || args.actor.is_some()
                || args.progress_summary.is_some()
            {
                return Err(LocalFailure::request_invalid(
                    "a disposed terminal session is archived automatically and does not accept existing-session policy fields",
                ));
            }
            args.auto_archive_terminal = true;
            Ok(ExistingStartResolution::Replace)
        }
        "completed" | "cancelled" => {
            if args.on_existing.is_some() || !interactive || args.dry_run {
                return Err(LocalFailure::session_start_decision_required(
                    preflight.facts.lifecycle.as_str(),
                    false,
                    &["continue", "record_disposition"],
                ));
            }
            match prompt_start_choice(
                "A terminal session needs a disposition before it can be archived.",
                &[
                    ("1", "Continue without starting a new task"),
                    ("2", "Record a handoff, archive it, and start the new task"),
                    (
                        "3",
                        "Record that no handoff is required, archive it, and start the new task",
                    ),
                ],
            )? {
                Some("2") => {
                    let Some(summary) = prompt_required_start_text("Handoff summary")? else {
                        return Ok(ExistingStartResolution::Continue);
                    };
                    let Some(reference) = prompt_required_start_text("Stable reference")? else {
                        return Ok(ExistingStartResolution::Continue);
                    };
                    let actor = prompt_optional_start_text("Actor (optional)")?;
                    args.terminal_disposition = Some(StartTerminalDisposition::HandedOff {
                        summary,
                        reference,
                        actor,
                    });
                    Ok(ExistingStartResolution::Replace)
                }
                Some("3") => {
                    let Some(reason) = prompt_required_start_text("No-handoff reason")? else {
                        return Ok(ExistingStartResolution::Continue);
                    };
                    let actor = prompt_optional_start_text("Actor (optional)")?;
                    args.terminal_disposition =
                        Some(StartTerminalDisposition::NotRequired { reason, actor });
                    Ok(ExistingStartResolution::Replace)
                }
                _ => Ok(ExistingStartResolution::Continue),
            }
        }
        _ => Err(LocalFailure::response_invalid(
            "start preflight returned an unsupported session lifecycle",
        )),
    }
}

fn prompt_start_choice(
    heading: &str,
    choices: &[(&'static str, &'static str)],
) -> Result<Option<&'static str>, LocalFailure> {
    let mut stderr = io::stderr().lock();
    writeln!(stderr, "{heading}")
        .and_then(|()| {
            for (key, label) in choices {
                writeln!(stderr, "  {key}. {label}")?;
            }
            write!(stderr, "Choose [1]: ")
        })
        .and_then(|()| stderr.flush())
        .map_err(|_| LocalFailure::internal("cannot write the start interview"))?;
    let mut input = String::new();
    let read = io::stdin()
        .read_line(&mut input)
        .map_err(|_| LocalFailure::request_invalid("cannot read the start interview"))?;
    if read == 0 || input.trim().is_empty() {
        return Ok(None);
    }
    let selected = input.trim();
    choices
        .iter()
        .find_map(|(key, _)| (*key == selected).then_some(*key))
        .map(Some)
        .ok_or_else(|| LocalFailure::request_invalid("invalid start interview selection"))
}

fn prompt_required_start_text(label: &str) -> Result<Option<String>, LocalFailure> {
    let value = prompt_start_text(label)?;
    match value {
        None => Ok(None),
        Some(value) if value.trim().is_empty() => Err(LocalFailure::request_invalid(
            "required start interview text cannot be blank",
        )),
        Some(value) => Ok(Some(value)),
    }
}

fn prompt_optional_start_text(label: &str) -> Result<Option<String>, LocalFailure> {
    Ok(prompt_start_text(label)?.filter(|value| !value.trim().is_empty()))
}

fn prompt_start_text(label: &str) -> Result<Option<String>, LocalFailure> {
    let mut stderr = io::stderr().lock();
    write!(stderr, "{label} (EOF cancels): ")
        .and_then(|()| stderr.flush())
        .map_err(|_| LocalFailure::internal("cannot write the start interview"))?;
    let mut input = String::new();
    let read = io::stdin()
        .read_line(&mut input)
        .map_err(|_| LocalFailure::request_invalid("cannot read the start interview"))?;
    if read == 0 {
        return Ok(None);
    }
    Ok(Some(input.trim_end_matches(['\r', '\n']).to_owned()))
}

/// Preview the same v3 base and default group used by native start. The
/// unfenced preview neither initializes nor opens a Store.
pub(super) fn preview(cli: &Cli) -> Result<RunResult, LocalFailure> {
    use podway_config::execution_groups::{
        SuccessorDefinition, parse_successor_document, validate_successor_definition,
    };

    let Command::Start(args) = &cli.command else {
        return Err(LocalFailure::request_invalid(
            "dry-run requires session start",
        ));
    };
    let command = "session.start";
    let (admitted, default_group_digest) = if let Some(name) = &args.preset {
        let preset = podway_presets::lookup_v3(name)
            .ok_or_else(|| LocalFailure::preset_not_found("unknown Procedure v3 preset"))?;
        let admitted = validate_successor_definition(
            parse_successor_document(
                preset.base_yaml.as_bytes(),
                podway_config::ProcedureDocumentFormat::Yaml,
            )
            .map_err(procedure_config_failure)?,
        )
        .map_err(procedure_config_failure)?;
        (admitted, Some(preset.group_digest))
    } else {
        let path = args
            .procedure
            .as_deref()
            .ok_or_else(|| LocalFailure::request_invalid("start requires a preset or procedure"))?;
        let worktree = workspace_target(cli.worktree.clone())?;
        let procedure = Path::new(path);
        let source = read_worktree_procedure(&worktree, procedure)?;
        let admitted = validate_successor_definition(
            parse_successor_document(&source, procedure_format(procedure))
                .map_err(procedure_config_failure)?,
        )
        .map_err(procedure_config_failure)?;
        let expected = args.expect_procedure_digest.as_deref().ok_or_else(|| {
            LocalFailure::digest_confirmation_required(admitted.digest(), command)
        })?;
        let expected = Sha256Digest::new(expected).map_err(invalid_v2_value)?;
        if admitted.digest() != &expected {
            return Err(LocalFailure::procedure_digest_mismatch(
                &expected,
                admitted.digest(),
                command,
            ));
        }
        (admitted, None)
    };
    let SuccessorDefinition::Base(base) = admitted.definition() else {
        return Err(LocalFailure::procedure_invalid(
            "start requires a base Procedure v3",
        ));
    };
    let Value::Object(result) = json!({
        "schema": "podway.successor-session-start-result/v1",
        "operation": command,
        "procedure_schema": "podway.procedure/v3",
        "procedure_digest": admitted.digest(),
        "default_group_digest": default_group_digest,
        "dry_run": true,
        "session_state": "prepared",
        "goal_tracking": base.goal_tracking().is_some(),
        "goal_defined": false,
        "active_attempt": null,
        "goal_revision": null,
        "entry_graph_node_id": base.graph().entry(),
    }) else {
        unreachable!("start preview is an object");
    };
    Ok(local_result_v2(
        command,
        result,
        format!("dry run: entry graph node {}", base.graph().entry()),
        0,
    ))
}
