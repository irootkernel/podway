//! Public command-line surface for the Podway v1 command contract.

mod completion;

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, IsTerminal, Read, Write},
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use clap::{ArgAction, Args, Parser, Subcommand};
use nix::{
    errno::Errno,
    fcntl::{OFlag, open, openat},
    sys::stat::Mode,
    unistd::geteuid,
};
use podway_cli::client::{
    DEFAULT_DAEMON_CONNECT_TIMEOUT_V1, DEFAULT_DAEMON_WRITE_TIMEOUT_V1, DaemonClientErrorV1,
    DaemonClientTimeoutsV1, DaemonClientV1,
};
use podway_config::{
    ConfigError, MAX_PROCEDURE_DOCUMENT_BYTES_V1, ProcedureFormatV1, ProcedureWarningPolicyV1,
    ProcedureWarningV1, parse_procedure_v1,
};
use podway_core::{AttemptId, Revision, SessionId, Sha256Digest, WorkspaceId};
use podway_presets::{PresetError, catalog_v1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, JobOutputV1, JobStateV1,
    MAX_SLICE_ITEM_TEXT_SCALARS_V1, MAX_WAIT_TIMEOUT_MILLIS_V1, NextResultV1, OperationV1,
    PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
    ResponseEnvelopeV1, Rfc3339MillisV1, StatusResultV1, WorkspaceContextV1,
    WorktreeSelectorWireV1,
};
use podway_service::ServiceRuntimePathsV1;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use completion::Shell;

const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;
const LOCAL_USAGE_EXIT: i32 = 2;
const LOCAL_DAEMON_EXIT: i32 = 3;
const LOCAL_CLIENT_EXIT: i32 = 6;

#[derive(Debug, Parser)]
#[command(
    name = "podway",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    subcommand_required = true,
    arg_required_else_help = false
)]
struct Cli {
    /// Emit exactly one versioned JSON object to stdout.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    json: bool,

    /// Target an explicit Git worktree.
    #[arg(long, global = true, value_name = "PATH")]
    worktree: Option<PathBuf>,

    /// Bound daemon connection or daemon-side waiting.
    #[arg(long, global = true, value_name = "DURATION", value_parser = parse_timeout_millis)]
    timeout: Option<u64>,

    /// Disable text color. Podway v1 currently emits uncolored text.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    no_color: bool,

    /// Suppress nonessential text output.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    quiet: bool,

    /// Reuse this idempotency key instead of generating a UUID-v4 key.
    #[arg(long, global = true, value_name = "KEY")]
    idempotency_key: Option<String>,

    /// Return after durable mutation admission.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    detach: bool,

    /// Require an exact session revision.
    #[arg(long, global = true, value_name = "N")]
    if_session_revision: Option<u64>,

    /// Require an exact active attempt identifier.
    #[arg(long, global = true, value_name = "UUID")]
    if_attempt: Option<String>,

    /// Require an exact active item revision.
    #[arg(long, global = true, value_name = "N")]
    if_item_revision: Option<u64>,

    /// Approve a destructive operation without prompting.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    yes: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show offline help and examples.
    Help {
        #[arg(value_name = "TOPIC")]
        topic: Option<String>,
    },
    /// Print the CLI version.
    Version,
    /// Emit an installable completion script.
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Validate or display a procedure without a daemon.
    Procedure {
        #[command(subcommand)]
        command: ProcedureCommand,
    },
    /// Inspect built-in presets without a daemon.
    Preset {
        #[command(subcommand)]
        command: PresetCommand,
    },
    /// Service lifecycle grammar. Effects are intentionally deferred to G007.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Init {
        #[arg(long, action = ArgAction::SetTrue)]
        repair: bool,
    },
    Doctor {
        #[arg(long, action = ArgAction::SetTrue)]
        deep: bool,
    },
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    Start(StartArgs),
    Status(ReadArgs),
    Next(ReadArgs),
    Complete,
    Skip {
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
    },
    Retry {
        #[arg(long, value_name = "TEXT")]
        reason: String,
    },
    Return(StageMutationArgs),
    Block {
        #[arg(long, value_name = "TEXT")]
        reason: String,
    },
    Unblock {
        #[arg(
            value_name = "BLOCKER_ID",
            required_unless_present = "all",
            conflicts_with = "all"
        )]
        blocker_id: Option<String>,
        #[arg(long, action = ArgAction::SetTrue)]
        all: bool,
    },
    Cancel {
        #[arg(long, value_name = "TEXT")]
        reason: String,
    },
    Reopen(StageMutationArgs),
    Reset(ResetArgs),
    Check {
        #[arg(value_name = "ITEM_ID")]
        item_id: String,
    },
    Uncheck {
        #[arg(value_name = "ITEM_ID")]
        item_id: String,
    },
    Set(SetArgs),
    Add {
        #[arg(value_name = "ITEM_ID")]
        item_id: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },
    Remove {
        #[arg(value_name = "ITEM_ID")]
        item_id: String,
        #[arg(value_name = "VALUE")]
        value: String,
        #[arg(long, action = ArgAction::SetTrue)]
        ignore_missing: bool,
    },
    Attach(AttachArgs),
    Clear {
        #[arg(value_name = "ITEM_ID")]
        item_id: String,
    },
    Job {
        #[command(subcommand)]
        command: JobCommand,
    },
    /// Internal best-effort candidates for generated shell completion scripts.
    #[command(name = "__complete", hide = true)]
    CompleteDynamic {
        #[arg(default_value = "items")]
        kind: String,
    },
}

#[derive(Debug, Args)]
struct StartArgs {
    #[arg(
        long,
        value_name = "PRESET",
        conflicts_with = "procedure",
        required_unless_present = "procedure"
    )]
    preset: Option<String>,
    #[arg(
        long,
        value_name = "FILE",
        conflicts_with = "preset",
        required_unless_present = "preset"
    )]
    procedure: Option<String>,
    #[arg(long, value_name = "TITLE")]
    task: String,
    #[arg(long, action = ArgAction::SetTrue)]
    replace: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    dry_run: bool,
}

#[derive(Debug, Args, Default)]
struct ReadArgs {
    #[arg(long, action = ArgAction::SetTrue)]
    verbose: bool,
    #[arg(long, action = ArgAction::SetTrue, conflicts_with = "after_job")]
    wait_for_idle: bool,
    #[arg(long, value_name = "JOB_ID", conflicts_with = "wait_for_idle")]
    after_job: Option<String>,
}

#[derive(Debug, Args)]
struct StageMutationArgs {
    #[arg(long, value_name = "STAGE_ID")]
    to: String,
    #[arg(long, value_name = "TEXT")]
    reason: String,
    #[arg(long, action = ArgAction::SetTrue)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct ResetArgs {
    #[arg(long, action = ArgAction::SetTrue)]
    all: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    force: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct SetArgs {
    #[arg(value_name = "ITEM_ID")]
    item_id: String,
    #[arg(
        value_name = "VALUE",
        required_unless_present = "stdin",
        conflicts_with = "stdin"
    )]
    value: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    stdin: bool,
}

#[derive(Debug, Args)]
struct AttachArgs {
    #[arg(value_name = "ITEM_ID")]
    item_id: String,
    #[arg(
        value_name = "PATH",
        required_unless_present = "reference",
        conflicts_with = "reference"
    )]
    path: Option<String>,
    #[arg(long, value_name = "REFERENCE", conflicts_with = "path", requires_all = ["digest", "size", "media_type"])]
    reference: Option<String>,
    #[arg(long, value_name = "SHA256", requires = "reference")]
    digest: Option<String>,
    #[arg(long, value_name = "BYTES", requires = "reference")]
    size: Option<u64>,
    #[arg(long, value_name = "TYPE")]
    media_type: Option<String>,
}

#[derive(Debug, Subcommand)]
enum ProcedureCommand {
    Validate {
        #[arg(value_name = "FILE")]
        file: PathBuf,
        #[arg(long, action = ArgAction::SetTrue)]
        warnings_as_errors: bool,
    },
    Show {
        #[arg(value_name = "FILE")]
        file: PathBuf,
        #[arg(long, action = ArgAction::SetTrue)]
        canonical: bool,
    },
}

#[derive(Debug, Subcommand)]
enum PresetCommand {
    List,
    Show {
        #[arg(value_name = "NAME")]
        name: String,
    },
    Explain {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Install {
        #[arg(long, value_name = "PATH")]
        daemon_path: Option<PathBuf>,
    },
    Uninstall {
        #[arg(long, action = ArgAction::SetTrue)]
        purge_logs: bool,
    },
    Start,
    Stop,
    Restart,
    Status,
    Logs {
        #[arg(long, action = ArgAction::SetTrue)]
        follow: bool,
        #[arg(long, value_name = "N")]
        lines: Option<u32>,
    },
}

#[derive(Debug, Subcommand)]
enum WorkspaceCommand {
    Show,
    Repair,
}

#[derive(Debug, Subcommand)]
enum JobCommand {
    List {
        #[arg(long, value_parser = ["queued", "running", "succeeded", "failed", "cancelled"])]
        state: Option<String>,
    },
    Status {
        #[arg(value_name = "JOB_ID")]
        job_id: String,
    },
    Wait {
        #[arg(value_name = "JOB_ID")]
        job_id: String,
    },
    Cancel {
        #[arg(value_name = "JOB_ID")]
        job_id: String,
    },
}

impl Command {
    const fn daemon_wire_name(&self) -> Option<&'static str> {
        match self {
            Self::Init { .. } => Some("workspace.init"),
            Self::Doctor { .. } => Some("workspace.doctor"),
            Self::Workspace {
                command: WorkspaceCommand::Show,
            } => Some("workspace.show"),
            Self::Workspace {
                command: WorkspaceCommand::Repair,
            } => Some("workspace.repair"),
            Self::Start(args) if args.replace => Some("session.start_replace"),
            Self::Start(_) => Some("session.start"),
            Self::Status(_) | Self::CompleteDynamic { .. } => Some("session.status"),
            Self::Next(_) => Some("session.next"),
            Self::Complete => Some("session.complete"),
            Self::Skip { .. } => Some("session.skip"),
            Self::Retry { .. } => Some("session.retry"),
            Self::Return(_) => Some("session.return"),
            Self::Block { .. } => Some("session.block"),
            Self::Unblock { .. } => Some("session.unblock"),
            Self::Cancel { .. } => Some("session.cancel"),
            Self::Reopen(_) => Some("session.reopen"),
            Self::Reset(args) if args.all => Some("workspace.reset_all"),
            Self::Reset(_) => Some("session.reset"),
            Self::Check { .. } => Some("item.check"),
            Self::Uncheck { .. } => Some("item.uncheck"),
            Self::Set(_) => Some("item.set"),
            Self::Add { .. } => Some("item.add"),
            Self::Remove { .. } => Some("item.remove"),
            Self::Attach(_) => Some("item.attach"),
            Self::Clear { .. } => Some("item.clear"),
            Self::Job {
                command: JobCommand::List { .. },
            } => Some("job.list"),
            Self::Job {
                command: JobCommand::Status { .. },
            } => Some("job.status"),
            Self::Job {
                command: JobCommand::Wait { .. },
            } => Some("job.wait"),
            Self::Job {
                command: JobCommand::Cancel { .. },
            } => Some("job.cancel"),
            Self::Help { .. }
            | Self::Version
            | Self::Completions { .. }
            | Self::Procedure { .. }
            | Self::Preset { .. }
            | Self::Daemon { .. } => None,
        }
    }

