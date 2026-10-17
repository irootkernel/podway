//! Interactive start resolution and independent fresh-source previews.

use std::{
    io::{self, IsTerminal, Write},
    path::Path,
};

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{PROCEDURE_SCHEMA_V2, Sha256Digest};
use podway_presets::catalog_v2;
use serde_json::{Value, json};

use super::{
    Cli, Command, ExistingSessionPolicy, LocalFailure, RunResult, StartArgs,
    StartTerminalDisposition, StatusPreflight, local_result_v2, preset_failure,
    procedure_config_failure, procedure_format, read_worktree_procedure, workspace_target,
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

pub(super) fn execute_start_dry_run(cli: &Cli) -> Result<RunResult, LocalFailure> {
    let Command::Start(args) = &cli.command else {
        return Err(LocalFailure::request_invalid(
            "dry-run requires session start",
        ));
    };
    if let Some(preset_name) = &args.preset {
        let preset = catalog_v2().lookup(preset_name).ok_or_else(|| {
            LocalFailure::preset_not_found("unknown preset").with_command("session.start")
        })?;
        let admitted = preset
            .validate()
            .map_err(|error| preset_failure(error).with_command("session.start"))?;
        let command = cli
            .command
            .direct_route()
            .ok_or_else(|| LocalFailure::request_invalid("invalid start command"))?;
        let result = json!({
            "schema": "podway.session-start-result/v4",
            "procedure_schema": PROCEDURE_SCHEMA_V2,
            "procedure_digest": admitted.digest().as_str(),
            "dry_run": true,
            "goal_tracking": admitted.parsed().goal_tracking().is_some(),
            "session_state": "prepared",
            "goal_defined": false,
            "active_attempt": Value::Null,
            "goal_revision": Value::Null,
        })
        .as_object()
        .cloned()
        .expect("v2 preset dry-run result is an object");
        let text = format!(
            "dry run: entry graph node {}",
            admitted.parsed().graph().entry()
        );
        return Ok(local_result_v2(command, result, text, 0));
    }

    let procedure = args
        .procedure
        .as_deref()
        .ok_or_else(|| LocalFailure::request_invalid("start requires a preset or procedure"))?;
    let root = workspace_target(cli.worktree.clone())?;
    let bytes = read_worktree_procedure(&root, Path::new(procedure))
        .map_err(|failure| failure.with_command("session.start"))?;
    let format = procedure_format(Path::new(procedure));
    execute_start_dry_run_v2(cli, args, procedure, &bytes, format)
}

fn execute_start_dry_run_v2(
    cli: &Cli,
    args: &StartArgs,
    procedure: &str,
    bytes: &[u8],
    format: ProcedureDocumentFormat,
) -> Result<RunResult, LocalFailure> {
    let source = std::str::from_utf8(bytes).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command("session.start")
    })?;
    let ParsedProcedure::V2(parsed) = parse_procedure_document(bytes, format)
        .map_err(|error| procedure_config_failure(error).with_command("session.start"))?;
    let validated = validate_procedure_v2(parsed)
        .map_err(|error| procedure_config_failure(error).with_command("session.start"))?;
    let context = AuthoringContext::new(procedure, source, format);
    if !vet_procedure_v2(&validated, &context).is_empty() {
        return Err(
            LocalFailure::procedure_invalid("Procedure v2 graph vetting failed")
                .with_command("session.start"),
        );
    }
    let expected = args.expect_procedure_digest.as_deref().ok_or_else(|| {
        LocalFailure::digest_confirmation_required(validated.digest(), "session.start")
    })?;
    let expected = Sha256Digest::new(expected.to_owned()).map_err(|_| {
        LocalFailure::request_invalid("expected procedure digest must be sha256:<lowercase-hex>")
            .with_command("session.start")
    })?;
    if validated.digest() != &expected {
        return Err(LocalFailure::procedure_digest_mismatch(
            &expected,
            validated.digest(),
            "session.start",
        ));
    }
    let command = cli
        .command
        .direct_route()
        .ok_or_else(|| LocalFailure::request_invalid("invalid start command"))?;
    let result = json!({
        "schema": "podway.session-start-result/v4",
        "procedure_schema": PROCEDURE_SCHEMA_V2,
        "procedure_digest": validated.digest().as_str(),
        "dry_run": true,
        "goal_tracking": validated.parsed().goal_tracking().is_some(),
        "session_state": "prepared",
        "goal_defined": false,
        "active_attempt": Value::Null,
        "goal_revision": Value::Null,
    })
    .as_object()
    .cloned()
    .expect("custom v2 dry-run result is an object");
    let text = format!(
        "dry run: entry graph node {}",
        validated.parsed().graph().entry()
    );
    Ok(local_result_v2(command, result, text, 0))
}
