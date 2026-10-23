//! Public command-line surface for the Podway command contract.

mod authoring;
mod grammar;
mod preflight;
mod procedure_files;
mod render;
mod start;

mod completion;
mod direct;
mod retirement;

#[cfg(debug_assertions)]
use std::ffi::OsStr;
use std::{
    env,
    ffi::OsString,
    fs::{self},
    io::{self, IsTerminal, Read, Write},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use clap::{CommandFactory, Parser};
use podway_config::ConfigError;
use podway_core::{
    ActorAttributionV2, AttemptId, CriterionAssessmentReasonV2, CriterionId, GoalCriterionV2,
    GoalDefinitionV2, GoalRevisionNumberV2, GoalRevisionReasonV2, GoalStatementV2, GraphNodeId,
    ItemId, OptionId, ReasonV2, Revision, SessionId, Sha256Digest, WorkspaceId,
};
use podway_presets::PresetError;
#[cfg(test)]
use podway_protocol::CommandNameV1;
use podway_protocol::{
    MAX_DIRECT_INPUT_BYTES_V1, MAX_SLICE_ITEM_TEXT_SCALARS_V1, RequestIdV1, Rfc3339MillisV1,
    build_identity_v1, decode_item_record_many_input_v1,
    ensure_procedure_independent_result_schema_v1, validate_command_result_v2,
    validate_procedure_independent_result_v1,
};
use serde_json::{Map, Value, json};
use uuid::Uuid;

#[cfg(test)]
use authoring::procedure_validate_v2_diagnostics;
use authoring::{
    PROCEDURE_FORMAT_COMMAND, execute_preset, execute_procedure,
    is_worktree_relative_procedure_path,
};
use grammar::{
    ArchiveCommand, Cli, Command, DelegationCommand, DispositionCommand, EvidenceCommand,
    ExistingSessionPolicy, GoalCommand, OutcomeCommand, PresetCommand, ProcedureCommand, ReadArgs,
    StartArgs, StartTerminalDisposition, WorkspaceCommand, help_text,
    normalize_top_level_help_alias, parse_failure_command_context,
};
#[cfg(test)]
use grammar::{ParseFailureCommandContext, parse_timeout_millis};
use preflight::{StatusPreflight, fill_facts};
use procedure_files::{
    OpenedProcedure, open_descriptor_relative_procedure, open_offline_procedure, procedure_format,
    read_offline_procedure, read_worktree_procedure, replace_procedure_document,
};
#[cfg(test)]
use render::{
    contains_check_result_projection_v1, render_human_error_direct,
    render_local_failure_with_clock_and_writers, render_result_with_clock_and_writers,
    unsatisfied_check_result_lines_v1,
};
use render::{
    render_authoring_diagnostics, render_local_failure, render_preview_start_command,
    render_preview_text_value, render_result, render_retirement, write_text_line,
};
use start::{ExistingStartResolution, execute_start_dry_run, resolve_existing_start};

const MAX_DIRECT_COMMAND_TIMEOUT_MILLIS_V1: u64 = 3_600_000;
const LOCAL_USAGE_EXIT: i32 = 2;
const LOCAL_CLIENT_EXIT: i32 = 6;

#[derive(Clone, Debug)]
struct WorkspaceTarget {
    path: PathBuf,
    path_bytes: Vec<u8>,
    display: String,
}

#[derive(Clone, Debug, Default)]
struct ExplicitPreconditions {
    workspace_id: Option<WorkspaceId>,
    session_id: Option<SessionId>,
    session_revision: Option<Revision>,
    attempt_id: Option<AttemptId>,
    item_revision: Option<Revision>,
    goal_revision: Option<GoalRevisionNumberV2>,
}

impl ExplicitPreconditions {
    fn parse(cli: &Cli) -> Result<Self, LocalFailure> {
        let workspace_id = cli
            .if_workspace_uuid
            .as_deref()
            .map(|id| {
                WorkspaceId::new(id.to_owned())
                    .map_err(|_| LocalFailure::request_invalid("invalid workspace identifier"))
            })
            .transpose()?;
        let session_id = cli
            .if_session_id
            .as_deref()
            .map(|id| {
                SessionId::new(id.to_owned())
                    .map_err(|_| LocalFailure::request_invalid("invalid session identifier"))
            })
            .transpose()?;
        let attempt_id = cli
            .if_attempt
            .as_deref()
            .map(|id| {
                AttemptId::new(id.to_owned())
                    .map_err(|_| LocalFailure::request_invalid("invalid attempt identifier"))
            })
            .transpose()?;
        Ok(Self {
            workspace_id,
            session_id,
            session_revision: cli.if_session_revision.map(Revision::new),
            attempt_id,
            item_revision: cli.if_item_revision.map(Revision::new),
            goal_revision: cli.if_goal_revision.map(GoalRevisionNumberV2::new),
        })
    }

    const fn any_transition(&self) -> bool {
        self.session_revision.is_some()
            || self.attempt_id.is_some()
            || self.item_revision.is_some()
            || self.goal_revision.is_some()
    }
}

#[derive(Clone, Debug)]
struct LocalFailure {
    code: &'static str,
    message: String,
    retryable: bool,
    exit_code: i32,
    command: String,
    request_id: Option<String>,
    details: Map<String, Value>,
}

impl LocalFailure {
    fn catalog(code: &'static str, message: impl Into<String>, command: impl Into<String>) -> Self {
        let (exit_code, retryable) = match code {
            "REQUEST_INVALID" | "REQUEST_TOO_LARGE" | "CONFIRMATION_REQUIRED" => {
                (LOCAL_USAGE_EXIT, false)
            }
            "PRESET_NOT_FOUND"
            | "PROCEDURE_NOT_FOUND"
            | "PROCEDURE_INVALID"
            | "PROCEDURE_SCHEMA_UNSUPPORTED" => (1, false),
            "GOAL_TRACKING_NOT_ENABLED" => (1, false),
            "SESSION_NOT_RUNNING" => (1, false),
            "SESSION_REVISION_CONFLICT" => (4, true),
            "ITEM_CONSTRAINT_FAILED" => (1, false),
            "DIGEST_CONFIRMATION_REQUIRED" => (LOCAL_USAGE_EXIT, false),
            "PROCEDURE_DIGEST_MISMATCH" => (4, false),
            "PATH_OUTSIDE_WORKTREE" => (5, false),
            "SESSION_START_DECISION_REQUIRED" => (5, false),
            "INTERNAL_ERROR" => (LOCAL_CLIENT_EXIT, false),
            _ => unreachable!("local failures must use a catalogued error code"),
        };
        Self {
            code,
            message: message.into(),
            retryable,
            exit_code,
            command: command.into(),
            request_id: None,
            details: if code == "INTERNAL_ERROR" {
                Map::from_iter([
                    (
                        "schema".to_owned(),
                        json!("podway.internal-error-details/v2"),
                    ),
                    (
                        "diagnostic_id".to_owned(),
                        json!(Uuid::new_v4().to_string()),
                    ),
                ])
            } else {
                Map::new()
            },
        }
    }

    fn request_invalid(message: impl Into<String>) -> Self {
        Self::catalog("REQUEST_INVALID", message, "cli")
    }

    fn session_revision_conflict(command: &str, expected: Revision, current: Revision) -> Self {
        let mut failure = Self::catalog(
            "SESSION_REVISION_CONFLICT",
            "the session changed after it was observed",
            command,
        );
        failure.details = json!({
            "schema": "podway.revision-conflict-details/v3",
            "expected_revision": expected,
            "current_revision": current,
            "recovery": {
                "action": "refresh_state", "command": "session.observe",
                "argv": ["podway", "--json", "observe"],
                "reason": "Read the current committed state before retrying.",
                "requires_explicit_authorization": false,
            },
        })
        .as_object()
        .expect("revision conflict details are an object")
        .clone();
        failure
    }

    fn confirmation_required(command: &str) -> Self {
        Self::catalog(
            "CONFIRMATION_REQUIRED",
            if command == "workspace.remove" {
                "explicit confirmation is required: use --yes --if-workspace-uuid <uuid> for non-interactive removal, or a human terminal to type the exact worktree path for unfenced removal"
            } else {
                "explicit confirmation is required"
            },
            command,
        )
    }

    fn response_invalid(message: impl Into<String>) -> Self {
        Self::catalog("INTERNAL_ERROR", message, "cli")
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::catalog("INTERNAL_ERROR", message, "cli")
    }

    fn session_start_decision_required(
        lifecycle: &str,
        current_terminal_disposition: bool,
        allowed_actions: &[&str],
    ) -> Self {
        let mut failure = Self::catalog(
            "SESSION_START_DECISION_REQUIRED",
            "starting a new session requires an explicit existing-session decision",
            "session.start",
        );
        failure.details = Map::from_iter([
            (
                "schema".to_owned(),
                Value::String("podway.session-start-decision-required-details/v2".to_owned()),
            ),
            ("lifecycle".to_owned(), Value::String(lifecycle.to_owned())),
            (
                "current_terminal_disposition".to_owned(),
                Value::Bool(current_terminal_disposition),
            ),
            (
                "allowed_actions".to_owned(),
                Value::Array(
                    allowed_actions
                        .iter()
                        .map(|value| Value::String((*value).to_owned()))
                        .collect(),
                ),
            ),
        ]);
        failure
    }

    fn procedure_not_found(message: impl Into<String>) -> Self {
        Self::catalog("PROCEDURE_NOT_FOUND", message, "procedure")
    }

    fn procedure_invalid(message: impl Into<String>) -> Self {
        Self::catalog("PROCEDURE_INVALID", message, "procedure")
    }

    fn preset_not_found(message: impl Into<String>) -> Self {
        Self::catalog("PRESET_NOT_FOUND", message, "preset")
    }

    fn procedure_digest_mismatch(
        expected: &Sha256Digest,
        actual: &Sha256Digest,
        command: &str,
    ) -> Self {
        let mut failure = Self::catalog(
            "PROCEDURE_DIGEST_MISMATCH",
            "The canonical Procedure digest differs from the expected digest.",
            command,
        );
        failure.details = Map::from_iter([
            (
                "schema".to_owned(),
                Value::String("podway.procedure-digest-mismatch-details/v3".to_owned()),
            ),
            (
                "expected_procedure_digest".to_owned(),
                Value::String(expected.as_str().to_owned()),
            ),
            (
                "actual_procedure_digest".to_owned(),
                Value::String(actual.as_str().to_owned()),
            ),
        ]);
        failure
    }

    fn digest_confirmation_required(actual: &Sha256Digest, command: &str) -> Self {
        let mut failure = Self::catalog(
            "DIGEST_CONFIRMATION_REQUIRED",
            "A custom Procedure v2 start requires explicit digest confirmation.",
            command,
        );
        failure.details = Map::from_iter([
            (
                "schema".to_owned(),
                Value::String("podway.v2-runtime-error-details/v2".to_owned()),
            ),
            (
                "kind".to_owned(),
                Value::String("DIGEST_CONFIRMATION_REQUIRED".to_owned()),
            ),
            (
                "procedure_digest".to_owned(),
                Value::String(actual.as_str().to_owned()),
            ),
        ]);
        failure
    }

    fn with_command(mut self, command: &str) -> Self {
        self.command = command.to_owned();
        self
    }
}

enum RunResult {
    Direct {
        response: Box<podway_engine::direct_api::DirectResponseV1>,
        procedure_check_path: Option<String>,
    },
    VersionSummary {
        name: &'static str,
        version: String,
        text: String,
    },
    Local {
        command: String,
        result: Map<String, Value>,
        text: String,
    },
    /// A local success carried by the v2 output envelope.
    ///
    /// Unlike [`RunResult::Local`] this can exit non-zero: an authoring command reports findings
    /// about a well-formed document as a success envelope with a domain exit code.
    LocalV2 {
        command: String,
        result: Map<String, Value>,
        /// The exact bytes the text renderer writes, newline included.
        text: String,
        exit_code: i32,
    },
}

fn preflight_error(
    command: &Command,
    error: podway_protocol::ErrorEnvelopeV2,
) -> Result<RunResult, LocalFailure> {
    let mut value = error.into_value();
    value["command"] = json!(command.canonical_route());
    let error = podway_protocol::ErrorEnvelopeV2::from_value(value).map_err(|_| {
        LocalFailure::internal("cannot attribute the preflight error to the requested command")
    })?;
    Ok(direct_response_result(
        command,
        podway_engine::direct_api::DirectResponseV1::Failure(error),
    ))
}

fn direct_response_result(
    command: &Command,
    response: podway_engine::direct_api::DirectResponseV1,
) -> RunResult {
    let procedure_check_path = match command {
        Command::Start(StartArgs {
            procedure: Some(procedure),
            ..
        }) => Some(procedure.clone()),
        _ => None,
    };
    RunResult::Direct {
        response: Box::new(response),
        procedure_check_path,
    }
}

/// Runs the CLI and returns its process exit code.
pub fn run() -> i32 {
    #[cfg(debug_assertions)]
    {
        const PROBE_ARGUMENT: &str = "--podway-test-isolation-probe";
        const PROBE_TOKEN: &str = "podway-test-isolation-v1";
        if env::args_os().nth(1).as_deref() == Some(OsStr::new(PROBE_ARGUMENT))
            && env::args_os().nth(2).is_none()
            && env::var_os("PODWAY_TEST_ISOLATION_PROBE").as_deref()
                == Some(OsStr::new(PROBE_TOKEN))
        {
            println!("{PROBE_TOKEN}");
            return 0;
        }
    }

    let mut arguments: Vec<OsString> = env::args_os().collect();
    normalize_top_level_help_alias(&mut arguments);
    let mut grammar = Cli::command();
    grammar.build();
    let classification = match retirement::classify(&arguments, &grammar) {
        Ok(classification) => classification,
        Err(_) => {
            return render_local_failure(
                LocalFailure::internal("the embedded retirement table is invalid"),
                false,
            );
        }
    };
    let json_requested = classification.json_requested;
    if let Some(retired) = classification.retired {
        return render_retirement(retired, json_requested);
    }
    match Cli::try_parse_from(&arguments) {
        Ok(cli) => {
            let route = cli.command.canonical_route();
            let json_output = cli.json;
            let quiet = cli.quiet;
            let exit = match execute(cli) {
                Ok(result) => render_result(&result, json_output, quiet),
                Err(failure) => render_local_failure(failure.with_command(route), json_output),
            };
            #[cfg(debug_assertions)]
            if env::var_os("PODWAY_TEST_PREFLIGHT_COUNT").as_deref()
                == Some(OsStr::new("podway-direct-preflight-test-v1"))
            {
                let (facts, status) =
                    podway_engine::direct_api::reads004_debug_preflight_counters_v1();
                let _ = writeln!(
                    io::stderr().lock(),
                    "podway-test-preflight-count:{facts}:{status}"
                );
                let (retained, consumed, reused) =
                    podway_engine::direct_api::reads004_debug_graph_hint_counters_v1();
                let _ = writeln!(
                    io::stderr().lock(),
                    "podway-test-graph-hint-count:{retained}:{consumed}:{reused}"
                );
            }
            exit
        }
        Err(_) => {
            let failure = LocalFailure::request_invalid("invalid command syntax");
            let failure = match parse_failure_command_context(&arguments) {
                Some(context) => failure.with_command(context.route),
                None => failure,
            };
            render_local_failure(failure, json_requested)
        }
    }
}

fn execute(cli: Cli) -> Result<RunResult, LocalFailure> {
    direct::execute(cli)
}

fn apply_archive_purge_revision(
    command: &mut Command,
    explicit: &ExplicitPreconditions,
) -> Result<(), LocalFailure> {
    let Command::Archive {
        command:
            Some(ArchiveCommand::Purge {
                expected_session_revision,
                ..
            }),
    } = command
    else {
        return Ok(());
    };
    *expected_session_revision = Some(
        explicit
            .session_revision
            .ok_or_else(|| {
                LocalFailure::request_invalid("archive purge requires --if-session-revision")
            })?
            .get(),
    );
    Ok(())
}
fn procedure_config_failure(error: ConfigError) -> LocalFailure {
    let code = match error {
        ConfigError::InvalidSchema { .. } => "PROCEDURE_SCHEMA_UNSUPPORTED",
        _ => "PROCEDURE_INVALID",
    };
    LocalFailure::catalog(code, error.to_string(), "procedure")
}
fn preset_failure(error: PresetError) -> LocalFailure {
    LocalFailure::procedure_invalid(error.to_string())
}

fn execute_local(cli: &Cli) -> Result<Option<RunResult>, LocalFailure> {
    let workspace_options = supplied_workspace_options(cli);
    match &cli.command {
        Command::Help { topic } => {
            reject_local_workspace_options(
                &workspace_options,
                "help runs locally and opens no workspace",
            )?;
            let text = help_text(topic)?;
            let topic = if topic.is_empty() {
                None
            } else {
                Some(topic.join(" "))
            };
            Ok(Some(local_result(
                "help",
                json!({ "topic": topic, "text": text }),
                text,
            )))
        }
        Command::Version { identity } => {
            reject_local_workspace_options(
                &workspace_options,
                "version runs locally and opens no workspace",
            )?;
            if !identity {
                return Ok(Some(RunResult::VersionSummary {
                    name: "podway",
                    version: format!("v{}", env!("CARGO_PKG_VERSION")),
                    text: format!("podway v{}", env!("CARGO_PKG_VERSION")),
                }));
            }
            if !cli.json {
                return Err(LocalFailure::request_invalid(
                    "version --identity requires --json",
                ));
            }
            let identity = build_identity_v1();
            Ok(Some(local_result(
                "version",
                serde_json::to_value(identity)
                    .expect("the static build identity always serializes"),
                format!("podway v{}", env!("CARGO_PKG_VERSION")),
            )))
        }
        Command::Completions { shell } => {
            reject_local_workspace_options(
                &workspace_options,
                "completions runs locally and opens no workspace",
            )?;
            Ok(Some(local_result(
                "completions",
                json!({ "shell": format!("{shell:?}").to_lowercase(), "script": shell.script() }),
                shell.script(),
            )))
        }
        Command::Preset { command } => {
            reject_local_workspace_options(
                &workspace_options,
                "preset commands run locally and open no workspace",
            )?;
            Ok(Some(execute_preset(command)?))
        }
        Command::Procedure { command } => {
            reject_local_workspace_options(
                &workspace_options,
                "procedure commands run locally and open no workspace",
            )?;
            Ok(Some(execute_procedure(command)?))
        }
        _ => Ok(None),
    }
}

/// The worktree request options a local command must reject, in the fixed order
/// its rejection message names them.
fn supplied_workspace_options(cli: &Cli) -> Vec<&'static str> {
    [
        ("--worktree", cli.worktree.is_some()),
        ("--timeout", cli.timeout.is_some()),
        ("--idempotency-key", cli.idempotency_key.is_some()),
        ("--if-workspace-uuid", cli.if_workspace_uuid.is_some()),
        ("--if-session-id", cli.if_session_id.is_some()),
        ("--if-session-revision", cli.if_session_revision.is_some()),
        ("--if-attempt", cli.if_attempt.is_some()),
        ("--if-item-revision", cli.if_item_revision.is_some()),
        ("--if-goal-revision", cli.if_goal_revision.is_some()),
        ("--yes", cli.yes),
    ]
    .into_iter()
    .filter(|(_, supplied)| *supplied)
    .map(|(option, _)| option)
    .collect()
}