    fn canonical_route(&self) -> &'static str {
        match self {
            Self::Help { .. } => "help",
            Self::Version => "version",
            Self::Completions { .. } => "completions",
            Self::Procedure {
                command: ProcedureCommand::Validate { .. },
            } => "procedure.validate",
            Self::Procedure {
                command: ProcedureCommand::Show { .. },
            } => "procedure.show",
            Self::Preset {
                command: PresetCommand::List,
            } => "preset.list",
            Self::Preset {
                command: PresetCommand::Show { .. },
            } => "preset.show",
            Self::Preset {
                command: PresetCommand::Explain { .. },
            } => "preset.explain",
            Self::Daemon { command } => daemon_command_name(command),
            Self::CompleteDynamic { .. } => "__complete",
            command => command
                .daemon_wire_name()
                .expect("non-local commands have a canonical wire route"),
        }
    }

    const fn is_mutation(&self) -> bool {
        match self {
            Self::Init { .. }
            | Self::Complete
            | Self::Skip { .. }
            | Self::Retry { .. }
            | Self::Block { .. }
            | Self::Unblock { .. }
            | Self::Cancel { .. }
            | Self::Check { .. }
            | Self::Uncheck { .. }
            | Self::Set(_)
            | Self::Add { .. }
            | Self::Remove { .. }
            | Self::Attach(_)
            | Self::Clear { .. } => true,
            Self::Start(args) => !args.dry_run,
            Self::Return(args) | Self::Reopen(args) => !args.dry_run,
            Self::Reset(args) => !args.dry_run,
            _ => false,
        }
    }

    const fn needs_preflight(&self) -> bool {
        match self {
            Self::Complete
            | Self::Skip { .. }
            | Self::Retry { .. }
            | Self::Return(_)
            | Self::Block { .. }
            | Self::Unblock { .. }
            | Self::Cancel { .. }
            | Self::Reopen(_)
            | Self::Check { .. }
            | Self::Uncheck { .. }
            | Self::Set(_)
            | Self::Add { .. }
            | Self::Remove { .. }
            | Self::Attach(_)
            | Self::Clear { .. } => true,
            Self::Start(args) => args.replace,
            Self::Reset(args) => !args.all,
            _ => false,
        }
    }

    const fn is_item_mutation(&self) -> bool {
        matches!(
            self,
            Self::Check { .. }
                | Self::Uncheck { .. }
                | Self::Set(_)
                | Self::Add { .. }
                | Self::Remove { .. }
                | Self::Attach(_)
                | Self::Clear { .. }
        )
    }

    const fn is_destructive(&self) -> bool {
        match self {
            Self::Start(args) => args.replace && !args.dry_run,
            Self::Reset(args) => !args.dry_run,
            _ => false,
        }
    }

    const fn is_dry_run(&self) -> bool {
        matches!(
            self,
            Self::Start(StartArgs { dry_run: true, .. })
                | Self::Return(StageMutationArgs { dry_run: true, .. })
                | Self::Reopen(StageMutationArgs { dry_run: true, .. })
                | Self::Reset(ResetArgs { dry_run: true, .. })
        )
    }
}

#[derive(Clone, Debug)]
struct WorkspaceTarget {
    path: PathBuf,
    path_bytes: Vec<u8>,
    display: String,
}

impl WorkspaceTarget {
    fn selector(
        &self,
        expected_uuid: Option<WorkspaceId>,
    ) -> Result<WorktreeSelectorWireV1, LocalFailure> {
        WorktreeSelectorWireV1::new(&self.path_bytes, self.display.clone(), expected_uuid)
            .map_err(|_| LocalFailure::request_invalid("workspace selector is invalid"))
    }

    fn context(
        &self,
        expected_uuid: Option<WorkspaceId>,
    ) -> Result<WorkspaceContextV1, LocalFailure> {
        WorkspaceContextV1::new(self.display.clone(), expected_uuid)
            .map_err(|_| LocalFailure::request_invalid("worktree path is invalid"))
    }
}

#[derive(Clone, Debug)]
struct StatusFacts {
    session_id: SessionId,
    session_revision: Revision,
    attempt_id: Option<AttemptId>,
    item_revisions: Vec<(String, Revision)>,
}

#[derive(Clone, Debug)]
struct StatusPreflight {
    transport_workspace_id: WorkspaceId,
    facts: StatusFacts,
}

impl StatusPreflight {
    fn from_output(output: &podway_protocol::OutputEnvelopeV1) -> Result<Self, LocalFailure> {
        let status = StatusResultV1::from_result_map(output.result())
            .map_err(|_| typed_result_failure(output))?;
        let transport_workspace_id = output
            .workspace()
            .map(|workspace| workspace.uuid().clone())
            .ok_or_else(|| {
                LocalFailure::response_invalid("status response omitted workspace identity")
            })?;
        Ok(Self {
            transport_workspace_id,
            facts: StatusFacts::from_status(&status),
        })
    }
}

impl StatusFacts {
    fn from_status(status: &StatusResultV1) -> Self {
        Self {
            session_id: status.session.id.clone(),
            session_revision: status.session.revision,
            attempt_id: status
                .current
                .as_ref()
                .map(|current| current.attempt_id.clone()),
            item_revisions: status
                .items
                .iter()
                .map(|item| (item.id.as_str().to_owned(), item.revision))
                .collect(),
        }
    }

    fn preconditions(
        &self,
        command: &Command,
        explicit: &ExplicitPreconditions,
    ) -> Result<PreconditionsV1, LocalFailure> {
        let attempt_id = explicit
            .attempt_id
            .clone()
            .or_else(|| self.attempt_id.clone());
        if command.is_item_mutation() {
            let item_id = item_id(command).ok_or_else(|| {
                LocalFailure::request_invalid("item command omitted an item identifier")
            })?;
            let item_revision = explicit.item_revision.or_else(|| {
                self.item_revisions
                    .iter()
                    .find_map(|(id, revision)| (id == item_id).then_some(*revision))
            });
            return PreconditionsV1::new(
                None,
                None,
                Some(attempt_id.ok_or_else(|| {
                    LocalFailure::response_invalid("status response omitted the active attempt")
                })?),
                Some(item_revision.ok_or_else(|| {
                    LocalFailure::response_invalid("status response omitted the requested item")
                })?),
                None,
                None,
            )
            .map_err(|_| LocalFailure::response_invalid("item preconditions are invalid"));
        }

        let session_revision = explicit.session_revision.unwrap_or(self.session_revision);
        if matches!(command, Command::Start(StartArgs { replace: true, .. })) {
            return PreconditionsV1::new(
                Some(self.session_id.clone()),
                Some(session_revision),
                None,
                None,
                None,
                None,
            )
            .map_err(|_| {
                LocalFailure::response_invalid("start-replace preconditions are invalid")
            });
        }
        if matches!(command, Command::Reset(_)) {
            return PreconditionsV1::new(
                Some(self.session_id.clone()),
                Some(session_revision),
                None,
                None,
                None,
                None,
            )
            .map_err(|_| LocalFailure::response_invalid("reset preconditions are invalid"));
        }
        if matches!(command, Command::Reopen(_)) {
            return PreconditionsV1::new(None, Some(session_revision), None, None, None, None)
                .map_err(|_| LocalFailure::response_invalid("reopen preconditions are invalid"));
        }
        PreconditionsV1::new(
            None,
            Some(session_revision),
            Some(attempt_id.ok_or_else(|| {
                LocalFailure::response_invalid("status response omitted the active attempt")
            })?),
            None,
            None,
            None,
        )
        .map_err(|_| LocalFailure::response_invalid("session preconditions are invalid"))
    }
}

#[derive(Clone, Debug, Default)]
struct ExplicitPreconditions {
    session_revision: Option<Revision>,
    attempt_id: Option<AttemptId>,
    item_revision: Option<Revision>,
}

impl ExplicitPreconditions {
    fn parse(cli: &Cli) -> Result<Self, LocalFailure> {
        let attempt_id = cli
            .if_attempt
            .as_deref()
            .map(|id| {
                AttemptId::new(id.to_owned())
                    .map_err(|_| LocalFailure::request_invalid("invalid attempt identifier"))
            })
            .transpose()?;
        Ok(Self {
            session_revision: cli.if_session_revision.map(Revision::new),
            attempt_id,
            item_revision: cli.if_item_revision.map(Revision::new),
        })
    }

    const fn any(&self) -> bool {
        self.session_revision.is_some() || self.attempt_id.is_some() || self.item_revision.is_some()
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
}

impl LocalFailure {
    fn catalog(code: &'static str, message: impl Into<String>, command: impl Into<String>) -> Self {
        let (exit_code, retryable) = match code {
            "REQUEST_INVALID" | "REQUEST_TOO_LARGE" | "CONFIRMATION_REQUIRED" => {
                (LOCAL_USAGE_EXIT, false)
            }
            "DAEMON_NOT_INSTALLED" => (LOCAL_DAEMON_EXIT, false),
            "DAEMON_UNAVAILABLE" => (LOCAL_DAEMON_EXIT, true),
            "PRESET_NOT_FOUND"
            | "PROCEDURE_NOT_FOUND"
            | "PROCEDURE_INVALID"
            | "PROCEDURE_SCHEMA_UNSUPPORTED" => (1, false),
            "PATH_OUTSIDE_WORKTREE" => (5, false),
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
        }
    }

    fn request_invalid(message: impl Into<String>) -> Self {
        Self::catalog("REQUEST_INVALID", message, "cli")
    }

    fn confirmation_required(command: &str) -> Self {
        Self::catalog(
            "CONFIRMATION_REQUIRED",
            "explicit confirmation is required",
            command,
        )
    }

    fn daemon_unavailable(command: &str) -> Self {
        Self::catalog(
            "DAEMON_UNAVAILABLE",
            "the local daemon is unavailable",
            command,
        )
    }

    fn service_unavailable(command: &str) -> Self {
        Self::catalog(
            "DAEMON_NOT_INSTALLED",
            "daemon lifecycle management is unavailable until the service adapter is installed",
            command,
        )
    }

    fn response_invalid(message: impl Into<String>) -> Self {
        Self::catalog("INTERNAL_ERROR", message, "cli")
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

    fn with_command(mut self, command: &str) -> Self {
        self.command = command.to_owned();
        self
    }

    fn with_correlation(mut self, command: &str, request_id: &str) -> Self {
        self.command = command.to_owned();
        self.request_id = Some(request_id.to_owned());
        self
    }
}

enum RunResult {
    Response(Box<ResponseEnvelopeV1>),
    Local {
        command: String,
        result: Map<String, Value>,
        text: String,
    },
}

/// Runs the CLI and returns its process exit code.
pub fn run() -> i32 {
    let arguments: Vec<OsString> = env::args_os().collect();
    let json_requested = arguments
        .iter()
        .any(|argument| argument.as_os_str() == OsStr::new("--json"));
    match Cli::try_parse_from(&arguments) {
        Ok(cli) => {
            let route = cli.command.canonical_route();
            let json_output = cli.json;
            let quiet = cli.quiet;
            match execute(cli) {
                Ok(result) => render_result(&result, json_output, quiet),
                Err(failure) => render_local_failure(failure.with_command(route), json_output),
            }
        }
        Err(_) => render_local_failure(
            LocalFailure::request_invalid("invalid command syntax"),
            json_requested,
        ),
    }
}

fn execute(mut cli: Cli) -> Result<RunResult, LocalFailure> {
    if let Command::CompleteDynamic { kind } = &cli.command {
        let kind = kind.clone();
        return dynamic_completion(cli.worktree.take(), &kind);
    }
    if let Some(local) = execute_local(&cli)? {
        return Ok(local);
    }
    let wire_name = cli
        .command
        .daemon_wire_name()
        .ok_or_else(|| LocalFailure::request_invalid("unsupported command"))?;
    validate_daemon_flags(&cli).map_err(|failure| failure.with_command(wire_name))?;
    validate_command_shape(&cli.command).map_err(|failure| failure.with_command(wire_name))?;
    prepare_stdin_payload(&mut cli.command)?;
    if matches!(
        &cli.command,
        Command::Start(StartArgs {
            dry_run: true,
            replace: false,
            ..
        })
    ) {
        return execute_start_dry_run(&cli);
    }
    confirm_if_required(&cli, wire_name)?;

    let target = workspace_target(cli.worktree.take())?;
    let wait_timeout_ms = cli.timeout.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
    let explicit = ExplicitPreconditions::parse(&cli)?;
    let client =
        daemon_client(wait_timeout_ms).map_err(|failure| failure.with_command(wire_name))?;

    let reset_all_workspace_id =
        if matches!(&cli.command, Command::Reset(ResetArgs { all: true, .. })) {
            let status_request = build_request(
                "session.status",
                &target,
                RequestSpec::query(wait_timeout_ms, Map::new(), None),
            )?;
            match request_daemon(&client, &status_request)
                .map_err(|failure| failure.with_command(wire_name))?
            {
                ResponseEnvelopeV1::Output(status) => Some(
                    StatusPreflight::from_output(&status)
                        .map_err(|failure| {
                            failure.with_correlation(wire_name, status.request_id().as_str())
                        })?
                        .transport_workspace_id,
                ),
                ResponseEnvelopeV1::Error(error) if reset_probe_can_recover(&error) => None,
                ResponseEnvelopeV1::Error(error) => {
                    return re_correlate_preflight_error(&error, wire_name);
                }
            }
        } else {
            None
        };

    if cli.command.needs_preflight() {
        let status_request = build_request(
            "session.status",
            &target,
            RequestSpec::query(wait_timeout_ms, Map::new(), None),
        )?;
        let status_response = request_daemon(&client, &status_request)
            .map_err(|failure| failure.with_command(wire_name))?;
        let status = match status_response {
            ResponseEnvelopeV1::Output(status) => status,
            ResponseEnvelopeV1::Error(error) => {
                return re_correlate_preflight_error(&error, wire_name);
            }
        };
        let preflight = StatusPreflight::from_output(&status)
            .map_err(|failure| failure.with_correlation(wire_name, status.request_id().as_str()))?;
        let (operation, payload) = daemon_payload(&mut cli.command)?;
        let preconditions = preflight
            .facts
            .preconditions(&cli.command, &explicit)
            .map_err(|failure| failure.with_correlation(wire_name, status.request_id().as_str()))?;
        let expected_workspace_id = preflight.transport_workspace_id;
        let request = build_request(
            wire_name,
            &target,
            RequestSpec {
                operation,
                expected_uuid: Some(expected_workspace_id),
                idempotency_key: requires_idempotency_key(operation)
                    .then(|| mutation_key(cli.idempotency_key))
                    .transpose()?,
                preconditions,
                detach: cli.detach,
                wait_timeout_ms,
                payload,
            },
        )?;
        return request_daemon(&client, &request)
            .map(|response| RunResult::Response(Box::new(response)));
    }

    let (operation, mut payload) = daemon_payload(&mut cli.command)?;
    if let Some(workspace_id) = &reset_all_workspace_id {
        payload.insert(
            "expected_workspace_uuid".to_owned(),
            Value::String(workspace_id.as_str().to_owned()),
        );
    }
    let request = build_request(
        wire_name,
        &target,
        RequestSpec {
            operation,
            expected_uuid: reset_all_workspace_id,
            idempotency_key: requires_idempotency_key(operation)
                .then(|| mutation_key(cli.idempotency_key))
                .transpose()?,
            preconditions: control_preconditions(&cli.command)?,
            detach: cli.detach,
            wait_timeout_ms,
            payload,
        },
    )?;
    request_daemon(&client, &request).map(|response| RunResult::Response(Box::new(response)))
}
fn requires_idempotency_key(operation: OperationV1) -> bool {
    matches!(operation, OperationV1::Mutate | OperationV1::Bootstrap)
}
fn reset_probe_can_recover(error: &podway_protocol::ErrorEnvelopeV1) -> bool {
    matches!(
        error.code().as_str(),
        "WORKSPACE_STATE_UNREADABLE" | "WORKSPACE_SCHEMA_UNSUPPORTED"
    )
}

fn control_preconditions(command: &Command) -> Result<PreconditionsV1, LocalFailure> {
    match command {
        Command::Job {
            command: JobCommand::Cancel { .. },
        } => PreconditionsV1::new(None, None, None, None, None, Some(JobStateV1::Queued)).map_err(
            |_| LocalFailure::request_invalid("job cancellation preconditions are invalid"),
        ),
        _ => Ok(PreconditionsV1::default()),
    }
}

fn execute_start_dry_run(cli: &Cli) -> Result<RunResult, LocalFailure> {
    let Command::Start(args) = &cli.command else {
        return Err(LocalFailure::request_invalid(
            "dry-run requires session start",
        ));
    };
    let (definition, source, digest) = if let Some(preset) = &args.preset {
        let preset = catalog_v1().lookup(preset).ok_or_else(|| {
            LocalFailure::preset_not_found("unknown preset").with_command("session.start")
        })?;
        let admitted = preset
            .validate()
            .map_err(|error| preset_failure(error).with_command("session.start"))?;
        (
            admitted.definition().clone(),
            json!({ "preset": preset.metadata.id }),
            admitted.digest().as_str().to_owned(),
        )
    } else {
        let procedure = args
            .procedure
            .as_deref()
            .ok_or_else(|| LocalFailure::request_invalid("start requires a preset or procedure"))?;
        let root = workspace_target(cli.worktree.clone())?;
        let bytes = read_worktree_procedure(&root, Path::new(procedure))
            .map_err(|failure| failure.with_command("session.start"))?;
        let format = if Path::new(procedure).extension().and_then(OsStr::to_str) == Some("json") {
            ProcedureFormatV1::Json
        } else {
            ProcedureFormatV1::Yaml
        };
        let admitted = parse_procedure_v1(bytes, format)
            .map_err(|error| procedure_config_failure(error).with_command("session.start"))?;
        (
            admitted.definition().clone(),
            json!({ "procedure": procedure }),
            admitted.digest().as_str().to_owned(),
        )
    };
    let first_stage = definition
        .stages
        .first()
        .ok_or_else(|| LocalFailure::request_invalid("procedure has no stages"))?;
    let command = cli
        .command
        .daemon_wire_name()
        .ok_or_else(|| LocalFailure::request_invalid("invalid start command"))?;
    let result = json!({
        "dry_run": true,
        "task": args.task,
        "source": source,
        "procedure_digest": digest,
        "first_stage": { "id": first_stage.id, "title": first_stage.title },
    });
    let text = format!(
        "dry run: first stage {} ({})",
        first_stage.id, first_stage.title
    );
    Ok(local_result(command, result, text))
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

fn read_worktree_procedure(
    root: &WorkspaceTarget,
    procedure: &Path,
) -> Result<Vec<u8>, LocalFailure> {
    read_descriptor_relative_procedure(
        &root.path,
        procedure,
        LocalFailure::catalog("PATH_OUTSIDE_WORKTREE", "cannot open worktree", "procedure"),
    )
}

fn read_offline_procedure(procedure: &Path) -> Result<Vec<u8>, LocalFailure> {
    let parent = procedure
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = procedure
        .file_name()
        .ok_or_else(|| LocalFailure::procedure_not_found("procedure file is not specified"))?;
    let root = fs::canonicalize(parent)
        .map_err(|_| LocalFailure::procedure_not_found("cannot read procedure file"))?;
    read_descriptor_relative_procedure(
        &root,
        Path::new(file_name),
        LocalFailure::procedure_not_found("cannot read procedure file"),
    )
}

fn read_descriptor_relative_procedure(
    root: &Path,
    procedure: &Path,
    root_failure: LocalFailure,
) -> Result<Vec<u8>, LocalFailure> {
    let mut directory = open(
        root,
        OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .map_err(|_| root_failure)?;
    let mut components = procedure.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(component) = component else {
            return Err(LocalFailure::catalog(
                "PATH_OUTSIDE_WORKTREE",
                "procedure must be worktree-relative",
                "procedure",
            ));
        };
        if components.peek().is_some() {
            directory = openat(
                &directory,
                component,
                OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
                Mode::empty(),
            )
            .map_err(procedure_open_failure)?;
            continue;
        }
        let descriptor = openat(
            &directory,
            component,
            OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map_err(procedure_open_failure)?;
        let file = fs::File::from(descriptor);
        let metadata = file
            .metadata()
            .map_err(|_| LocalFailure::procedure_not_found("cannot read procedure file"))?;
        if !metadata.file_type().is_file() {
            return Err(LocalFailure::procedure_invalid(
                "procedure must be a regular file",
            ));
        }
        if metadata.len() > MAX_PROCEDURE_DOCUMENT_BYTES_V1 as u64 {
            return Err(LocalFailure::procedure_invalid(
                "procedure exceeds the maximum document size",
            ));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_PROCEDURE_DOCUMENT_BYTES_V1 as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| LocalFailure::procedure_not_found("cannot read procedure file"))?;
        if bytes.len() > MAX_PROCEDURE_DOCUMENT_BYTES_V1 {
            return Err(LocalFailure::procedure_invalid(
                "procedure exceeds the maximum document size",
            ));
        }
        return Ok(bytes);
    }
    Err(LocalFailure::procedure_not_found(
        "procedure file is not specified",
    ))
}

fn procedure_open_failure(error: Errno) -> LocalFailure {
    if error == Errno::ELOOP {
        LocalFailure::catalog(
            "PATH_OUTSIDE_WORKTREE",
            "procedure source must not traverse symlinks",
            "procedure",
        )
    } else {
        LocalFailure::procedure_not_found("cannot read procedure file")
    }
}

fn execute_local(cli: &Cli) -> Result<Option<RunResult>, LocalFailure> {
    let local_flags = cli.worktree.is_some()
        || cli.timeout.is_some()
        || cli.detach
        || cli.idempotency_key.is_some()
        || cli.if_session_revision.is_some()
        || cli.if_attempt.is_some()
        || cli.if_item_revision.is_some()
        || cli.yes;
    match &cli.command {
        Command::Help { topic } => {
            reject_local_flags(local_flags, "help accepts no daemon-only flags")?;
            let text = help_text(topic.as_deref())?;
            Ok(Some(local_result(
                "help",
                json!({ "topic": topic, "text": text }),
                text,
            )))
        }
        Command::Version => {
            reject_local_flags(local_flags, "version accepts no daemon-only flags")?;
            Ok(Some(local_result(
                "version",
                json!({ "version": env!("CARGO_PKG_VERSION"), "protocol": "podway.ipc/v1" }),
                format!("podway {}", env!("CARGO_PKG_VERSION")),
            )))
        }
        Command::Completions { shell } => {
            reject_local_flags(local_flags, "completions accepts no daemon-only flags")?;
            Ok(Some(local_result(
                "completions",
                json!({ "shell": format!("{shell:?}").to_lowercase(), "script": shell.script() }),
                shell.script(),
            )))
        }
        Command::Preset { command } => {
            reject_local_flags(local_flags, "preset commands accept no daemon-only flags")?;
            Ok(Some(execute_preset(command)?))
        }
        Command::Procedure { command } => {
            reject_local_flags(
                local_flags,
                "procedure commands accept no daemon-only flags",
            )?;
            Ok(Some(execute_procedure(command)?))
        }
        Command::Daemon { command } => {
            if cli.worktree.is_some()
                || cli.timeout.is_some()
                || cli.detach
                || cli.idempotency_key.is_some()
                || ExplicitPreconditions::parse(cli)?.any()
            {
                return Err(LocalFailure::request_invalid(
                    "daemon lifecycle commands accept no daemon request flags",
                ));
            }
            if !matches!(command, DaemonCommand::Uninstall { .. }) && cli.yes {
                return Err(LocalFailure::request_invalid(
                    "--yes applies only to daemon uninstall",
                ));
            }
            if matches!(command, DaemonCommand::Uninstall { .. }) {
                confirm_daemon_uninstall(cli)?;
            }
            Err(LocalFailure::service_unavailable(daemon_command_name(
                command,
            )))
        }
        _ => Ok(None),
    }
}

fn reject_local_flags(has_flags: bool, message: &'static str) -> Result<(), LocalFailure> {
    if has_flags {
        Err(LocalFailure::request_invalid(message))
    } else {
        Ok(())
    }
}

fn local_result(command: &str, result: Value, text: String) -> RunResult {
    let result = result
        .as_object()
        .cloned()
        .expect("local result is always an object");
    RunResult::Local {
        command: command.to_owned(),
        result,
        text,
    }
}

fn procedure_warning_output(warnings: &[ProcedureWarningV1]) -> Vec<Value> {
    warnings
        .iter()
        .map(|warning| {
            let code = warning.code().as_str();
            let path = match (warning.stage_id(), warning.item_id()) {
                (Some(stage_id), Some(item_id)) => {
                    format!("stages/{stage_id}/items/{item_id}")
                }
                (Some(stage_id), None) => format!("stages/{stage_id}"),
                (None, Some(item_id)) => format!("items/{item_id}"),
                (None, None) => "procedure".to_owned(),
            };
            json!({
                "code": code,
                "path": path,
                "message": format!("procedure warning: {code}"),
            })
        })
        .collect()
}

fn execute_preset(command: &PresetCommand) -> Result<RunResult, LocalFailure> {
    match command {
        PresetCommand::List => {
            let presets: Vec<Value> = catalog_v1().list().iter().map(|preset| {
                let metadata = preset.metadata;
                json!({ "id": metadata.id, "name": metadata.name, "version": metadata.version, "description": metadata.description })
            }).collect();
            let text = presets
                .iter()
                .filter_map(|preset| preset.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(local_result(
                "preset.list",
                json!({ "presets": presets }),
                text,
            ))
        }
        PresetCommand::Show { name } => {
            let preset = catalog_v1().lookup(name).ok_or_else(|| {
                LocalFailure::preset_not_found("unknown preset").with_command("preset.show")
            })?;
            let admitted = preset
                .validate()
                .map_err(|error| preset_failure(error).with_command("preset.show"))?;
            let warnings = procedure_warning_output(admitted.warnings());
            Ok(local_result(
                "preset.show",
                json!({
                    "preset": preset.metadata.id,
                    "metadata": preset.metadata,
                    "digest": admitted.digest().as_str(),
                    "procedure": admitted.definition(),
                    "warnings": warnings,
                }),
                preset.yaml.to_owned(),
            ))
        }
        PresetCommand::Explain { name } => {
            let preset = catalog_v1().lookup(name).ok_or_else(|| {
                LocalFailure::preset_not_found("unknown preset").with_command("preset.explain")
            })?;
            let admitted = preset
                .validate()
                .map_err(|error| preset_failure(error).with_command("preset.explain"))?;
            let stages: Vec<Value> = admitted
                .definition()
                .stages
                .iter()
                .map(|stage| json!({ "id": stage.id, "title": stage.title }))
                .collect();
            let text = format!(
                "{}\n{}\nStages: {}",
                preset.metadata.name,
                preset.metadata.description,
                stages
                    .iter()
                    .filter_map(|stage| stage.get("id").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            Ok(local_result(
                "preset.explain",
                json!({ "preset": preset.metadata, "stages": stages }),
                text,
            ))
        }
    }
}

fn execute_procedure(command: &ProcedureCommand) -> Result<RunResult, LocalFailure> {
    let (file, warnings_as_errors, name) = match command {
        ProcedureCommand::Validate {
            file,
            warnings_as_errors,
        } => (file, *warnings_as_errors, "procedure.validate"),
        ProcedureCommand::Show { file, .. } => (file, false, "procedure.show"),
    };
    let bytes = read_offline_procedure(file).map_err(|failure| failure.with_command(name))?;
    std::str::from_utf8(&bytes).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(name)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureFormatV1::Json
    } else {
        ProcedureFormatV1::Yaml
    };
    let validated = parse_procedure_v1(&bytes, format)
        .map_err(|error| procedure_config_failure(error).with_command(name))?;
    if warnings_as_errors {
        validated
            .clone()
            .admit(ProcedureWarningPolicyV1::Reject)
            .map_err(|error| procedure_config_failure(error).with_command(name))?;
    }
    let warnings = procedure_warning_output(validated.warnings());
    let result = json!({
        "file": file.display().to_string(),
        "digest": validated.digest().as_str(),
        "procedure": validated.definition(),
        "warnings": warnings,
        "canonical_json": validated.canonical_json().as_str(),
    });
    let text = match command {
        ProcedureCommand::Validate { .. } => format!(
            "{} ({})",
            validated.definition().name,
            validated.digest().as_str()
        ),
        ProcedureCommand::Show {
            canonical: true, ..
        } => validated.canonical_json().as_str().to_owned(),
        ProcedureCommand::Show {
            canonical: false, ..
        } => String::from_utf8(bytes).map_err(|_| {
            LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(name)
        })?,
    };
    Ok(local_result(name, result, text))
}

fn daemon_command_name(command: &DaemonCommand) -> &'static str {
    match command {
        DaemonCommand::Install { .. } => "daemon.install",
        DaemonCommand::Uninstall { .. } => "daemon.uninstall",
        DaemonCommand::Start => "daemon.start",
        DaemonCommand::Stop => "daemon.stop",
        DaemonCommand::Restart => "daemon.restart",
        DaemonCommand::Status => "daemon.status",
        DaemonCommand::Logs { .. } => "daemon.logs",
    }
}

fn validate_daemon_flags(cli: &Cli) -> Result<(), LocalFailure> {
    let command = &cli.command;
    let wire = command
        .daemon_wire_name()
        .ok_or_else(|| LocalFailure::request_invalid("unsupported command"))?;
    if !command.is_mutation() && (cli.detach || cli.idempotency_key.is_some()) {
        return Err(LocalFailure::request_invalid(
            "--detach and --idempotency-key apply only to mutations",
        ));
    }
    if command.is_dry_run() && (cli.detach || cli.idempotency_key.is_some()) {
        return Err(LocalFailure::request_invalid(
            "dry-run commands cannot detach or use an idempotency key",
        ));
    }
    let explicit = ExplicitPreconditions::parse(cli)?;
    if explicit.any() && !command.needs_preflight() {
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
    if cli.if_attempt.is_some()
        && matches!(
            command,
            Command::Start(StartArgs { replace: true, .. })
                | Command::Reopen(_)
                | Command::Reset(_)
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
    if matches!(command, Command::Next(ReadArgs { verbose: true, .. })) {
        return Err(LocalFailure::request_invalid(
            "--verbose applies only to status",
        ));
    }
    if wire == "workspace.reset_all"
        && let Command::Reset(args) = command
    {
        if !args.force {
            return Err(LocalFailure::request_invalid(
                "reset --all requires --force",
            ));
        }
        if args.dry_run {
            return Err(LocalFailure::request_invalid(
                "reset --all does not support --dry-run",
            ));
        }
    }
    if let Command::Start(StartArgs {
        procedure: Some(procedure),
        ..
    }) = command
        && (PathBuf::from(procedure).is_absolute()
            || PathBuf::from(procedure)
                .components()
                .any(|component| matches!(component, Component::ParentDir)))
    {
        return Err(LocalFailure::request_invalid(
            "procedure must be worktree-relative",
        ));
    }
    if let Command::Reset(args) = command
        && args.force
        && !args.all
    {
        return Err(LocalFailure::request_invalid(
            "--force applies only to reset --all",
        ));
    }
    Ok(())
}

fn validate_command_shape(command: &Command) -> Result<(), LocalFailure> {
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
    Ok(())
}

fn confirm_if_required(cli: &Cli, command: &str) -> Result<(), LocalFailure> {
    if !cli.command.is_destructive() || cli.yes {
        return Ok(());
    }
    if cli.json || !io::stdin().is_terminal() {
        return Err(LocalFailure::confirmation_required(command));
    }
    let mut stdout = io::stdout().lock();
    write!(stdout, "This operation is destructive. Continue? [y/N] ")
        .map_err(|_| LocalFailure::response_invalid("cannot write confirmation prompt"))?;
    stdout
        .flush()
        .map_err(|_| LocalFailure::response_invalid("cannot write confirmation prompt"))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|_| LocalFailure::daemon_unavailable(command))?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(LocalFailure::confirmation_required(command))
    }
}
fn confirm_daemon_uninstall(cli: &Cli) -> Result<(), LocalFailure> {
    if cli.yes {
        return Ok(());
    }
    if cli.json || !io::stdin().is_terminal() {
        return Err(LocalFailure::confirmation_required("daemon.uninstall"));
    }
    let mut stdout = io::stdout().lock();
    write!(stdout, "Remove the daemon service? [y/N] ")
        .map_err(|_| LocalFailure::response_invalid("cannot write confirmation prompt"))?;
    stdout
        .flush()
        .map_err(|_| LocalFailure::response_invalid("cannot write confirmation prompt"))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|_| LocalFailure::daemon_unavailable("daemon.uninstall"))?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(LocalFailure::confirmation_required("daemon.uninstall"))
    }
}

fn daemon_payload(
    command: &mut Command,
) -> Result<(OperationV1, Map<String, Value>), LocalFailure> {
    let mut payload = Map::new();
    let operation = match command {
        Command::Workspace {
            command: WorkspaceCommand::Repair,
        }
        | Command::Job {
            command: JobCommand::Cancel { .. },
        } => OperationV1::Control,
        Command::Init { .. } | Command::Reset(ResetArgs { all: true, .. }) => {
            OperationV1::Bootstrap
        }
        _ if command.is_dry_run() => OperationV1::Query,
        _ if command.is_mutation() => OperationV1::Mutate,
        _ => OperationV1::Query,
    };

    match command {
        Command::Init { repair } => {
            if *repair {
                payload.insert("repair".to_owned(), Value::Bool(true));
            }
        }
        Command::Workspace {
            command: WorkspaceCommand::Repair,
        }
        | Command::Workspace {
            command: WorkspaceCommand::Show,
        } => {}
        Command::Doctor { deep } => {
            payload.insert("deep".to_owned(), Value::Bool(*deep));
        }
        Command::Start(args) => {
            payload.insert("task_title".to_owned(), Value::String(args.task.clone()));
            if let Some(preset) = &args.preset {
                payload.insert("preset".to_owned(), Value::String(preset.clone()));
            }
            if let Some(procedure) = &args.procedure {
                payload.insert("procedure".to_owned(), Value::String(procedure.clone()));
            }
            if args.dry_run {
                payload.insert("dry_run".to_owned(), Value::Bool(true));
            } else if args.replace {
                payload.insert("confirmed".to_owned(), Value::Bool(true));
            }
        }
        Command::Status(args) | Command::Next(args) => read_payload(&mut payload, args),
        Command::Complete => {}
        Command::Skip { reason } => {
            if let Some(reason) = reason {
                payload.insert("reason".to_owned(), Value::String(reason.clone()));
            }
        }
        Command::Retry { reason } | Command::Block { reason } | Command::Cancel { reason } => {
            payload.insert("reason".to_owned(), Value::String(reason.clone()));
        }
        Command::Return(args) | Command::Reopen(args) => {
            payload.insert(
                "destination_stage_id".to_owned(),
                Value::String(args.to.clone()),
            );
            payload.insert("reason".to_owned(), Value::String(args.reason.clone()));
            if args.dry_run {
                payload.insert("dry_run".to_owned(), Value::Bool(true));
            }
        }
        Command::Unblock { blocker_id, all } => {
            if let Some(blocker_id) = blocker_id {
                payload.insert("blocker_id".to_owned(), Value::String(blocker_id.clone()));
            }
            payload.insert("all".to_owned(), Value::Bool(*all));
        }
        Command::Reset(args) => {
            if args.dry_run {
                payload.insert("dry_run".to_owned(), Value::Bool(true));
            } else {
                payload.insert("confirmed".to_owned(), Value::Bool(true));
            }
        }
        Command::Check { item_id } | Command::Uncheck { item_id } | Command::Clear { item_id } => {
            payload.insert("item_id".to_owned(), Value::String(item_id.clone()));
        }
        Command::Set(args) => {
            let value = args
                .value
                .clone()
                .ok_or_else(|| LocalFailure::request_invalid("set requires a value or --stdin"))?;
            payload.insert("item_id".to_owned(), Value::String(args.item_id.clone()));
            payload.insert("value".to_owned(), Value::String(value));
        }
        Command::Add { item_id, value } => {
            payload.insert("item_id".to_owned(), Value::String(item_id.clone()));
            payload.insert("value".to_owned(), Value::String(value.clone()));
        }
        Command::Remove {
            item_id,
            value,
            ignore_missing,
        } => {
            payload.insert("item_id".to_owned(), Value::String(item_id.clone()));
            payload.insert("value".to_owned(), Value::String(value.clone()));
            payload.insert("ignore_missing".to_owned(), Value::Bool(*ignore_missing));
        }
        Command::Attach(args) => {
            payload.insert("item_id".to_owned(), Value::String(args.item_id.clone()));
            if let Some(path) = &args.path {
                payload.insert("path".to_owned(), Value::String(path.clone()));
            }
            if let Some(reference) = &args.reference {
                payload.insert("reference".to_owned(), Value::String(reference.clone()));
                payload.insert(
                    "digest".to_owned(),
                    Value::String(args.digest.clone().expect("validated reference digest")),
                );
                payload.insert(
                    "size_bytes".to_owned(),
                    Value::from(args.size.expect("validated reference size")),
                );
            }
            if let Some(media_type) = &args.media_type {
                payload.insert("media_type".to_owned(), Value::String(media_type.clone()));
            }
        }
        Command::Job { command } => match command {
            JobCommand::List { state } => {
                if let Some(state) = state {
                    payload.insert("state".to_owned(), Value::String(state.clone()));
                }
            }
            JobCommand::Status { job_id }
            | JobCommand::Wait { job_id }
            | JobCommand::Cancel { job_id } => {
                payload.insert("job_id".to_owned(), Value::String(job_id.clone()));
            }
        },
        Command::CompleteDynamic { .. }
        | Command::Help { .. }
        | Command::Version
        | Command::Completions { .. }
        | Command::Procedure { .. }
        | Command::Preset { .. }
        | Command::Daemon { .. } => {
            return Err(LocalFailure::request_invalid("unsupported daemon command"));
        }
    }
    Ok((operation, payload))
}

fn read_payload(payload: &mut Map<String, Value>, args: &ReadArgs) {
    if args.verbose {
        payload.insert("verbose".to_owned(), Value::Bool(true));
    }
    if args.wait_for_idle {
        payload.insert("wait_for_idle".to_owned(), Value::Bool(true));
    }
    if let Some(after_job) = &args.after_job {
        payload.insert("after_job_id".to_owned(), Value::String(after_job.clone()));
    }
}

fn prepare_stdin_payload(command: &mut Command) -> Result<(), LocalFailure> {
    if let Command::Set(args) = command
        && args.stdin
    {
        args.value = Some(read_stdin_text()?);
    }
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
            .map_err(|_| LocalFailure::daemon_unavailable("cli"))?
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

fn daemon_client(wait_timeout_ms: u64) -> Result<DaemonClientV1, LocalFailure> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| LocalFailure::daemon_unavailable("cli"))?;
    let temporary = env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let paths = ServiceRuntimePathsV1::for_user(home, temporary, geteuid().as_raw())
        .map_err(|_| LocalFailure::daemon_unavailable("cli"))?;
    let read_timeout = Duration::from_millis(wait_timeout_ms.saturating_add(1_000))
        .max(DEFAULT_DAEMON_CONNECT_TIMEOUT_V1);
    let timeouts = DaemonClientTimeoutsV1::new(
        DEFAULT_DAEMON_CONNECT_TIMEOUT_V1,
        read_timeout,
        DEFAULT_DAEMON_WRITE_TIMEOUT_V1,
    )
    .map_err(|_| LocalFailure::daemon_unavailable("cli"))?;
    Ok(DaemonClientV1::with_timeouts(paths, timeouts))
}

struct RequestSpec {
    operation: OperationV1,
    expected_uuid: Option<WorkspaceId>,
    idempotency_key: Option<IdempotencyKeyV1>,
    preconditions: PreconditionsV1,
    detach: bool,
    wait_timeout_ms: u64,
    payload: Map<String, Value>,
}

impl RequestSpec {
    fn query(
        wait_timeout_ms: u64,
        payload: Map<String, Value>,
        expected_uuid: Option<WorkspaceId>,
    ) -> Self {
        Self {
            operation: OperationV1::Query,
            expected_uuid,
            idempotency_key: None,
            preconditions: PreconditionsV1::default(),
            detach: false,
            wait_timeout_ms,
            payload,
        }
    }
}

fn build_request(
    command: &str,
    target: &WorkspaceTarget,
    spec: RequestSpec,
) -> Result<RequestEnvelopeV1, LocalFailure> {
    let RequestSpec {
        operation,
        expected_uuid,
        idempotency_key,
        preconditions,
        detach,
        wait_timeout_ms,
        mut payload,
    } = spec;
    let selector = target.selector(expected_uuid.clone())?;
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(selector)
            .map_err(|_| LocalFailure::request_invalid("cannot encode worktree selector"))?,
    );
    let request_id = RequestIdV1::new(Uuid::new_v4().to_string())
        .map_err(|_| LocalFailure::daemon_unavailable(command))?;
    let client = ClientInfoV1::new("podway", env!("CARGO_PKG_VERSION"), std::process::id())
        .map_err(|_| LocalFailure::daemon_unavailable(command))?;
    let command_name = CommandNameV1::new(command.to_owned())
        .map_err(|_| LocalFailure::request_invalid("invalid command"))?;
    let workspace = target.context(expected_uuid)?;
    let options = RequestOptionsV1::new(detach, wait_timeout_ms)
        .map_err(|_| LocalFailure::request_invalid("invalid timeout"))?;
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id,
        client,
        operation,
        command: command_name,
        workspace: Some(workspace),
        idempotency_key,
        preconditions,
        options,
        payload,
    })
    .map_err(|_| LocalFailure::request_invalid("invalid request"))
}