fn reject_local_workspace_options(
    supplied: &[&'static str],
    subject: &'static str,
) -> Result<(), LocalFailure> {
    if supplied.is_empty() {
        Ok(())
    } else {
        Err(LocalFailure::request_invalid(format!(
            "{subject}; omit {}",
            supplied.join(", ")
        )))
    }
}

fn local_result(command: &str, mut result: Value, text: String) -> RunResult {
    ensure_procedure_independent_result_schema_v1(
        command,
        result
            .as_object_mut()
            .expect("local result is always an object"),
    );
    validate_procedure_independent_result_v1(command, &result)
        .expect("local command result must satisfy its closed protocol contract");
    let Value::Object(result) = result else {
        unreachable!("local result is always an object");
    };
    RunResult::Local {
        command: command.to_owned(),
        result,
        text,
    }
}

/// Builds one v2 local success.
///
/// The v2 registry never infers a discriminator — a single route can carry two closed families —
/// so the caller sets `result["schema"]` and this asserts the choice against the contract.
fn local_result_v2(
    command: &str,
    result: Map<String, Value>,
    text: String,
    exit_code: i32,
) -> RunResult {
    let result = Value::Object(result);
    if result.get("schema").and_then(Value::as_str) == Some("podway.session-start-result/v4") {
        assert!(
            podway_protocol::validate_daemon_free_document_v1(
                "podway.session-start-result/v4",
                &result
            ),
            "start preview must satisfy its current contract"
        );
    } else {
        validate_command_result_v2(command, &result)
            .expect("local v2 results must satisfy their closed contract");
    }
    assert!(
        exit_code == 0 || exit_code == 1,
        "a v2 success envelope exits clean or domain, never usage or higher",
    );
    let Value::Object(result) = result else {
        unreachable!("local v2 result is always an object");
    };
    RunResult::LocalV2 {
        command: command.to_owned(),
        result,
        text,
        exit_code,
    }
}

fn validate_direct_flags(cli: &Cli) -> Result<(), LocalFailure> {
    let command = &cli.command;
    let wire = command
        .direct_route()
        .ok_or_else(|| LocalFailure::request_invalid("unsupported command"))?;
    let lookup = matches!(
        command,
        Command::Outcome {
            command: OutcomeCommand::Lookup
        }
    );
    if lookup && cli.idempotency_key.is_none() {
        return Err(LocalFailure::request_invalid(
            "outcome lookup requires --idempotency-key",
        ));
    }
    if !command.is_mutation() && cli.idempotency_key.is_some() && !lookup {
        return Err(LocalFailure::request_invalid(
            "--idempotency-key applies only to mutations or outcome lookup",
        ));
    }
    if command.is_dry_run() && cli.idempotency_key.is_some() {
        return Err(LocalFailure::request_invalid(
            "dry-run commands cannot use an idempotency key",
        ));
    }
    let explicit = ExplicitPreconditions::parse(cli)?;
    if cli.if_workspace_uuid.is_some() && !command.accepts_workspace_identity() {
        return Err(LocalFailure::request_invalid(
            "--if-workspace-uuid does not apply to this command",
        ));
    }
    if cli.if_session_id.is_some() && !command.accepts_session_identity() {
        return Err(LocalFailure::request_invalid(
            "--if-session-id does not apply to this command",
        ));
    }
    // A plain start carries the paired optional session fences; the Engine
    // verifies them inside the start writer transaction.
    let plain_start = matches!(
        command,
        Command::Start(args)
            if args.on_existing.is_none()
                && !args.auto_archive_terminal
                && args.terminal_disposition.is_none()
    );
    if plain_start && (cli.if_session_id.is_some() != cli.if_session_revision.is_some()) {
        return Err(LocalFailure::request_invalid(
            "plain start requires --if-session-id and --if-session-revision together",
        ));
    }
    let plain_start_fenced = plain_start && cli.if_session_id.is_some();
    if explicit.any_transition()
        && !command.needs_preflight()
        && !plain_start_fenced
        && !(matches!(command, Command::Evidence { .. }) && explicit.session_revision.is_none())
        && !matches!(
            command,
            Command::Delegation {
                command: DelegationCommand::Accept(_)
            }
        )
    {
        return Err(LocalFailure::request_invalid(
            "explicit preconditions do not apply to this command",
        ));
    }
    if cli.if_session_revision.is_some() && command.is_item_mutation() {
        return Err(LocalFailure::request_invalid(
            "--if-session-revision does not apply to item commands",
        ));
    }
    if cli.if_item_revision.is_some() && !command.is_item_mutation() {
        return Err(LocalFailure::request_invalid(
            "--if-item-revision applies only to item commands",
        ));
    }
    if cli.if_goal_revision.is_some()
        && !matches!(
            command,
            Command::Decide(_)
                | Command::Goal {
                    command: GoalCommand::Revise(_) | GoalCommand::AssessCriterion(_)
                }
        )
    {
        return Err(LocalFailure::request_invalid(
            "--if-goal-revision applies only to decide, goal revise, and goal assess-criterion",
        ));
    }
    if cli.if_attempt.is_some()
        && matches!(
            command,
            Command::Start(_) | Command::Begin(_) | Command::Disposition { .. } | Command::Reset(_)
        )
    {
        return Err(LocalFailure::request_invalid(
            "--if-attempt does not apply to this session transition",
        ));
    }
    if cli.yes && !command.is_destructive() {
        return Err(LocalFailure::request_invalid(
            "--yes applies only to destructive commands",
        ));
    }
    if matches!(
        command,
        Command::Archive {
            command: Some(ArchiveCommand::Purge { .. })
        }
    ) && !cli.yes
    {
        return Err(LocalFailure::request_invalid(
            "archive purge requires --yes",
        ));
    }
    if cli.json
        && matches!(
            command,
            Command::Workspace {
                command: WorkspaceCommand::Remove { force: false }
            }
        )
    {
        return Err(LocalFailure::request_invalid(
            "JSON workspace removal requires --force",
        ));
    }
    if matches!(
        command,
        Command::Workspace {
            command: WorkspaceCommand::Remove { .. }
        }
    ) && (cli.json || !io::stdin().is_terminal())
        && (!cli.yes || explicit.workspace_id.is_none())
    {
        return Err(LocalFailure::request_invalid(
            "non-interactive workspace removal requires --yes and --if-workspace-uuid",
        ));
    }
    if matches!(
        command,
        Command::Next(ReadArgs { verbose: true, .. })
            | Command::Observe(ReadArgs { verbose: true, .. })
    ) {
        return Err(LocalFailure::request_invalid(
            "--verbose applies only to status",
        ));
    }
    if let (true, Command::Reset(args)) = (wire == "workspace.reset_all", command) {
        if args.dry_run {
            return Err(LocalFailure::request_invalid(
                "reset --all does not support --dry-run",
            ));
        }
        if args.progress_summary.is_some() {
            return Err(LocalFailure::request_invalid(
                "reset --all does not support --progress-summary",
            ));
        }
    }
    match command {
        Command::Start(StartArgs {
            procedure: Some(procedure),
            ..
        }) if !is_worktree_relative_procedure_path(Path::new(procedure)) => {
            return Err(LocalFailure::request_invalid(
                "procedure must be worktree-relative",
            ));
        }
        _ => {}
    }
    match command {
        Command::Start(args)
            if args.supersede_reason.is_some()
                && args.on_existing != Some(ExistingSessionPolicy::Preserve) =>
        {
            return Err(LocalFailure::request_invalid(
                "--supersede-reason requires --on-existing preserve",
            ));
        }
        Command::Start(args)
            if args.actor.is_some()
                && args.on_existing != Some(ExistingSessionPolicy::Preserve) =>
        {
            return Err(LocalFailure::request_invalid(
                "--actor requires --on-existing preserve",
            ));
        }
        Command::Start(args)
            if args.on_existing == Some(ExistingSessionPolicy::Preserve)
                && args.supersede_reason.is_none() =>
        {
            return Err(LocalFailure::request_invalid(
                "--on-existing preserve requires --supersede-reason",
            ));
        }
        Command::Start(args)
            if args.progress_summary.is_some()
                && args.on_existing != Some(ExistingSessionPolicy::Delete) =>
        {
            return Err(LocalFailure::request_invalid(
                "--progress-summary requires --on-existing delete",
            ));
        }
        Command::Reset(args) if args.dry_run && args.progress_summary.is_some() => {
            return Err(LocalFailure::request_invalid(
                "reset --dry-run does not support --progress-summary",
            ));
        }
        Command::Reset(args)
            if !args.all && !args.dry_run && cli.yes && args.progress_summary.is_none() =>
        {
            return Err(LocalFailure::request_invalid(
                "reset --yes requires --progress-summary",
            ));
        }
        Command::Reset(args) if args.force && !args.all => {
            return Err(LocalFailure::request_invalid(
                "--force applies only to reset --all",
            ));
        }
        _ => {}
    }
    Ok(())
}

fn validate_command_shape(command: &Command) -> Result<(), LocalFailure> {
    if let Some(id) = item_id(command) {
        ItemId::new(id.to_owned()).map_err(invalid_v2_value)?;
    }
    if let Command::Unblock {
        blocker_id: Some(id),
        ..
    } = command
    {
        podway_core::BlockerId::new(id.clone()).map_err(invalid_v2_value)?;
    }
    if let Command::Start(StartArgs {
        expect_procedure_digest: Some(digest),
        ..
    }) = command
    {
        Sha256Digest::new(digest.clone()).map_err(|_| {
            LocalFailure::request_invalid(
                "expected procedure digest must be sha256:<lowercase-hex>",
            )
        })?;
    }
    if let Command::Attach(args) = command {
        let reference_mode = args.reference.is_some();
        let incomplete_reference = reference_mode
            && (args.digest.is_none() || args.size.is_none() || args.media_type.is_none());
        let orphaned_reference_metadata =
            !reference_mode && (args.digest.is_some() || args.size.is_some());
        if incomplete_reference || orphaned_reference_metadata {
            return Err(LocalFailure::request_invalid(
                "reference attachment requires --reference, --digest, --size, and --media-type together",
            ));
        }
        if let Some(digest) = &args.digest {
            Sha256Digest::new(digest.clone()).map_err(|_| {
                LocalFailure::request_invalid("attachment digest must be sha256:<hex>")
            })?;
        }
    }
    match command {
        Command::Archive {
            command: Some(ArchiveCommand::Show { session_id, .. }),
        }
        | Command::Archive {
            command: Some(ArchiveCommand::Purge { session_id, .. }),
        } => {
            SessionId::new(session_id.clone())
                .map_err(|_| LocalFailure::request_invalid("archive session ID is invalid"))?;
        }
        _ => {}
    }
    match command {
        Command::Start(args) => {
            validate_optional_lifecycle_text(args.progress_summary.as_deref(), "progress summary")?;
            validate_optional_lifecycle_text(args.supersede_reason.as_deref(), "supersede reason")?;
            validate_actor(args.actor.as_deref())?;
        }
        Command::Reset(args) => {
            validate_optional_lifecycle_text(args.progress_summary.as_deref(), "progress summary")?;
        }
        Command::Begin(args) => {
            validate_optional_goal(&args.goal, &args.criterion, args.actor.as_deref())?;
        }
        Command::Disposition { command } => match command {
            DispositionCommand::HandedOff {
                summary,
                reference,
                actor,
            } => {
                validate_lifecycle_text_cli(summary, "summary")?;
                validate_lifecycle_text_cli(reference, "reference")?;
                validate_actor(actor.as_deref())?;
            }
            DispositionCommand::NotRequired { reason, actor } => {
                validate_lifecycle_text_cli(reason, "reason")?;
                validate_actor(actor.as_deref())?;
            }
        },
        Command::Decide(args) => {
            OptionId::new(args.option.clone()).map_err(invalid_v2_value)?;
            ReasonV2::new(args.reason.clone()).map_err(invalid_v2_value)?;
            validate_actor(args.actor.as_deref())?;
        }
        Command::Rework(args) => {
            GraphNodeId::new(args.to.clone()).map_err(invalid_v2_value)?;
            ReasonV2::new(args.reason.clone()).map_err(invalid_v2_value)?;
            validate_actor(args.actor.as_deref())?;
        }
        Command::Goal {
            command: GoalCommand::Define(args),
        } => validate_goal(&args.goal, &args.criterion, args.actor.as_deref())?,
        Command::Goal {
            command: GoalCommand::Revise(args),
        } => {
            validate_goal(&args.goal, &args.criterion, args.actor.as_deref())?;
            GraphNodeId::new(args.rework_to.clone()).map_err(invalid_v2_value)?;
            GoalRevisionReasonV2::new(args.reason.clone()).map_err(invalid_v2_value)?;
        }
        Command::Goal {
            command: GoalCommand::AssessCriterion(args),
        } => {
            CriterionId::new(args.criterion_id.clone()).map_err(invalid_v2_value)?;
            CriterionAssessmentReasonV2::new(args.reason.clone()).map_err(invalid_v2_value)?;
            validate_actor(args.actor.as_deref())?;
            if args.evidence.len() + args.item.len() > 4 {
                return Err(LocalFailure::request_invalid(
                    "criterion assessment permits at most four citations",
                ));
            }
            if args.status == "not_applicable"
                && (!args.evidence.is_empty() || !args.item.is_empty())
            {
                return Err(LocalFailure::request_invalid(
                    "not_applicable criterion assessments cannot cite evidence or items",
                ));
            }
            for evidence in &args.evidence {
                GraphNodeId::new(evidence.clone()).map_err(invalid_v2_value)?;
            }
            for item in &args.item {
                ItemId::new(item.clone()).map_err(invalid_v2_value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn invalid_v2_value(error: impl std::fmt::Display) -> LocalFailure {
    LocalFailure::request_invalid(error.to_string())
}

fn validate_lifecycle_text_cli(value: &str, field: &str) -> Result<(), LocalFailure> {
    if value.trim().is_empty() || value.chars().count() > 4_000 {
        return Err(LocalFailure::request_invalid(format!(
            "{field} must contain 1..=4000 Unicode scalars"
        )));
    }
    Ok(())
}

fn validate_optional_lifecycle_text(value: Option<&str>, field: &str) -> Result<(), LocalFailure> {
    value.map_or(Ok(()), |value| validate_lifecycle_text_cli(value, field))
}

fn validate_actor(actor: Option<&str>) -> Result<(), LocalFailure> {
    actor
        .map(|actor| ActorAttributionV2::new(actor.to_owned()))
        .transpose()
        .map(drop)
        .map_err(invalid_v2_value)
}

fn validate_optional_goal(
    goal: &Option<String>,
    criteria: &[String],
    actor: Option<&str>,
) -> Result<(), LocalFailure> {
    match (goal, criteria.is_empty(), actor) {
        (None, true, None) => Ok(()),
        (Some(goal), false, actor) => validate_goal(goal, criteria, actor),
        _ => Err(LocalFailure::request_invalid(
            "--goal requires at least one --criterion, and criteria or actor require --goal",
        )),
    }
}

fn validate_goal(goal: &str, criteria: &[String], actor: Option<&str>) -> Result<(), LocalFailure> {
    GoalStatementV2::new(goal.to_owned()).map_err(invalid_v2_value)?;
    let criteria = parse_criteria(criteria)?;
    GoalDefinitionV2::new(criteria)
        .map(drop)
        .map_err(invalid_v2_value)?;
    validate_actor(actor)
}

fn parse_criteria(criteria: &[String]) -> Result<Vec<GoalCriterionV2>, LocalFailure> {
    criteria
        .iter()
        .map(|criterion| {
            let (id, statement) = criterion.split_once('=').ok_or_else(|| {
                LocalFailure::request_invalid("criterion must use <criterion-id>=<statement>")
            })?;
            let id = CriterionId::new(id.to_owned()).map_err(invalid_v2_value)?;
            GoalCriterionV2::new(id, statement.to_owned()).map_err(invalid_v2_value)
        })
        .collect()
}

fn confirm_if_required(
    cli: &Cli,
    command: &str,
    target: &WorkspaceTarget,
) -> Result<(), LocalFailure> {
    if matches!(cli.command, Command::Start(_)) {
        return Ok(());
    }
    let unfenced_removal = matches!(
        cli.command,
        Command::Workspace {
            command: WorkspaceCommand::Remove { .. }
        }
    ) && cli.if_workspace_uuid.is_none();
    if !cli.command.is_destructive() || (cli.yes && !unfenced_removal) {
        return Ok(());
    }
    if cli.json || !io::stdin().is_terminal() {
        return Err(LocalFailure::confirmation_required(command));
    }
    if matches!(
        cli.command,
        Command::Workspace {
            command: WorkspaceCommand::Remove { .. }
        }
    ) {
        let exact_path = workspace_removal_confirmation_path(target)?;
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "Resolved worktree root: {exact_path}")
            .and_then(|()| {
                write!(
                    stdout,
                    "Type that exact path to remove all .podway content: "
                )
            })
            .and_then(|()| stdout.flush())
            .map_err(|_| LocalFailure::response_invalid("cannot write confirmation prompt"))?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer).map_err(|_| {
            LocalFailure::internal("cannot read confirmation input").with_command(command)
        })?;
        return if answer.trim_end_matches(['\r', '\n']) == exact_path {
            Ok(())
        } else {
            Err(LocalFailure::confirmation_required(command))
        };
    }
    let mut stdout = io::stdout().lock();
    write!(stdout, "This operation is destructive. Continue? [y/N] ")
        .map_err(|_| LocalFailure::response_invalid("cannot write confirmation prompt"))?;
    stdout
        .flush()
        .map_err(|_| LocalFailure::response_invalid("cannot write confirmation prompt"))?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).map_err(|_| {
        LocalFailure::internal("cannot read confirmation input").with_command(command)
    })?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(LocalFailure::confirmation_required(command))
    }
}

fn workspace_removal_confirmation_path(target: &WorkspaceTarget) -> Result<&str, LocalFailure> {
    target.path.to_str().ok_or_else(|| {
        LocalFailure::request_invalid(
            "interactive removal cannot confirm a non-UTF-8 worktree path; use --yes and --if-workspace-uuid",
        )
    })
}
fn prepare_stdin_payload(command: &mut Command) -> Result<(), LocalFailure> {
    match command {
        Command::Set(args) if args.stdin => {
            args.value = Some(read_stdin_text()?);
        }
        Command::Record(args) if args.stdin => {
            let mut bytes = Vec::with_capacity(MAX_DIRECT_INPUT_BYTES_V1.min(8 * 1024));
            io::stdin()
                .take((MAX_DIRECT_INPUT_BYTES_V1 + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| LocalFailure::request_invalid("cannot read record stdin"))?;
            args.input = Some(decode_item_record_many_input_v1(&bytes).map_err(|error| {
                if bytes.len() > MAX_DIRECT_INPUT_BYTES_V1 {
                    LocalFailure::catalog(
                        "REQUEST_TOO_LARGE",
                        "record stdin exceeds the maximum size",
                        "cli",
                    )
                } else {
                    match error {
                        podway_protocol::SliceErrorV1::BoundExceeded {
                            field,
                            maximum,
                            actual,
                            unit,
                        } => {
                            let mut failure = LocalFailure::catalog(
                                "ITEM_CONSTRAINT_FAILED",
                                format!(
                                    "{field} exceeds its maximum of {maximum} {unit} (received {actual})"
                                ),
                                "item.record_many",
                            );
                            failure.details = Map::from_iter([
                                (
                                    "schema".to_owned(),
                                    Value::String(
                                        "podway.v2-runtime-error-details/v2".to_owned(),
                                    ),
                                ),
                                (
                                    "kind".to_owned(),
                                    Value::String("ITEM_CONSTRAINT_FAILED".to_owned()),
                                ),
                                ("field".to_owned(), Value::String(field.to_owned())),
                                ("actual".to_owned(), json!(actual)),
                                ("maximum".to_owned(), json!(maximum)),
                                ("unit".to_owned(), Value::String(unit.to_owned())),
                                                ]);
                            failure
                        }
                        error => LocalFailure::request_invalid(error.to_string()),
                    }
                }
            })?);
        }
        Command::Delegation {
            command: DelegationCommand::Accept(args),
        } if args.stdin => {
            const MAX_BYTES: usize = 32 * 1_024;
            let mut bytes = Vec::with_capacity(MAX_BYTES);
            io::stdin()
                .take((MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| {
                    LocalFailure::request_invalid("cannot read delegation acceptance stdin")
                })?;
            if bytes.len() > MAX_BYTES {
                return Err(LocalFailure::catalog(
                    "REQUEST_TOO_LARGE",
                    "delegation acceptance stdin exceeds 32 KiB",
                    "cli",
                ));
            }
            let input: Value = serde_json::from_slice(&bytes).map_err(|_| {
                LocalFailure::request_invalid("delegation acceptance stdin is not JSON")
            })?;
            if !podway_protocol::validate_delegation_document_v1(
                "podway.delegation-accept-input/v1",
                &input,
            ) {
                return Err(LocalFailure::request_invalid(
                    "delegation acceptance input is invalid",
                ));
            }
            args.input = Some(input);
        }
        _ => {}
    }
    Ok(())
}

fn apply_record_input(cli: &mut Cli) -> Result<(), LocalFailure> {
    if let Command::Delegation {
        command: DelegationCommand::Accept(args),
    } = &cli.command
    {
        let input = args
            .input
            .as_ref()
            .ok_or_else(|| LocalFailure::request_invalid("delegation accept requires --stdin"))?;
        if cli.idempotency_key.is_some()
            || cli.if_workspace_uuid.is_some()
            || cli.if_session_id.is_some()
            || cli.if_session_revision.is_some()
            || cli.if_attempt.is_some()
            || cli.if_item_revision.is_some()
            || cli.if_goal_revision.is_some()
        {
            return Err(LocalFailure::request_invalid(
                "delegation acceptance identity and idempotency must come only from stdin",
            ));
        }
        let field = |name: &str| {
            input
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    LocalFailure::request_invalid("delegation acceptance input identity is invalid")
                })
        };
        cli.idempotency_key = Some(field("idempotency_key")?);
        cli.if_workspace_uuid = Some(field("workspace_uuid")?);
        cli.if_session_id = Some(field("session_id")?);
        cli.if_session_revision = input.get("session_revision").and_then(Value::as_u64);
        cli.if_attempt = Some(field("attempt_id")?);
        return Ok(());
    }
    let input = match &cli.command {
        Command::Record(args) => args
            .input
            .clone()
            .ok_or_else(|| LocalFailure::request_invalid("record requires --stdin"))?,
        _ => return Ok(()),
    };
    if cli.idempotency_key.is_some()
        || cli.if_workspace_uuid.is_some()
        || cli.if_session_id.is_some()
        || cli.if_session_revision.is_some()
        || cli.if_attempt.is_some()
        || cli.if_item_revision.is_some()
        || cli.if_goal_revision.is_some()
    {
        return Err(LocalFailure::request_invalid(
            "record identity, revision, and idempotency fences must come only from stdin",
        ));
    }
    cli.idempotency_key = Some(input.idempotency_key.as_str().to_owned());
    cli.if_workspace_uuid = Some(input.workspace_uuid.as_str().to_owned());
    cli.if_session_id = Some(input.session_id.as_str().to_owned());
    cli.if_session_revision = Some(input.session_revision.get());
    cli.if_attempt = Some(input.attempt_id.as_str().to_owned());
    Ok(())
}

fn read_stdin_text() -> Result<String, LocalFailure> {
    const MAX_BYTES: usize = MAX_SLICE_ITEM_TEXT_SCALARS_V1 * 4;
    let mut bytes = Vec::with_capacity(MAX_BYTES.min(8 * 1024));
    io::stdin()
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| LocalFailure::request_invalid("cannot read stdin text"))?;
    if bytes.len() > MAX_BYTES {
        return Err(LocalFailure::catalog(
            "REQUEST_TOO_LARGE",
            "stdin value exceeds the maximum size",
            "cli",
        ));
    }
    let value = String::from_utf8(bytes)
        .map_err(|_| LocalFailure::request_invalid("stdin text must be valid UTF-8"))?;
    if value.chars().count() > MAX_SLICE_ITEM_TEXT_SCALARS_V1 {
        return Err(LocalFailure::catalog(
            "REQUEST_TOO_LARGE",
            "stdin value exceeds the maximum size",
            "cli",
        ));
    }
    Ok(value)
}

fn item_id(command: &Command) -> Option<&str> {
    match command {
        Command::Check { item_id }
        | Command::Uncheck { item_id }
        | Command::Add { item_id, .. }
        | Command::Remove { item_id, .. }
        | Command::Clear { item_id } => Some(item_id),
        Command::Set(args) => Some(&args.item_id),
        Command::Attach(args) => Some(&args.item_id),
        _ => None,
    }
}

fn workspace_target(worktree: Option<PathBuf>) -> Result<WorkspaceTarget, LocalFailure> {
    let path = worktree.unwrap_or_else(|| PathBuf::from("."));
    let absolute = if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .map_err(|_| LocalFailure::internal("current directory cannot be resolved"))?
            .join(path)
    };
    let canonical = match fs::canonicalize(&absolute) {
        Ok(canonical) => canonical,
        Err(error) if error.kind() == io::ErrorKind::NotFound => absolute,
        Err(_) => {
            return Err(LocalFailure::request_invalid(
                "worktree path cannot be resolved",
            ));
        }
    };
    let path_bytes = canonical.as_os_str().as_bytes().to_vec();
    if path_bytes.is_empty() {
        return Err(LocalFailure::request_invalid("worktree path is empty"));
    }
    let display = canonical.display().to_string();
    Ok(WorkspaceTarget {
        path: canonical,
        path_bytes,
        display,
    })
}