fn mutation_key(value: Option<String>) -> Result<IdempotencyKeyV1, LocalFailure> {
    IdempotencyKeyV1::new(value.unwrap_or_else(|| Uuid::new_v4().to_string()))
        .map_err(|_| LocalFailure::request_invalid("invalid idempotency key"))
}

fn request_daemon(
    client: &DaemonClientV1,
    request: &RequestEnvelopeV1,
) -> Result<ResponseEnvelopeV1, LocalFailure> {
    client.request(request).map_err(|error| {
        map_client_error(error)
            .with_correlation(request.command().as_str(), request.request_id().as_str())
    })
}

fn re_correlate_preflight_error(
    error: &podway_protocol::ErrorEnvelopeV1,
    command: &str,
) -> Result<RunResult, LocalFailure> {
    let mut envelope = serde_json::to_value(error)
        .map_err(|_| LocalFailure::response_invalid("status preflight error cannot be read"))?;
    envelope
        .as_object_mut()
        .ok_or_else(|| LocalFailure::response_invalid("status preflight error is invalid"))?
        .insert("command".to_owned(), Value::String(command.to_owned()));
    let error = serde_json::from_value(envelope)
        .map_err(|_| LocalFailure::response_invalid("status preflight error is invalid"))?;
    Ok(RunResult::Response(Box::new(ResponseEnvelopeV1::Error(
        error,
    ))))
}

fn map_client_error(error: DaemonClientErrorV1) -> LocalFailure {
    match error {
        DaemonClientErrorV1::RequestAdmission { .. }
        | DaemonClientErrorV1::RequestEncoding { .. } => {
            LocalFailure::request_invalid("the request cannot be admitted")
        }
        DaemonClientErrorV1::ResponseDecoding { .. }
        | DaemonClientErrorV1::Framing { .. }
        | DaemonClientErrorV1::MissingResponse
        | DaemonClientErrorV1::ResponseMismatch { .. } => {
            LocalFailure::response_invalid("the daemon returned an invalid response")
        }
        DaemonClientErrorV1::InvalidTimeout { .. }
        | DaemonClientErrorV1::Connection { .. }
        | DaemonClientErrorV1::SocketConfiguration { .. }
        | DaemonClientErrorV1::Timeout { .. } => LocalFailure::daemon_unavailable("cli"),
    }
}

fn parse_timeout_millis(value: &str) -> Result<u64, String> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix("ms") {
        (number, 1_u64)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1_000)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60_000)
    } else {
        return Err("duration must use ms, s, or m".to_owned());
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("duration must be an unsigned integer followed by ms, s, or m".to_owned());
    }
    let millis = number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .ok_or_else(|| "duration is out of range".to_owned())?;
    if millis > MAX_WAIT_TIMEOUT_MILLIS_V1 {
        return Err(format!(
            "duration exceeds the maximum of {MAX_WAIT_TIMEOUT_MILLIS_V1}ms"
        ));
    }
    Ok(millis)
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