fn workspace_removal_target(target: WorkspaceTarget) -> Result<WorkspaceTarget, LocalFailure> {
    let mut candidate = target.path.as_path();
    for _ in 0..128 {
        let marker = candidate.join(".git");
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_dir() || metadata.file_type().is_file() => {
                return workspace_target(Some(candidate.to_path_buf()));
            }
            Ok(_) => {
                return Err(LocalFailure::request_invalid(
                    "worktree Git marker is unsafe",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(LocalFailure::request_invalid(
                    "worktree Git marker cannot be inspected",
                ));
            }
        }
        let Some(parent) = candidate.parent() else {
            break;
        };
        candidate = parent;
    }
    Err(LocalFailure::request_invalid(
        "workspace removal requires an existing Git worktree",
    ))
}

trait LocalEnvelopeClock {
    fn now(&self) -> SystemTime;
}

#[derive(Clone, Copy, Debug, Default)]
struct SystemLocalEnvelopeClock;

impl LocalEnvelopeClock for SystemLocalEnvelopeClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

fn local_generated_at(clock: &impl LocalEnvelopeClock) -> Result<Rfc3339MillisV1, LocalFailure> {
    let milliseconds = clock
        .now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| LocalFailure::response_invalid("system clock is before the Unix epoch"))?
        .as_millis();
    let milliseconds = u64::try_from(milliseconds)
        .map_err(|_| LocalFailure::response_invalid("system clock is out of range"))?;
    let seconds = milliseconds / 1_000;
    let (year, month, day) = civil_date_from_unix_days(
        i64::try_from(seconds / 86_400)
            .map_err(|_| LocalFailure::response_invalid("system clock is out of range"))?,
    );
    if !(0..=9_999).contains(&year) {
        return Err(LocalFailure::response_invalid(
            "system clock is out of range",
        ));
    }
    let second_of_day = seconds % 86_400;
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;
    Rfc3339MillisV1::new(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        milliseconds % 1_000
    ))
    .map_err(|_| LocalFailure::response_invalid("system clock produced an invalid timestamp"))
}