fn render_result(result: &RunResult, json_output: bool, quiet: bool) -> i32 {
    render_result_with_clock(result, json_output, quiet, &SystemLocalEnvelopeClock)
}

fn render_result_with_clock(
    result: &RunResult,
    json_output: bool,
    quiet: bool,
    clock: &impl LocalEnvelopeClock,
) -> i32 {
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    render_result_with_clock_and_writers(
        result,
        json_output,
        quiet,
        clock,
        &mut stdout,
        &mut stderr,
    )
}

fn render_result_with_clock_and_writers(
    result: &RunResult,
    json_output: bool,
    quiet: bool,
    clock: &impl LocalEnvelopeClock,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    match result {
        RunResult::Response(response) => render_response_with_clock_and_writers(
            response,
            json_output,
            quiet,
            clock,
            stdout,
            stderr,
        ),
        RunResult::Local {
            command,
            result,
            text,
        } => {
            if json_output {
                let generated_at = match local_generated_at(clock) {
                    Ok(timestamp) => timestamp,
                    Err(failure) => return render_clock_failure_to(failure, stderr),
                };
                let output = json!({ "schema": "podway.output/v1", "request_id": Uuid::new_v4().to_string(), "command": command, "generated_at": generated_at.as_str(), "result": result, "warnings": [] });
                if serde_json::to_writer(&mut *stdout, &output).is_err()
                    || writeln!(stdout).is_err()
                {
                    return LOCAL_CLIENT_EXIT;
                }
            } else if !quiet && !text.is_empty() && writeln!(stdout, "{text}").is_err() {
                return LOCAL_CLIENT_EXIT;
            }
            0
        }
    }
}

fn render_clock_failure_to(failure: LocalFailure, stderr: &mut dyn Write) -> i32 {
    if writeln!(stderr, "error: {}", failure.message).is_err() {
        LOCAL_CLIENT_EXIT
    } else {
        failure.exit_code
    }
}

fn render_response_with_clock_and_writers(
    response: &ResponseEnvelopeV1,
    json_output: bool,
    quiet: bool,
    clock: &impl LocalEnvelopeClock,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if let ResponseEnvelopeV1::Output(output) = response
        && let Err(failure) = validate_typed_output_result(output)
    {
        return render_local_failure_with_clock_and_writers(
            failure,
            json_output,
            clock,
            stdout,
            stderr,
        );
    }
    if json_output {
        if serde_json::to_writer(&mut *stdout, response).is_err() || writeln!(stdout).is_err() {
            return LOCAL_CLIENT_EXIT;
        }
    } else if (!quiet || matches!(response, ResponseEnvelopeV1::Error(_)))
        && let Err(failure) = render_human_response(response, stdout, stderr)
    {
        return render_local_failure_with_clock_and_writers(failure, false, clock, stdout, stderr);
    }
    match response {
        ResponseEnvelopeV1::Output(_) => 0,
        ResponseEnvelopeV1::Error(error) => i32::from(error.exit_code().get()),
    }
}

fn validate_typed_output_result(
    output: &podway_protocol::OutputEnvelopeV1,
) -> Result<(), LocalFailure> {
    match output.command().as_str() {
        "session.status" => StatusResultV1::from_result_map(output.result())
            .map(|_| ())
            .map_err(|_| typed_result_failure(output)),
        "session.next" => NextResultV1::from_result_map(output.result())
            .map(|_| ())
            .map_err(|_| typed_result_failure(output)),
        _ => Ok(()),
    }
}

fn typed_result_failure(output: &podway_protocol::OutputEnvelopeV1) -> LocalFailure {
    LocalFailure::response_invalid(format!(
        "the daemon returned an invalid {} result",
        output.command().as_str()
    ))
    .with_correlation(output.command().as_str(), output.request_id().as_str())
}

fn render_local_failure(failure: LocalFailure, json_output: bool) -> i32 {
    render_local_failure_with_clock(failure, json_output, &SystemLocalEnvelopeClock)
}

fn render_local_failure_with_clock(
    failure: LocalFailure,
    json_output: bool,
    clock: &impl LocalEnvelopeClock,
) -> i32 {
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    render_local_failure_with_clock_and_writers(
        failure,
        json_output,
        clock,
        &mut stdout,
        &mut stderr,
    )
}