fn civil_date_from_unix_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_parameter = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_parameter + 2) / 5 + 1;
    let month = month_parameter + if month_parameter < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month as u32, day as u32)
}

fn empty_dynamic_completion() -> RunResult {
    local_result("__complete", json!({ "candidates": [] }), String::new())
}

fn dynamic_candidates_v2(result: &Map<String, Value>, kind: &str) -> Vec<String> {
    if !matches!(
        result.get("schema").and_then(Value::as_str),
        Some("podway.status-result/v5")
    ) {
        return Vec::new();
    }
    match kind {
        "items" => candidate_ids(result, "items", "item_id"),
        "blockers" => candidate_ids(result, "blocker_window", "blocker_id"),
        "options" => candidate_strings(result, "allowed_option_ids"),
        "rework-targets" => candidate_strings(result, "allowed_manual_rework_targets"),
        "goal-criteria" => result
            .get("goal")
            .and_then(Value::as_object)
            .and_then(|goal| goal.get("criteria"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|criterion| {
                let criterion = criterion.as_object()?;
                (criterion.get("status").and_then(Value::as_str) == Some("unassessed"))
                    .then(|| criterion.get("criterion_id").and_then(Value::as_str))
                    .flatten()
                    .map(str::to_owned)
            })
            .take(128)
            .collect(),
        "evidence-sources" => result
            .get("references")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|reference| {
                let reference = reference.as_object()?;
                (reference.get("state").and_then(Value::as_str) == Some("resolved"))
                    .then(|| {
                        reference
                            .get("source_graph_node_id")
                            .and_then(Value::as_str)
                    })
                    .flatten()
                    .map(str::to_owned)
            })
            .take(128)
            .collect(),
        "item-values" => candidate_ids(result, "item_values", "item_id"),
        _ => Vec::new(),
    }
}

fn candidate_ids(result: &Map<String, Value>, collection: &str, field: &str) -> Vec<String> {
    result
        .get(collection)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|candidate| {
            candidate
                .as_object()
                .and_then(|candidate| candidate.get(field))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .take(128)
        .collect()
}

fn candidate_strings(result: &Map<String, Value>, collection: &str) -> Vec<String> {
    result
        .get(collection)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .take(128)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        io::{self, Write},
        os::unix::ffi::OsStringExt,
        path::PathBuf,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use super::{
        Cli, Command, CommandNameV1, LocalEnvelopeClock, LocalFailure, ParseFailureCommandContext,
        RequestIdV1, Rfc3339MillisV1, RunResult, StatusPreflight, WorkspaceTarget,
        contains_check_result_projection_v1, direct_response_result, dynamic_candidates_v2,
        local_generated_at, local_result, local_result_v2, parse_failure_command_context,
        parse_timeout_millis, procedure_validate_v2_diagnostics, render_authoring_diagnostics,
        render_local_failure_with_clock_and_writers, render_result_with_clock_and_writers,
        unsatisfied_check_result_lines_v1, validate_command_shape, validate_direct_flags,
        workspace_removal_confirmation_path,
    };
    use clap::Parser;
    use podway_engine::direct_api::{DirectCommandV1, DirectResponseV1};
    use podway_protocol::{ErrorEnvelopeV2, OutputEnvelopeInputV4, OutputEnvelopeV4};

    fn error_result(command: &Command, error: ErrorEnvelopeV2) -> RunResult {
        direct_response_result(command, DirectResponseV1::Failure(error))
    }

    use serde_json::{Value, json};

    #[test]
    fn delegated_human_errors_render_the_reason_without_changing_other_errors() {
        let mut error: ErrorEnvelopeV2 = serde_json::from_value(json!({
            "schema": "podway.error/v2",
            "request_id": "00000000-0000-4000-8000-000000000002",
            "command": "delegation.package",
            "generated_at": "1970-01-01T00:00:00.012Z",
            "code": "DELEGATION_BINDING_MISMATCH",
            "message": "The acceptance does not match the package.",
            "retryable": false,
            "exit_code": 1,
            "details": {
                "schema": "podway.delegation-error-details/v2",
                "reason": "binding_mismatch"
            }
        }))
        .unwrap();
        let mut stderr = Vec::new();
        super::render_human_error_direct(&error, None, &mut stderr).unwrap();
        assert!(
            String::from_utf8(stderr)
                .unwrap()
                .contains("reason: binding_mismatch\n")
        );

        error = serde_json::from_value(json!({
            "schema": "podway.error/v2",
            "request_id": "00000000-0000-4000-8000-000000000002",
            "command": "delegation.package",
            "generated_at": "1970-01-01T00:00:00.012Z",
            "code": "WORKSPACE_NOT_INITIALIZED",
            "message": "No workspace exists.",
            "retryable": false,
            "exit_code": 5,
            "details": {}
        }))
        .unwrap();
        let mut stderr = Vec::new();
        super::render_human_error_direct(&error, None, &mut stderr).unwrap();
        assert!(!String::from_utf8(stderr).unwrap().contains("reason:"));
    }

    #[test]
    fn v2sdg006_cli_accepts_delegated_status_generation_for_preflight_and_completion() {
        use podway_core::{SessionId, WorkspaceId};
        use podway_protocol::WorkspaceOutputV1;

        let mut result: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v2/protocol/current-contracts.json"
        ))
        .unwrap();
        result = result["fixtures"]["podway.status-result/v5"].take();
        result["schema"] = json!("podway.status-result/v5");
        result["delegation"] = json!({
            "state":"inactive",
            "reason":"no_active_attempt",
            "safe_next_action":"begin_or_inspect"
        });
        result["receipt_summaries"] = json!([]);
        result["receipt_summaries_total"] = json!(0);
        result["receipt_summaries_truncated"] = json!(false);
        let output = OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
            request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000002").unwrap(),
            command: CommandNameV1::new("session.status").unwrap(),
            generated_at: Rfc3339MillisV1::new("1970-01-01T00:00:00.012Z").unwrap(),
            workspace: Some(
                WorkspaceOutputV1::new(
                    WorkspaceId::new("00000000-0000-4000-8000-000000000004").unwrap(),
                    "/tmp/podway-v2sdg006-cli",
                    1,
                )
                .unwrap(),
            ),
            session: None,
            result: result.as_object().unwrap().clone(),
            warnings: Vec::new(),
        })
        .unwrap();
        let preflight = StatusPreflight::from_direct(&output).unwrap();
        assert_eq!(
            preflight.facts.session_id,
            SessionId::new("00000000-0000-4000-8000-000000000001").unwrap()
        );
        let candidates = json!({
            "schema":"podway.status-result/v5",
            "items":[{"item_id":"review"}]
        });
        assert_eq!(
            dynamic_candidates_v2(candidates.as_object().unwrap(), "items"),
            ["review"]
        );
    }

    #[test]
    fn v2ret004_workspace_remove_requires_automation_fences_and_builds_control_payload() {
        let workspace_uuid = "00000000-0000-4000-8000-000000040004";
        let command = Cli::try_parse_from([
            "podway",
            "--json",
            "--yes",
            "--if-workspace-uuid",
            workspace_uuid,
            "workspace",
            "remove",
            "--force",
        ])
        .unwrap();
        assert!(validate_direct_flags(&command).is_ok());
        assert_eq!(command.command.direct_route(), Some("workspace.remove"));
        assert!(command.command.is_destructive());
        let fences = super::ExplicitPreconditions::parse(&command).unwrap();
        assert_eq!(fences.workspace_id.unwrap().as_str(), workspace_uuid);
        assert!(matches!(
            super::direct::typed_command(
                &command,
                &super::ExplicitPreconditions::parse(&command).unwrap()
            )
            .unwrap(),
            DirectCommandV1::WorkspaceRemove {
                confirmed: true,
                force: true
            }
        ));

        for arguments in [
            vec![
                "podway",
                "--json",
                "--if-workspace-uuid",
                workspace_uuid,
                "workspace",
                "remove",
                "--force",
            ],
            vec![
                "podway",
                "--json",
                "--yes",
                "workspace",
                "remove",
                "--force",
            ],
        ] {
            let command = Cli::try_parse_from(arguments).unwrap();
            assert!(validate_direct_flags(&command).is_err());
        }
        assert!(
            super::help_text(&["workspace.remove".to_owned()])
                .unwrap()
                .contains("preserving the Git worktree")
        );
    }

    #[test]
    fn v2ret004_interactive_confirmation_rejects_non_utf8_worktree_paths() {
        let path = PathBuf::from(OsString::from_vec(b"/tmp/podway-\xff".to_vec()));
        let target = WorkspaceTarget {
            path,
            path_bytes: b"/tmp/podway-\xff".to_vec(),
            display: "/tmp/podway-�".to_owned(),
        };
        assert!(workspace_removal_confirmation_path(&target).is_err());
    }

    #[test]
    fn v2ast005_human_rendering_detects_structurally_bound_check_results() {
        use podway_core::WorkspaceId;
        use podway_protocol::WorkspaceOutputV1;

        assert!(contains_check_result_projection_v1(&json!({
            "operation_id": "make-test",
            "operation_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "input_basis": {"digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
            "outcome": "pass",
            "output_digest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        })));
        assert!(contains_check_result_projection_v1(&json!({
            "items": [{"item_id": "verification", "type": "check_result", "value": null}]
        })));
        assert!(!contains_check_result_projection_v1(&json!({
            "type": "artifact",
            "sha256_digest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        })));

        let families: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v2/protocol/current-contracts.json"
        ))
        .unwrap();
        // A running observation: the prepared fixture supplies the closed status shape, the
        // running next fixture supplies metadata-only guidance, and one stored but unsatisfied
        // check result carries the named reason.
        let guidance = families["fixtures"]["podway.next-result/v5"].clone();
        let mut status = families["fixtures"]["podway.observation-result/v5"]["status"].clone();
        status["session"] = json!({
            "id": "00000000-0000-4000-8000-000000000001",
            "lifecycle": "running",
            "revision": 1
        });
        status["current"] = json!({
            "node": guidance["node"],
            "attempt": guidance["attempt"],
            "readiness": guidance["readiness"],
            "missing_required_item_count": guidance["missing_required_item_count"],
            "blockers_total": guidance["blockers_total"]
        });
        status["trace_length"] = json!(1);
        status["counters"] = guidance["counters"].clone();
        status["items"] = json!([{
            "item_id": "verification",
            "type": "check_result",
            "required": true,
            "satisfied": false,
            "revision": 1
        }]);
        status["items_total"] = json!(1);
        let rendered_item = json!({
            "item_id": "verification",
            "type": "check_result",
            "prompt": "Record the verification result.",
            "required": true,
            "required_now": true,
            "satisfied": false,
            "unsatisfied_reason": "operation_digest_mismatch",
            "revision": 1,
            "constraints": {
                "operation_id": "make-test",
                "operation_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "accepted_outcomes": ["pass"],
                "choices_total": 0,
                "choices_truncated": false
            },
            "value": {
                "operation_id": "make-test",
                "outcome": "pass",
                "operation_digest": "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                "input_basis": {"digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
                "output_digest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
            },
            "value_truncated": false
        });
        let result = json!({
            "schema": "podway.observation-result/v5",
            "status": status,
            "guidance": guidance,
            "active_items": [rendered_item.clone()],
            "active_items_total": 1,
            "active_items_truncated": false,
            "mutation_templates": [],
            "mutation_templates_total": 0,
            "mutation_templates_truncated": false
        });
        let output = OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
            request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000002").unwrap(),
            command: CommandNameV1::new("session.observe").unwrap(),
            generated_at: Rfc3339MillisV1::new("1970-01-01T00:00:00.012Z").unwrap(),
            workspace: Some(
                WorkspaceOutputV1::new(
                    WorkspaceId::new("00000000-0000-4000-8000-000000000004").unwrap(),
                    "/tmp/podway-v2ast005-render",
                    1,
                )
                .unwrap(),
            ),
            session: None,
            result: result.as_object().unwrap().clone(),
            warnings: Vec::new(),
        })
        .unwrap();
        let result = RunResult::Direct {
            response: Box::new(DirectResponseV1::Success(output)),
            procedure_check_path: None,
        };
        let clock = FixedClock(UNIX_EPOCH + Duration::from_millis(12));
        let note = "note: check_result is a caller-supplied, structurally bound external check result; Podway does not verify or attest it";
        let reason_line = "unsatisfied check_result verification: operation_digest_mismatch; declared operation_digest sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa; recorded sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

        let mut human = Vec::new();
        let mut human_stderr = Vec::new();
        assert_eq!(
            render_result_with_clock_and_writers(
                &result,
                false,
                false,
                &clock,
                &mut human,
                &mut human_stderr,
            ),
            0
        );
        let human = String::from_utf8(human).unwrap();
        assert_eq!(human.matches(note).count(), 1);
        assert_eq!(human.matches(reason_line).count(), 1);
        assert!(human_stderr.is_empty());

        let mut machine = Vec::new();
        let mut machine_stderr = Vec::new();
        assert_eq!(
            render_result_with_clock_and_writers(
                &result,
                true,
                false,
                &clock,
                &mut machine,
                &mut machine_stderr,
            ),
            0
        );
        let machine = String::from_utf8(machine).unwrap();
        assert!(!machine.contains(note));
        assert!(!machine.contains("unsatisfied check_result"));
        assert!(machine_stderr.is_empty());

        // Every closed reason renders its own declared-versus-recorded comparison, and items
        // without a reason (empty, satisfied, or other types) render nothing.
        let mut outcome_item = rendered_item.clone();
        outcome_item["unsatisfied_reason"] = json!("outcome_not_accepted");
        outcome_item["value"]["outcome"] = json!("inconclusive");
        assert_eq!(
            unsatisfied_check_result_lines_v1(&json!({"active_items": [outcome_item]})),
            vec![
                "unsatisfied check_result verification: outcome_not_accepted; accepted outcomes pass; recorded inconclusive"
                    .to_owned()
            ]
        );
        let mut identity_item = rendered_item.clone();
        identity_item["unsatisfied_reason"] = json!("operation_id_mismatch");
        identity_item["value"]["operation_id"] = json!("other-test");
        assert_eq!(
            unsatisfied_check_result_lines_v1(&json!({"active_items": [identity_item]})),
            vec![
                "unsatisfied check_result verification: operation_id_mismatch; declared operation_id make-test; recorded other-test"
                    .to_owned()
            ]
        );
        let mut silent_item = rendered_item;
        silent_item
            .as_object_mut()
            .unwrap()
            .remove("unsatisfied_reason");
        assert!(
            unsatisfied_check_result_lines_v1(&json!({
                "active_items": [silent_item, {"item_id": "notes", "type": "text", "satisfied": false}]
            }))
            .is_empty()
        );
    }

    #[test]
    fn v2sdg007_delegation_routes_require_closed_arguments() {
        assert!(Cli::try_parse_from(["podway", "delegation", "package"]).is_ok());
        assert!(Cli::try_parse_from(["podway", "delegation", "accept"]).is_err());
        assert!(Cli::try_parse_from(["podway", "delegation", "accept", "--stdin"]).is_ok());
        assert!(Cli::try_parse_from(["podway", "delegation", "receipt"]).is_err());
        assert!(
            Cli::try_parse_from([
                "podway",
                "delegation",
                "receipt",
                "--session",
                "00000000-0000-4000-8000-000000000001",
                "--attempt",
                "00000000-0000-4000-8000-000000000002",
            ])
            .is_ok()
        );
        let arguments = [
            OsString::from("podway"),
            OsString::from("delegation"),
            OsString::from("accept"),
        ];
        assert_eq!(
            parse_failure_command_context(&arguments),
            Some(ParseFailureCommandContext::new("delegation.accept", true))
        );
    }

    #[test]
    fn parser_accepts_canonical_session_start_and_attachment_forms() {
        assert!(matches!(
            Cli::try_parse_from(["podway", "start", "--preset", "sw-dev-v2", "--task", "task"])
                .unwrap()
                .command,
            Command::Start(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["podway", "attach", "artifact", "report.txt"])
                .unwrap()
                .command,
            Command::Attach(_)
        ));
        assert!(
            Cli::try_parse_from([
                "podway",
                "attach",
                "artifact",
                "--reference",
                "build:1",
                "--digest",
                "sha256:abc",
                "--size",
                "1",
                "--media-type",
                "text/plain"
            ])
            .is_ok()
        );
    }

    #[test]
    fn v2lif004_parser_owns_prepared_lifecycle_mutation_forms() {
        assert!(
            Cli::try_parse_from([
                "podway",
                "start",
                "--preset",
                "sw-dev-v2",
                "--task",
                "task",
                "--goal",
                "no longer accepted",
            ])
            .is_err()
        );

        let begin = Cli::try_parse_from([
            "podway",
            "begin",
            "--goal",
            "Ship safely.",
            "--criterion",
            "tests=The focused tests pass.",
            "--actor",
            "agent",
        ])
        .unwrap();
        assert_eq!(begin.command.direct_route(), Some("session.begin"));
        let fences = super::ExplicitPreconditions {
            session_id: Some(
                podway_core::SessionId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            ),
            session_revision: Some(podway_core::Revision::new(0)),
            ..Default::default()
        };
        let DirectCommandV1::Procedure(
            podway_protocol::ProcedureV2MutationCommandV1::SessionBegin(input),
        ) = super::direct::typed_command(&begin, &fences).unwrap()
        else {
            panic!("begin must use its typed request")
        };
        let goal = input.initial_goal.unwrap();
        assert_eq!(goal.goal, "Ship safely.");
        assert_eq!(goal.criteria[0].criterion_id.as_str(), "tests");

        let handed_off = Cli::try_parse_from([
            "podway",
            "disposition",
            "handed-off",
            "--summary",
            "Handoff complete.",
            "--reference",
            "local:handoff",
        ])
        .unwrap();
        assert_eq!(
            handed_off.command.direct_route(),
            Some("session.terminal_disposition")
        );
        let DirectCommandV1::Procedure(
            podway_protocol::ProcedureV2MutationCommandV1::SessionTerminalDisposition(input),
        ) = super::direct::typed_command(&handed_off, &fences).unwrap()
        else {
            panic!("disposition must use its typed request")
        };
        assert!(
            matches!(input.disposition, podway_protocol::TerminalDispositionInputV1::HandedOff { summary, reference } if summary == "Handoff complete." && reference == "local:handoff")
        );
        assert!(
            Cli::try_parse_from([
                "podway",
                "disposition",
                "handed-off",
                "--summary",
                "Handoff complete.",
            ])
            .is_err()
        );

        let not_required = Cli::try_parse_from([
            "podway",
            "disposition",
            "not-required",
            "--reason",
            "No external handoff exists.",
        ])
        .unwrap();
        assert_eq!(
            not_required.command.direct_route(),
            Some("session.terminal_disposition")
        );

        let preserve = Cli::try_parse_from([
            "podway",
            "start",
            "--preset",
            "sw-dev-v2",
            "--task",
            "task",
            "--on-existing",
            "preserve",
            "--supersede-reason",
            "A successor task takes precedence.",
        ])
        .unwrap();
        assert_eq!(
            preserve.command.direct_route(),
            Some("session.start_replace")
        );
        assert!(
            Cli::try_parse_from([
                "podway",
                "start",
                "--preset",
                "sw-dev-v2",
                "--task",
                "task",
                "--replace",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "podway",
                "start",
                "--preset",
                "sw-dev-v2",
                "--task",
                "task",
                "--replace-eligible",
            ])
            .is_err()
        );

        for arguments in [
            vec![
                "podway",
                "--if-attempt",
                "0198b91d-7ea3-7d94-bf78-e18ca8d34970",
                "begin",
            ],
            vec![
                "podway",
                "--if-attempt",
                "0198b91d-7ea3-7d94-bf78-e18ca8d34970",
                "disposition",
                "not-required",
                "--reason",
                "No handoff is required.",
            ],
            vec![
                "podway",
                "--if-attempt",
                "0198b91d-7ea3-7d94-bf78-e18ca8d34970",
                "start",
                "--preset",
                "sw-dev-v2",
                "--task",
                "task",
                "--on-existing",
                "delete",
            ],
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            assert!(validate_direct_flags(&cli).is_err());
        }

        let reset_all_with_summary = Cli::try_parse_from([
            "podway",
            "--yes",
            "reset",
            "--all",
            "--force",
            "--progress-summary",
            "must not be discarded",
        ])
        .unwrap();
        assert!(validate_direct_flags(&reset_all_with_summary).is_err());

        let preserve_without_reason = Cli::try_parse_from([
            "podway",
            "start",
            "--preset",
            "sw-dev-v2",
            "--task",
            "task",
            "--on-existing",
            "preserve",
        ])
        .unwrap();
        assert!(validate_direct_flags(&preserve_without_reason).is_err());

        let force_reset_dry_run = Cli::try_parse_from([
            "podway",
            "reset",
            "--progress-summary",
            "must not be discarded",
            "--dry-run",
        ])
        .unwrap();
        assert!(validate_direct_flags(&force_reset_dry_run).is_err());

        let preserve_with_summary = Cli::try_parse_from([
            "podway",
            "start",
            "--preset",
            "sw-dev-v2",
            "--task",
            "task",
            "--on-existing",
            "preserve",
            "--supersede-reason",
            "A successor takes precedence.",
            "--progress-summary",
            "must not be discarded",
        ])
        .unwrap();
        assert!(validate_direct_flags(&preserve_with_summary).is_err());

        let reset_dry_run_with_confirmation =
            Cli::try_parse_from(["podway", "--yes", "reset", "--dry-run"]).unwrap();
        assert!(validate_direct_flags(&reset_dry_run_with_confirmation).is_err());

        let reset_all_after_interactive_confirmation =
            Cli::try_parse_from(["podway", "reset", "--all", "--force"]).unwrap();
        assert!(validate_direct_flags(&reset_all_after_interactive_confirmation).is_ok());
        assert!(
            reset_all_after_interactive_confirmation
                .command
                .is_destructive()
        );
        assert!(matches!(
            super::direct::typed_command(&reset_all_after_interactive_confirmation, &fences)
                .unwrap(),
            DirectCommandV1::WorkspaceResetAll { confirmed: true }
        ));

        for arguments in [
            vec![
                "podway",
                "disposition",
                "handed-off",
                "--summary",
                " ",
                "--reference",
                "local:handoff",
            ],
            vec![
                "podway",
                "disposition",
                "handed-off",
                "--summary",
                "Completed.",
                "--reference",
                " ",
            ],
            vec!["podway", "disposition", "not-required", "--reason", " "],
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            assert!(validate_command_shape(&cli.command).is_err());
        }

        let over_limit = "x".repeat(4_001);
        let force_reset_over_limit = Cli::try_parse_from(vec![
            "podway".to_owned(),
            "reset".to_owned(),
            "--progress-summary".to_owned(),
            over_limit,
        ])
        .unwrap();
        assert!(validate_command_shape(&force_reset_over_limit.command).is_err());
    }

    #[test]
    fn parser_failure_context_recovers_routes_and_admission_semantics() {
        let context = |arguments: &[&str]| {
            let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
            parse_failure_command_context(&arguments)
        };

        assert_eq!(
            context(&["podway", "start", "--preset", "sw-dev-v2"]),
            Some(ParseFailureCommandContext::new("session.start", true))
        );
        assert_eq!(
            context(&[
                "podway",
                "start",
                "--preset",
                "sw-dev-v2",
                "--on-existing",
                "delete",
            ]),
            Some(ParseFailureCommandContext::new(
                "session.start_replace",
                true
            ))
        );
        assert_eq!(
            context(&["podway", "start", "--preset", "sw-dev-v2", "--dry-run"]),
            Some(ParseFailureCommandContext::new("session.start", false))
        );
        assert_eq!(
            context(&["podway", "reset", "--all", "--unknown"]),
            Some(ParseFailureCommandContext::new("workspace.reset_all", true))
        );
        assert_eq!(
            context(&["podway", "status", "--unknown"]),
            Some(ParseFailureCommandContext::new("session.status", false))
        );
        assert_eq!(
            context(&["podway", "outcome", "lookup", "--unknown"]),
            Some(ParseFailureCommandContext::new("outcome.lookup", false))
        );
        assert_eq!(context(&["podway", "unknown-command"]), None);
    }

    #[test]
    fn timeout_parser_accepts_documented_units_only() {
        assert_eq!(parse_timeout_millis("500ms"), Ok(500));
        assert_eq!(parse_timeout_millis("30s"), Ok(30_000));
        assert_eq!(parse_timeout_millis("2m"), Ok(120_000));
        assert_eq!(parse_timeout_millis("60m"), Ok(3_600_000));
        assert!(parse_timeout_millis("61m").is_err());
        assert!(parse_timeout_millis("30").is_err());
        assert!(parse_timeout_millis("1h").is_err());
    }

    struct FixedClock(SystemTime);

    impl LocalEnvelopeClock for FixedClock {
        fn now(&self) -> SystemTime {
            self.0
        }
    }
    struct BrokenWriter;

    impl Write for BrokenWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken writer"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken writer"))
        }
    }

    #[test]
    fn local_envelopes_use_the_injected_rfc3339_millisecond_clock() {
        let clock = FixedClock(UNIX_EPOCH + Duration::from_millis(12));
        assert_eq!(
            local_generated_at(&clock)
                .expect("fixed clock must render")
                .as_str(),
            "1970-01-01T00:00:00.012Z"
        );
    }

    #[test]
    fn procedure_start_human_error_names_diagnostics_and_the_authoring_check() {
        let response: ErrorEnvelopeV2 = serde_json::from_value(json!({
            "schema": "podway.error/v2",
            "request_id": "00000000-0000-4000-8000-000000000001",
            "command": "session.start",
            "generated_at": "2026-09-22T00:00:00.000Z",
            "code": "PROCEDURE_V2_SCHEMA_INVALID",
            "message": "The Procedure v2 document violates its closed schema.",
            "retryable": false,
            "exit_code": 1,
            "details": {
                "schema": "podway.v2-runtime-error-details/v2",
                "kind": "PROCEDURE_V2_SCHEMA_INVALID",
                "diagnostic_codes": [
                    "GRAPH_NODE_COUNT_OUT_OF_RANGE",
                    "OBSERVATION_STATIC_MEMBERS_BUDGET_EXCEEDED"
                ],
                            }
        }))
        .expect("fixture is a valid Procedure admission error");
        let expected_json = serde_json::to_value(&response).expect("serialize fixture");
        let cli = Cli::try_parse_from([
            "podway",
            "start",
            "--procedure",
            ".podway/procedures/review's workflow.yaml",
            "--task",
            "review",
        ])
        .unwrap();
        let result = error_result(&cli.command, response);
        let clock = FixedClock(UNIX_EPOCH + Duration::from_millis(12));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        assert_eq!(
            render_result_with_clock_and_writers(
                &result,
                false,
                false,
                &clock,
                &mut stdout,
                &mut stderr,
            ),
            1
        );
        assert!(stdout.is_empty());
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            concat!(
                "error: PROCEDURE_V2_SCHEMA_INVALID: The Procedure v2 document violates its closed schema.\n",
                "diagnostics: GRAPH_NODE_COUNT_OUT_OF_RANGE, OBSERVATION_STATIC_MEMBERS_BUDGET_EXCEEDED\n",
                "hint: podway procedure check -- '.podway/procedures/review'\\''s workflow.yaml' for field-level diagnostics and remediation guidance\n",
            )
        );

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            render_result_with_clock_and_writers(
                &result,
                true,
                false,
                &clock,
                &mut stdout,
                &mut stderr,
            ),
            1
        );
        assert!(stderr.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&stdout).expect("JSON output"),
            expected_json,
        );
    }

    #[test]
    fn procedure_start_human_error_preserves_leading_dash_paths_and_custom_scope() {
        fn response(code: &str) -> ErrorEnvelopeV2 {
            let details = if code == "PROCEDURE_V2_SCHEMA_INVALID" {
                json!({
                "schema": "podway.v2-runtime-error-details/v2",
                "kind": code,
                "diagnostic_codes": ["GRAPH_NODE_COUNT_OUT_OF_RANGE"],
                                })
            } else {
                json!({
                "schema": "podway.v2-runtime-error-details/v2",
                "kind": code,
                                })
            };
            serde_json::from_value(json!({
                "schema": "podway.error/v2",
                "request_id": "00000000-0000-4000-8000-000000000001",
                "command": "session.start",
                "generated_at": "2026-09-22T00:00:00.000Z",
                "code": code,
                "message": "Admission failed.",
                "retryable": false,
                "exit_code": 1,
                "details": details
            }))
            .unwrap()
        }

        let custom = Cli::try_parse_from([
            "podway",
            "start",
            "--procedure=--note.yaml",
            "--task",
            "review",
        ])
        .unwrap();
        let result = error_result(&custom.command, response("PROCEDURE_V2_SCHEMA_INVALID"));
        let clock = FixedClock(UNIX_EPOCH + Duration::from_millis(12));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            render_result_with_clock_and_writers(
                &result,
                false,
                false,
                &clock,
                &mut stdout,
                &mut stderr,
            ),
            1
        );
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            concat!(
                "error: PROCEDURE_V2_SCHEMA_INVALID: Admission failed.\n",
                "diagnostics: GRAPH_NODE_COUNT_OUT_OF_RANGE\n",
                "hint: podway procedure check -- --note.yaml for field-level diagnostics and remediation guidance\n",
            )
        );

        let preset = Cli::try_parse_from([
            "podway",
            "start",
            "--preset",
            "sw-dev-v2",
            "--task",
            "review",
        ])
        .unwrap();
        let result = error_result(&preset.command, response("PROCEDURE_V2_SCHEMA_INVALID"));
        let mut stderr = Vec::new();
        assert_eq!(
            render_result_with_clock_and_writers(
                &result,
                false,
                false,
                &clock,
                &mut Vec::new(),
                &mut stderr,
            ),
            1
        );
        let rendered = String::from_utf8(stderr).unwrap();
        assert!(rendered.contains("diagnostics: GRAPH_NODE_COUNT_OUT_OF_RANGE\n"));
        assert!(!rendered.contains("procedure check"));

        let result = error_result(&custom.command, response("GOAL_TRACKING_NOT_ENABLED"));
        let mut stderr = Vec::new();
        assert_eq!(
            render_result_with_clock_and_writers(
                &result,
                false,
                false,
                &clock,
                &mut Vec::new(),
                &mut stderr,
            ),
            1
        );
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "error: GOAL_TRACKING_NOT_ENABLED: Admission failed.\n"
        );
    }

    #[test]
    fn human_authoring_output_discloses_diagnostic_truncation() {
        use podway_core::{AuthoringDiagnostic, AuthoringDiagnosticCode, SourceLocation};

        let diagnostic = AuthoringDiagnostic::new(
            AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            "workflow.yaml",
            SourceLocation::document_start(),
            "$",
            "Invalid schema.",
            "Correct the schema.",
        );
        let diagnostics = vec![diagnostic.clone(); 256];
        let rendered = render_authoring_diagnostics(&diagnostics, true, 300);

        assert!(rendered.ends_with("diagnostics truncated: showing 256 of 300\n"));
        assert_eq!(rendered.matches("AUTHORING_SCHEMA_INVALID").count(), 256);

        let complete = render_authoring_diagnostics(&diagnostics[..1], false, 1);
        assert!(!complete.contains("diagnostics truncated:"));

        let result =
            procedure_validate_v2_diagnostics("workflow.yaml", None, vec![diagnostic; 300]);
        let clock = FixedClock(UNIX_EPOCH + Duration::from_millis(12));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            render_result_with_clock_and_writers(
                &result,
                true,
                false,
                &clock,
                &mut stdout,
                &mut stderr,
            ),
            1
        );
        assert!(stderr.is_empty());
        let output: Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(output["result"]["diagnostics_truncated"], true);
        assert_eq!(output["result"]["diagnostics_total"], 300);
        assert!(
            !String::from_utf8(stdout)
                .unwrap()
                .contains("diagnostics truncated: showing")
        );
    }

    #[test]
    fn oversized_output_returns_a_valid_error_without_partial_success() {
        let clock = FixedClock(UNIX_EPOCH + Duration::from_millis(12));
        for (length, exit) in [
            (65_537, 0),
            (podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1, 6),
        ] {
            let Value::Object(source) = json!({
                "schema": "podway.procedure-source-result/v1",
                "operation": "scaffold",
                "target_schema": "podway.procedure/v2",
                "target_digest": format!("sha256:{}", "0".repeat(64)),
                "document": "x".repeat(length),
                "template": "basic",
            }) else {
                unreachable!();
            };
            for (result, field) in [
                (
                    local_result(
                        "procedure.show",
                        json!({"canonical_json": "x".repeat(length)}),
                        String::new(),
                    ),
                    "canonical_json",
                ),
                (
                    local_result_v2("procedure.scaffold", source, String::new(), 0),
                    "document",
                ),
            ] {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                assert_eq!(
                    render_result_with_clock_and_writers(
                        &result,
                        true,
                        false,
                        &clock,
                        &mut stdout,
                        &mut stderr,
                    ),
                    exit,
                );
                assert!(stderr.is_empty());
                if exit == 0 {
                    let output: OutputEnvelopeV4 = serde_json::from_slice(&stdout).unwrap();
                    assert_eq!(
                        output.as_value()["result"][field].as_str().unwrap().len(),
                        length
                    );
                } else {
                    let error: podway_protocol::ErrorEnvelopeV2 =
                        serde_json::from_slice(&stdout).unwrap();
                    assert_eq!(error.as_value()["code"], "INTERNAL_ERROR");
                    assert_eq!(
                        error.as_value()["details"]["schema"],
                        "podway.internal-error-details/v2"
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_clock_returns_bounded_stderr_without_partial_json() {
        let clock = FixedClock(UNIX_EPOCH - Duration::from_secs(1));
        let digest = podway_core::Sha256Digest::new(format!("sha256:{}", "0".repeat(64))).unwrap();
        for result in [
            local_result("help", json!({"text": "help"}), "help".to_owned()),
            procedure_validate_v2_diagnostics("workflow.yaml", Some(&digest), Vec::new()),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            assert_eq!(
                render_result_with_clock_and_writers(
                    &result,
                    true,
                    false,
                    &clock,
                    &mut stdout,
                    &mut stderr,
                ),
                6
            );
            assert!(stdout.is_empty());
            assert!(stderr.starts_with(b"error: "));
            assert!(stderr.len() < 512);
        }
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            render_local_failure_with_clock_and_writers(
                LocalFailure::internal("clock failure"),
                true,
                &clock,
                &mut stdout,
                &mut stderr,
            ),
            6
        );
        assert!(stdout.is_empty());
        assert!(stderr.starts_with(b"error: "));
        assert!(stderr.len() < 512);
    }

    #[test]
    fn invalid_local_error_uses_a_valid_internal_fallback() {
        let clock = FixedClock(UNIX_EPOCH + Duration::from_millis(12));
        let mut failure = LocalFailure::internal("invalid details");
        failure.details.clear();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            render_local_failure_with_clock_and_writers(
                failure,
                true,
                &clock,
                &mut stdout,
                &mut stderr
            ),
            6
        );
        let error: podway_protocol::ErrorEnvelopeV2 = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(error.as_value()["code"], "INTERNAL_ERROR");
        assert!(stderr.is_empty());
    }

    #[test]
    fn broken_writers_return_the_client_exit_code_for_text_and_json() {
        let clock = FixedClock(UNIX_EPOCH + Duration::from_millis(12));
        let result = local_result("help", json!({ "text": "help" }), "help".to_owned());

        for json_output in [false, true] {
            let mut broken_stdout = BrokenWriter;
            let mut stderr = Vec::new();
            assert_eq!(
                render_result_with_clock_and_writers(
                    &result,
                    json_output,
                    false,
                    &clock,
                    &mut broken_stdout,
                    &mut stderr,
                ),
                6,
            );

            if json_output {
                let mut broken_stdout = BrokenWriter;
                let mut stderr = Vec::new();
                assert_eq!(
                    render_local_failure_with_clock_and_writers(
                        LocalFailure::request_invalid("invalid"),
                        true,
                        &clock,
                        &mut broken_stdout,
                        &mut stderr,
                    ),
                    6,
                );
            } else {
                let mut stdout = Vec::new();
                let mut broken_stderr = BrokenWriter;
                assert_eq!(
                    render_local_failure_with_clock_and_writers(
                        LocalFailure::request_invalid("invalid"),
                        false,
                        &clock,
                        &mut stdout,
                        &mut broken_stderr,
                    ),
                    6,
                );
            }
        }
    }
}