fn render_local_failure_with_clock_and_writers(
    failure: LocalFailure,
    json_output: bool,
    clock: &impl LocalEnvelopeClock,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if json_output {
        let generated_at = match local_generated_at(clock) {
            Ok(timestamp) => timestamp,
            Err(clock_failure) => return render_clock_failure_to(clock_failure, stderr),
        };
        let request_id = failure
            .request_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let output = json!({ "schema": "podway.error/v1", "request_id": request_id, "command": failure.command, "generated_at": generated_at.as_str(), "code": failure.code, "message": failure.message, "retryable": failure.retryable, "exit_code": failure.exit_code, "details": {} });
        if serde_json::to_writer(&mut *stdout, &output).is_err() || writeln!(stdout).is_err() {
            return LOCAL_CLIENT_EXIT;
        }
    } else if writeln!(stderr, "error: {}", failure.message).is_err() {
        return LOCAL_CLIENT_EXIT;
    }
    failure.exit_code
}

fn render_human_response(
    response: &ResponseEnvelopeV1,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), LocalFailure> {
    match response {
        ResponseEnvelopeV1::Output(output) => match output.command().as_str() {
            "session.status" => {
                let status = StatusResultV1::from_result_map(output.result())
                    .map_err(|_| typed_result_failure(output))?;
                render_output_metadata(stdout, output)?;
                render_status_text(stdout, &status)?;
                render_warnings(stdout, output.warnings())?;
            }
            "session.next" => {
                let next = NextResultV1::from_result_map(output.result())
                    .map_err(|_| typed_result_failure(output))?;
                render_output_metadata(stdout, output)?;
                render_next_text(stdout, &next)?;
                render_warnings(stdout, output.warnings())?;
            }
            _ => render_generic_output(stdout, output)?,
        },
        ResponseEnvelopeV1::Error(error) => {
            write_text_line(
                stderr,
                format_args!("error: {}: {}", error.code().as_str(), error.message()),
            )?;
        }
    }
    Ok(())
}

fn render_write_failure() -> LocalFailure {
    LocalFailure::response_invalid("cannot write command output")
}

fn write_text_line(
    writer: &mut dyn Write,
    text: std::fmt::Arguments<'_>,
) -> Result<(), LocalFailure> {
    writer.write_fmt(text).map_err(|_| render_write_failure())?;
    writer.write_all(b"\n").map_err(|_| render_write_failure())
}

fn render_output_metadata(
    stdout: &mut dyn Write,
    output: &podway_protocol::OutputEnvelopeV1,
) -> Result<(), LocalFailure> {
    if let Some(workspace) = output.workspace() {
        write_text_line(
            stdout,
            format_args!(
                "workspace: {} {} sequence={}",
                workspace.uuid().as_str(),
                workspace.root(),
                workspace.latest_workspace_sequence()
            ),
        )?;
    }
    if let Some(session) = output.session() {
        write_text_line(
            stdout,
            format_args!(
                "session: {} {} {:?} revision={}",
                session.id().as_str(),
                session.title(),
                session.lifecycle(),
                session.revision_after().get()
            ),
        )?;
    }
    if let Some(job) = output.job() {
        write_text_line(
            stdout,
            format_args!(
                "job: {} {:?} sequence={} submitted_at={} claimed_at={} finished_at={}",
                job.id().as_str(),
                job.state(),
                job.sequence(),
                job.submitted_at().as_str(),
                job.claimed_at().map(Rfc3339MillisV1::as_str).unwrap_or("-"),
                job.finished_at()
                    .map(Rfc3339MillisV1::as_str)
                    .unwrap_or("-")
            ),
        )?;
    }
    Ok(())
}

fn render_status_text(stdout: &mut dyn Write, status: &StatusResultV1) -> Result<(), LocalFailure> {
    write_text_line(stdout, format_args!("task: {}", status.task.title))?;
    write_text_line(
        stdout,
        format_args!(
            "session: {} {:?} revision={} created_at={} completed_at={} cancelled_at={}",
            status.session.id.as_str(),
            status.session.lifecycle,
            status.session.revision.get(),
            status.session.created_at.as_str(),
            status
                .session
                .completed_at
                .as_ref()
                .map(Rfc3339MillisV1::as_str)
                .unwrap_or("-"),
            status
                .session
                .cancelled_at
                .as_ref()
                .map(Rfc3339MillisV1::as_str)
                .unwrap_or("-")
        ),
    )?;
    match &status.current {
        Some(current) => {
            write_text_line(
                stdout,
                format_args!(
                    "current: {} {} attempt={} id={} blocked={} ready_to_complete={}",
                    current.stage_id.as_str(),
                    current.title,
                    current.attempt_number,
                    current.attempt_id.as_str(),
                    current.blocked,
                    current.ready_to_complete
                ),
            )?;
        }
        None => write_text_line(stdout, format_args!("current: none"))?,
    }
    for stage in &status.stages {
        write_text_line(
            stdout,
            format_args!(
                "stage: {} {} status={:?} latest_attempt={}",
                stage.id.as_str(),
                stage.title,
                stage.status,
                stage.latest_attempt_number
            ),
        )?;
    }
    for item in &status.items {
        let value = serde_json::to_string(&item.value).expect("JSON item values serialize");
        write_text_line(
            stdout,
            format_args!(
                "item: {} {:?} required={} satisfied={} revision={} prompt={} value={}",
                item.id.as_str(),
                item.item_type,
                item.required,
                item.satisfied,
                item.revision.get(),
                item.prompt,
                value
            ),
        )?;
    }
    for blocker in &status.blockers {
        write_text_line(
            stdout,
            format_args!(
                "blocker: {} attempt={} reason={}",
                blocker.id.as_str(),
                blocker.attempt_id.as_str(),
                blocker.reason
            ),
        )?;
    }
    write_text_line(
        stdout,
        format_args!(
            "queue: pending_mutations={} queued_count={} running_job_id={} latest_workspace_sequence={}",
            status.queue.pending_mutations,
            status.queue.queued_count,
            status
                .queue
                .running_job_id
                .as_ref()
                .map(|id| id.as_str())
                .unwrap_or("-"),
            status.queue.latest_workspace_sequence
        ),
    )
}

fn render_next_text(stdout: &mut dyn Write, next: &NextResultV1) -> Result<(), LocalFailure> {
    match &next.stage {
        Some(stage) => {
            write_text_line(
                stdout,
                format_args!(
                    "stage: {} {} attempt={} id={} instructions={}",
                    stage.id.as_str(),
                    stage.title,
                    stage.attempt_number,
                    stage.attempt_id.as_str(),
                    serde_json::to_string(&stage.instructions).expect("instructions serialize")
                ),
            )?;
        }
        None => write_text_line(stdout, format_args!("stage: none"))?,
    }
    for item in &next.missing_required_items {
        write_text_line(
            stdout,
            format_args!(
                "missing_item: {} {:?} prompt={}",
                item.id.as_str(),
                item.item_type,
                item.prompt
            ),
        )?;
    }
    for blocker in &next.blockers {
        write_text_line(
            stdout,
            format_args!(
                "blocker: {} attempt={} reason={}",
                blocker.id.as_str(),
                blocker.attempt_id.as_str(),
                blocker.reason
            ),
        )?;
    }
    write_text_line(
        stdout,
        format_args!(
            "allowed_actions: complete={} skip={} retry={} return_to={} cancel={}",
            next.allowed_actions.complete,
            next.allowed_actions.skip,
            next.allowed_actions.retry,
            next.allowed_actions
                .return_to
                .iter()
                .map(|stage| stage.as_str())
                .collect::<Vec<_>>()
                .join(","),
            next.allowed_actions.cancel
        ),
    )?;
    match &next.next_stage_after_completion {
        Some(stage) => write_text_line(
            stdout,
            format_args!(
                "next_stage_after_completion: {} {}",
                stage.id.as_str(),
                stage.title
            ),
        )?,
        None => write_text_line(stdout, format_args!("next_stage_after_completion: none"))?,
    }
    for suggestion in &next.suggestions {
        write_text_line(
            stdout,
            format_args!(
                "suggestion: {} {}",
                suggestion.command,
                suggestion.argv.join(" ")
            ),
        )?;
    }
    Ok(())
}

fn render_generic_output(
    stdout: &mut dyn Write,
    output: &podway_protocol::OutputEnvelopeV1,
) -> Result<(), LocalFailure> {
    render_output_metadata(stdout, output)?;
    if output.result().is_empty() {
        write_text_line(
            stdout,
            format_args!("command: {}", output.command().as_str()),
        )?;
    } else {
        let result = serde_json::to_string_pretty(output.result()).expect("JSON result serializes");
        write_text_line(stdout, format_args!("result: {result}"))?;
    }
    render_warnings(stdout, output.warnings())
}

fn render_warnings(
    stdout: &mut dyn Write,
    warnings: &[Map<String, Value>],
) -> Result<(), LocalFailure> {
    for warning in warnings {
        let warning = serde_json::to_string(warning).expect("JSON warning serializes");
        write_text_line(stdout, format_args!("warning: {warning}"))?;
    }
    Ok(())
}

fn dynamic_completion(worktree: Option<PathBuf>, kind: &str) -> Result<RunResult, LocalFailure> {
    let target = match workspace_target(worktree) {
        Ok(target) => target,
        Err(_) => {
            return Ok(empty_dynamic_completion());
        }
    };
    let client = match daemon_client(200) {
        Ok(client) => client,
        Err(_) => {
            return Ok(empty_dynamic_completion());
        }
    };
    let command = match kind {
        "items" | "blockers" => "session.status",
        "returns" => "session.next",
        "jobs" => "job.list",
        _ => return Ok(empty_dynamic_completion()),
    };
    let request = match build_request(command, &target, RequestSpec::query(200, Map::new(), None)) {
        Ok(request) => request,
        Err(_) => {
            return Ok(empty_dynamic_completion());
        }
    };
    let candidates = match request_daemon(&client, &request) {
        Ok(ResponseEnvelopeV1::Output(output)) => dynamic_candidates(output.result(), kind),
        Ok(ResponseEnvelopeV1::Error(_)) | Err(_) => Vec::new(),
    };
    Ok(local_result(
        "__complete",
        json!({ "candidates": candidates }),
        candidates.join("\n"),
    ))
}

fn empty_dynamic_completion() -> RunResult {
    local_result("__complete", json!({ "candidates": [] }), String::new())
}

fn dynamic_candidates(result: &Map<String, Value>, kind: &str) -> Vec<String> {
    match kind {
        "items" => StatusResultV1::from_result_map(result)
            .map(|status| {
                status
                    .items
                    .into_iter()
                    .map(|item| item.id.as_str().to_owned())
                    .take(128)
                    .collect()
            })
            .unwrap_or_default(),
        "blockers" => StatusResultV1::from_result_map(result)
            .map(|status| {
                status
                    .blockers
                    .into_iter()
                    .map(|blocker| blocker.id.as_str().to_owned())
                    .take(128)
                    .collect()
            })
            .unwrap_or_default(),
        "returns" => NextResultV1::from_result_map(result)
            .map(|next| {
                next.allowed_actions
                    .return_to
                    .into_iter()
                    .map(|stage| stage.as_str().to_owned())
                    .take(128)
                    .collect()
            })
            .unwrap_or_default(),
        "jobs" => result
            .get("jobs")
            .cloned()
            .and_then(|jobs| serde_json::from_value::<Vec<JobOutputV1>>(jobs).ok())
            .map(|jobs| {
                jobs.into_iter()
                    .map(|job| job.id().as_str().to_owned())
                    .take(128)
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn help_text(topic: Option<&str>) -> Result<String, LocalFailure> {
    let text = match topic.unwrap_or("overview") {
        "overview" => {
            "Podway coordinates durable worktree-local procedures.\n\nUsage:\n  podway help <route>\n\nExamples:\n  podway start --preset sw-dev --task 'add retry backoff'\n  podway status --json\n  podway next"
        }
        "workflow" => {
            "Workflow:\n  podway start --preset sw-dev --task 'implement feature'\n  podway next\n  podway check reproduced\n  podway complete"
        }
        "rework" => {
            "Rework:\n  podway return --to implement --reason 'review found a gap' --dry-run\n  podway reopen --to implement --reason 'follow-up'"
        }
        "automation" => {
            "Automation:\n  podway complete --if-session-revision 12 --if-attempt <uuid> --idempotency-key task-42 --json"
        }
        "procedures" => {
            "Procedures:\n  podway procedure validate .podway/procedures/custom.yaml\n  podway start --procedure .podway/procedures/custom.yaml --task 'perform work'"
        }
        "daemon" => {
            "Daemon lifecycle grammar:\n  podway daemon status\n  podway daemon install --daemon-path /absolute/podwayd\nLifecycle effects are unavailable until G007."
        }
        "artifacts" => {
            "Artifacts:\n  podway attach report report.md --media-type text/markdown\n  podway attach report --reference build:42 --digest sha256:<hex> --size 42 --media-type text/plain"
        }
        "help" => "Usage:\n  podway help [<route>]\n\nExample:\n  podway help session.start",
        "version" => "Usage:\n  podway version\n\nExample:\n  podway version",
        "completions" => {
            "Usage:\n  podway completions <bash|zsh|fish>\n\nExample:\n  podway completions bash"
        }
        "procedure.validate" => {
            "Usage:\n  podway procedure validate <file> [--warnings-as-errors]\n\nExample:\n  podway procedure validate .podway/procedures/custom.yaml"
        }
        "procedure.show" => {
            "Usage:\n  podway procedure show <file> [--canonical]\n\nExample:\n  podway procedure show .podway/procedures/custom.yaml --canonical"
        }
        "preset.list" => "Usage:\n  podway preset list\n\nExample:\n  podway preset list",
        "preset.show" => {
            "Usage:\n  podway preset show <name>\n\nExample:\n  podway preset show sw-dev"
        }
        "preset.explain" => {
            "Usage:\n  podway preset explain <name>\n\nExample:\n  podway preset explain sw-dev"
        }
        "daemon.install" => {
            "Usage:\n  podway daemon install [--daemon-path <path>]\n\nExample:\n  podway daemon install --daemon-path /absolute/podwayd"
        }
        "daemon.uninstall" => {
            "Usage:\n  podway daemon uninstall [--purge-logs] [--yes]\n\nExample:\n  podway daemon uninstall --yes"
        }
        "daemon.start" => "Usage:\n  podway daemon start\n\nExample:\n  podway daemon start",
        "daemon.stop" => "Usage:\n  podway daemon stop\n\nExample:\n  podway daemon stop",
        "daemon.restart" => "Usage:\n  podway daemon restart\n\nExample:\n  podway daemon restart",
        "daemon.status" => "Usage:\n  podway daemon status\n\nExample:\n  podway daemon status",
        "daemon.logs" => {
            "Usage:\n  podway daemon logs [--follow] [--lines <n>]\n\nExample:\n  podway daemon logs --lines 100"
        }
        "workspace.init" => {
            "Usage:\n  podway init [--repair]\n\nExamples:\n  podway init\n  podway init --repair"
        }
        "workspace.doctor" => {
            "Usage:\n  podway doctor [--deep]\n\nExample:\n  podway doctor --deep"
        }
        "workspace.show" => "Usage:\n  podway workspace show\n\nExample:\n  podway workspace show",
        "workspace.repair" => {
            "Usage:\n  podway workspace repair\n\nExample:\n  podway workspace repair"
        }
        "session.start" => {
            "Usage:\n  podway start (--preset <name> | --procedure <file>) --task <title> [--dry-run]\n\nExamples:\n  podway start --preset sw-dev --task 'implement feature'\n  podway start --preset sw-dev --task 'preview procedure' --dry-run"
        }
        "session.start_replace" => {
            "Usage:\n  podway start (--preset <name> | --procedure <file>) --task <title> --replace [--dry-run] [--yes]\n\nExamples:\n  podway start --preset sw-dev --task 'replace task' --replace --yes\n  podway start --preset sw-dev --task 'preview replacement' --replace --dry-run"
        }
        "session.status" => {
            "Usage:\n  podway status [--verbose] [--wait-for-idle | --after-job <uuid>]\n\nExample:\n  podway status --verbose"
        }
        "session.next" => {
            "Usage:\n  podway next [--wait-for-idle | --after-job <uuid>]\n\nExample:\n  podway next"
        }
        "session.complete" => {
            "Usage:\n  podway complete [--if-session-revision <n>] [--if-attempt <uuid>]\n\nExample:\n  podway complete --if-session-revision 12 --if-attempt <uuid>"
        }
        "session.skip" => {
            "Usage:\n  podway skip [--reason <text>]\n\nExample:\n  podway skip --reason 'not applicable'"
        }
        "session.retry" => {
            "Usage:\n  podway retry --reason <text>\n\nExample:\n  podway retry --reason 'rerun after fixing input'"
        }
        "session.return" => {
            "Usage:\n  podway return --to <stage-id> --reason <text> [--dry-run]\n\nExample:\n  podway return --to implement --reason 'review found a gap' --dry-run"
        }
        "session.block" => {
            "Usage:\n  podway block --reason <text>\n\nExample:\n  podway block --reason 'waiting for API owner'"
        }
        "session.unblock" => {
            "Usage:\n  podway unblock (<blocker-id> | --all)\n\nExample:\n  podway unblock --all"
        }
        "session.cancel" => {
            "Usage:\n  podway cancel --reason <text>\n\nExample:\n  podway cancel --reason 'task no longer needed'"
        }
        "session.reopen" => {
            "Usage:\n  podway reopen --to <stage-id> --reason <text> [--dry-run]\n\nExample:\n  podway reopen --to implement --reason 'follow-up' --dry-run"
        }
        "session.reset" => {
            "Usage:\n  podway reset [--dry-run] [--yes]\n\nExample:\n  podway reset --yes"
        }
        "workspace.reset_all" => {
            "Usage:\n  podway reset --all --force --yes\n\nExample:\n  podway reset --all --force --yes"
        }
        "item.check" => "Usage:\n  podway check <item-id>\n\nExample:\n  podway check reproduced",
        "item.uncheck" => {
            "Usage:\n  podway uncheck <item-id>\n\nExample:\n  podway uncheck reproduced"
        }
        "item.set" => {
            "Usage:\n  podway set <item-id> (<value> | --stdin)\n\nExample:\n  podway set implementation-summary 'completed work'"
        }
        "item.add" => {
            "Usage:\n  podway add <item-id> <value>\n\nExample:\n  podway add affected-components daemon"
        }
        "item.remove" => {
            "Usage:\n  podway remove <item-id> <value> [--ignore-missing]\n\nExample:\n  podway remove affected-components daemon"
        }
        "item.attach" => {
            "Usage:\n  podway attach <item-id> (<path> [--media-type <type>] | --reference <ref> --digest <sha256> --size <bytes> --media-type <type>)\n\nExample:\n  podway attach report report.md --media-type text/markdown"
        }
        "item.clear" => "Usage:\n  podway clear <item-id>\n\nExample:\n  podway clear notes",
        "job.list" => {
            "Usage:\n  podway job list [--state <queued|running|succeeded|failed|cancelled>]\n\nExample:\n  podway job list --state queued"
        }
        "job.status" => {
            "Usage:\n  podway job status <job-id>\n\nExample:\n  podway job status <uuid>"
        }
        "job.wait" => "Usage:\n  podway job wait <job-id>\n\nExample:\n  podway job wait <uuid>",
        "job.cancel" => {
            "Usage:\n  podway job cancel <job-id>\n\nExample:\n  podway job cancel <uuid>"
        }
        _ => return Err(LocalFailure::request_invalid("unknown help topic")),
    };
    Ok(text.to_owned())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use super::{
        Cli, Command, LocalEnvelopeClock, LocalFailure, local_generated_at, local_result,
        parse_timeout_millis, render_local_failure_with_clock_and_writers,
        render_result_with_clock_and_writers,
    };
    use clap::Parser;
    use serde_json::json;

    #[test]
    fn parser_accepts_canonical_session_start_and_attachment_forms() {
        assert!(matches!(
            Cli::try_parse_from(["podway", "start", "--preset", "sw-dev", "--task", "task"])
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
    fn timeout_parser_accepts_documented_units_only() {
        assert_eq!(parse_timeout_millis("500ms"), Ok(500));
        assert_eq!(parse_timeout_millis("30s"), Ok(30_000));
        assert_eq!(parse_timeout_millis("2m"), Ok(120_000));
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
