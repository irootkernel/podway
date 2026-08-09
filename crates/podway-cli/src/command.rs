//! Public command-line surface for the Podway command contract.

mod completion;

use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, IsTerminal, Read, Seek, SeekFrom, Write},
    os::{
        fd::OwnedFd,
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt},
        },
    },
    path::{Component, Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use clap::{ArgAction, ArgMatches, Args, CommandFactory, Parser, Subcommand};
use nix::{
    errno::Errno,
    fcntl::{OFlag, open, openat, renameat},
    sys::stat::{Mode, fchmod, mode_t},
    unistd::{UnlinkatFlags, fsync, geteuid, unlinkat},
};
use podway_cli::client::{
    DEFAULT_DAEMON_CONNECT_TIMEOUT_V1, DEFAULT_DAEMON_WRITE_TIMEOUT_V1, DaemonClientErrorV1,
    DaemonClientTimeoutsV1, DaemonClientV1,
};
use podway_config::{
    AuthoringContext, AuthoringStage, ConfigError, ConvertedProcedureV2, FormatFailure,
    FormatRequest, FormattedProcedureV2, MAX_PROCEDURE_DOCUMENT_BYTES_V1, PROCEDURE_SCHEMA_V1,
    ParsedProcedure, ProcedureFormatV1, ProcedureWarningPolicyV1, ProcedureWarningV1,
    ScaffoldTemplate, check_procedure_v2, config_error_diagnostic, convert_procedure_v1_to_v2,
    finalize_diagnostics, format_procedure_v2, lint_procedure_v2, normalize_procedure_v2_graph,
    parse_procedure_document, parse_procedure_v1, preview_procedure_v2, project_procedure_v2_dot,
    project_procedure_v2_graph, project_procedure_v2_mermaid, project_procedure_v2_plantuml,
    scaffold_procedure_v2, sniff_procedure_schema, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{
    ActorAttributionV2, AttemptId, CriterionAssessmentReasonV2, CriterionId, GoalCriterionV2,
    GoalDefinitionV2, GoalRevisionNumberV2, GoalRevisionReasonV2, GoalStatementV2, GraphNodeId,
    ItemId, OptionId, PROCEDURE_SCHEMA_V2, ReasonV2, Revision, SessionId, Sha256Digest, UnixMillis,
    WorkspaceId,
};
use podway_presets::{PresetError, catalog_v1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, CompactStatusResultV1, IdempotencyKeyV1, JobOutputV1, JobStateV1,
    MAX_SLICE_ITEM_TEXT_SCALARS_V1, MAX_WAIT_TIMEOUT_MILLIS_V1, NextResultV1, OperationV1,
    OutputEnvelopeInputV1, OutputEnvelopeInputV2, OutputEnvelopeV1, OutputEnvelopeV2,
    PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
    ResponseEnvelopeV1, ResponseEnvelopeV2, Rfc3339MillisV1, StatusResultV1, WorkspaceContextV1,
    WorktreeSelectorWireV1, build_identity_v1, ensure_command_result_schema_v1,
    ensure_error_details_schema_v1, validate_command_result_v1, validate_command_result_v2,
};
use podway_service::{
    DaemonContractVerifierV1, InstallSpecV1, LaunchctlRunnerV1, LocalPlatformPathV1, LogQueryV1,
    MacosServiceCommandRunnerV1, SERVICE_METADATA_MAX_BYTES_V1, ServiceClockV1, ServiceErrorV1,
    ServiceFilesystemV1, ServiceLabelV1, ServiceLogStreamV1, ServiceManagerContractV1,
    ServiceManagerV1, ServiceOutcomeV1, ServicePathErrorV1, ServiceRuntimePathsV1, ServiceStatusV1,
    StdServiceFilesystemV1, SystemLaunchctlRunnerV1, UninstallOptionsV1,
    installed_socket_path_from_metadata_v1,
};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use completion::Shell;

const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;
const LOCAL_USAGE_EXIT: i32 = 2;
const LOCAL_DAEMON_EXIT: i32 = 3;
const LOCAL_CLIENT_EXIT: i32 = 6;
const MAX_SERVICE_LOG_READ_BYTES: u64 = 10 * 1024 * 1024;
const SERVICE_HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
const DAEMON_VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const DAEMON_VERSION_PROBE_OUTPUT_LIMIT: usize = 4 * 1024;
const DAEMON_VERSION_PROBE_POST_KILL_DRAIN: Duration = Duration::from_millis(100);

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
    /// Use the isolated contributor daemon and state tree.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    dev: bool,

    /// Emit exactly one versioned JSON object to stdout.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    json: bool,

    /// Target an explicit Git worktree.
    #[arg(long, global = true, value_name = "PATH")]
    worktree: Option<PathBuf>,

    /// Bound daemon connection or daemon-side waiting.
    #[arg(long, global = true, value_name = "DURATION", value_parser = parse_timeout_millis)]
    timeout: Option<u64>,

    /// Connect to an explicit daemon Unix socket.
    #[arg(long, global = true, value_name = "ABSOLUTE_PATH")]
    socket: Option<PathBuf>,

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

    /// Require an exact workspace identifier.
    #[arg(long, global = true, value_name = "UUID")]
    if_workspace_uuid: Option<String>,

    /// Require an exact session identifier.
    #[arg(long, global = true, value_name = "UUID")]
    if_session_id: Option<String>,

    /// Require an exact session revision.
    #[arg(long, global = true, value_name = "N")]
    if_session_revision: Option<u64>,

    /// Require an exact active attempt identifier.
    #[arg(long, global = true, value_name = "UUID")]
    if_attempt: Option<String>,

    /// Require an exact active item revision.
    #[arg(long, global = true, value_name = "N")]
    if_item_revision: Option<u64>,

    /// Require an exact current Procedure v2 goal revision.
    #[arg(long, global = true, value_name = "N", value_parser = parse_positive_u64)]
    if_goal_revision: Option<u64>,

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
    Version {
        /// Emit the complete build and contract identity.
        #[arg(long, action = ArgAction::SetTrue)]
        identity: bool,
    },
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
    /// Per-user LaunchAgent lifecycle commands executed directly through the service manager.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Orderly stop the isolated foreground dev daemon.
    Terminate,
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
    Status(StatusArgs),
    Next(ReadArgs),
    Complete,
    Decide(DecideArgs),
    Rework(ReworkArgs),
    Goal {
        #[command(subcommand)]
        command: GoalCommand,
    },
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
    #[arg(
        long,
        value_name = "SHA256",
        requires = "procedure",
        conflicts_with = "preset"
    )]
    expect_procedure_digest: Option<String>,
    #[arg(long, value_name = "TITLE")]
    task: String,
    #[arg(long, value_name = "TEXT", requires = "criterion")]
    goal: Option<String>,
    #[arg(long, value_name = "ID=STATEMENT", action = ArgAction::Append, requires = "goal")]
    criterion: Vec<String>,
    #[arg(long, value_name = "TEXT", requires = "goal")]
    actor: Option<String>,
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

#[derive(Debug, Args, Default)]
struct StatusArgs {
    #[command(flatten)]
    read: ReadArgs,
    #[arg(
        long,
        action = ArgAction::SetTrue,
        requires = "wait_for_idle",
        conflicts_with = "verbose"
    )]
    compact: bool,
}

#[derive(Debug, Args)]
struct DecideArgs {
    #[arg(long, value_name = "ID")]
    option: String,
    #[arg(long, value_name = "TEXT")]
    reason: String,
    #[arg(long, value_name = "TEXT")]
    actor: Option<String>,
}

#[derive(Debug, Args)]
struct ReworkArgs {
    #[arg(long, value_name = "GRAPH_NODE_ID")]
    to: String,
    #[arg(long, value_name = "TEXT")]
    reason: String,
    #[arg(long, value_name = "TEXT")]
    actor: Option<String>,
}

#[derive(Debug, Subcommand)]
enum GoalCommand {
    Define(GoalDefineArgs),
    Revise(GoalReviseArgs),
    AssessCriterion(GoalAssessCriterionArgs),
}

#[derive(Debug, Args)]
struct GoalDefineArgs {
    #[arg(long, value_name = "TEXT")]
    goal: String,
    #[arg(long, value_name = "ID=STATEMENT", action = ArgAction::Append, required = true)]
    criterion: Vec<String>,
    #[arg(long, value_name = "TEXT")]
    actor: Option<String>,
}

#[derive(Debug, Args)]
struct GoalReviseArgs {
    #[arg(long, value_name = "TEXT")]
    goal: String,
    #[arg(long, value_name = "ID=STATEMENT", action = ArgAction::Append, required = true)]
    criterion: Vec<String>,
    #[arg(long, value_name = "GRAPH_NODE_ID")]
    rework_to: String,
    #[arg(long, value_name = "TEXT")]
    reason: String,
    #[arg(long, value_name = "TEXT")]
    actor: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    reactivate: bool,
}

#[derive(Debug, Args)]
struct GoalAssessCriterionArgs {
    #[arg(value_name = "CRITERION_ID")]
    criterion_id: String,
    #[arg(long, value_name = "STATUS", value_parser = ["satisfied", "unsatisfied", "not_applicable"])]
    status: String,
    #[arg(long, value_name = "TEXT")]
    reason: String,
    #[arg(long, value_name = "GRAPH_NODE_ID", action = ArgAction::Append)]
    evidence: Vec<String>,
    #[arg(long, value_name = "ITEM_ID", action = ArgAction::Append)]
    item: Vec<String>,
    #[arg(long, value_name = "TEXT")]
    actor: Option<String>,
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
    Format {
        #[arg(value_name = "FILE")]
        file: PathBuf,
        #[arg(long, action = ArgAction::SetTrue, conflicts_with = "write")]
        check: bool,
        #[arg(long, action = ArgAction::SetTrue, conflicts_with = "check")]
        write: bool,
    },
    Vet {
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },
    Graph {
        #[arg(value_name = "FILE")]
        file: PathBuf,
        #[arg(long, required = true, value_parser = ["json", "mermaid", "puml", "dot"])]
        format: String,
    },
    Preview {
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },
    Lint {
        #[arg(value_name = "FILE")]
        file: PathBuf,
        #[arg(long, action = ArgAction::SetTrue)]
        warnings_as_errors: bool,
    },
    Check {
        #[arg(value_name = "FILE")]
        file: PathBuf,
        #[arg(long, action = ArgAction::SetTrue)]
        warnings_as_errors: bool,
    },
    Scaffold {
        /// The template to emit. The closed list is `ScaffoldTemplate::NAMES`, so an unknown value
        /// is a Clap parse failure — a usage error — rather than a runtime rejection this command
        /// would have to invent a result shape for.
        #[arg(long, default_value = "minimal", value_parser = ScaffoldTemplate::NAMES)]
        template: String,
    },
    Convert {
        #[arg(value_name = "FILE")]
        file: PathBuf,
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
        lines: Option<u16>,
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
    Lookup,
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
            Self::Decide(_) => Some("session.decide"),
            Self::Rework(_) => Some("session.rework"),
            Self::Goal {
                command: GoalCommand::Define(_),
            } => Some("goal.define"),
            Self::Goal {
                command: GoalCommand::Revise(_),
            } => Some("goal.revise"),
            Self::Goal {
                command: GoalCommand::AssessCriterion(_),
            } => Some("goal.assess_criterion"),
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
                command: JobCommand::Lookup,
            } => Some("job.lookup"),
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
            | Self::Version { .. }
            | Self::Completions { .. }
            | Self::Procedure { .. }
            | Self::Preset { .. }
            | Self::Daemon { .. }
            | Self::Terminate => None,
        }
    }

    fn canonical_route(&self) -> &'static str {
        match self {
            Self::Help { .. } => "help",
            Self::Version { .. } => "version",
            Self::Completions { .. } => "completions",
            Self::Procedure {
                command: ProcedureCommand::Validate { .. },
            } => "procedure.validate",
            Self::Procedure {
                command: ProcedureCommand::Show { .. },
            } => "procedure.show",
            Self::Procedure {
                command: ProcedureCommand::Format { .. },
            } => "procedure.format",
            Self::Procedure {
                command: ProcedureCommand::Vet { .. },
            } => "procedure.vet",
            Self::Procedure {
                command: ProcedureCommand::Graph { .. },
            } => "procedure.graph",
            Self::Procedure {
                command: ProcedureCommand::Preview { .. },
            } => "procedure.preview",
            Self::Procedure {
                command: ProcedureCommand::Lint { .. },
            } => "procedure.lint",
            Self::Procedure {
                command: ProcedureCommand::Check { .. },
            } => "procedure.check",
            Self::Procedure {
                command: ProcedureCommand::Scaffold { .. },
            } => "procedure.scaffold",
            Self::Procedure {
                command: ProcedureCommand::Convert { .. },
            } => "procedure.convert",
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
            Self::Terminate => "daemon.terminate",
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
            | Self::Decide(_)
            | Self::Rework(_)
            | Self::Goal { .. }
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
            | Self::Decide(_)
            | Self::Rework(_)
            | Self::Goal { .. }
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

    const fn accepts_workspace_identity(&self) -> bool {
        matches!(
            self,
            Self::Start(_)
                | Self::Status(_)
                | Self::Next(_)
                | Self::Complete
                | Self::Decide(_)
                | Self::Rework(_)
                | Self::Goal { .. }
                | Self::Skip { .. }
                | Self::Retry { .. }
                | Self::Return(_)
                | Self::Block { .. }
                | Self::Unblock { .. }
                | Self::Cancel { .. }
                | Self::Reopen(_)
                | Self::Reset(_)
                | Self::Check { .. }
                | Self::Uncheck { .. }
                | Self::Set(_)
                | Self::Add { .. }
                | Self::Remove { .. }
                | Self::Attach(_)
                | Self::Clear { .. }
        )
    }

    const fn accepts_session_identity(&self) -> bool {
        matches!(
            self,
            Self::Start(StartArgs { replace: true, .. })
                | Self::Status(_)
                | Self::Next(_)
                | Self::Complete
                | Self::Decide(_)
                | Self::Rework(_)
                | Self::Goal { .. }
                | Self::Skip { .. }
                | Self::Retry { .. }
                | Self::Return(_)
                | Self::Block { .. }
                | Self::Unblock { .. }
                | Self::Cancel { .. }
                | Self::Reopen(_)
                | Self::Reset(ResetArgs { all: false, .. })
                | Self::Check { .. }
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
    goal_revision: Option<GoalRevisionNumberV2>,
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

    fn from_output_v2(output: &OutputEnvelopeV2) -> Result<Self, LocalFailure> {
        let result = output.result();
        if result.get("schema").and_then(Value::as_str) != Some("podway.status-result/v2") {
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
        let transport_workspace_id = output
            .workspace()
            .map(|workspace| workspace.uuid().clone())
            .ok_or_else(|| {
                LocalFailure::response_invalid("status response omitted workspace identity")
            })?;
        Ok(Self {
            transport_workspace_id,
            facts: StatusFacts {
                session_id,
                session_revision,
                attempt_id,
                item_revisions,
                goal_revision,
            },
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
            goal_revision: None,
        }
    }

    fn preconditions(
        &self,
        command: &Command,
        explicit: &ExplicitPreconditions,
    ) -> Result<PreconditionsV1, LocalFailure> {
        let session_id = explicit
            .session_id
            .clone()
            .unwrap_or_else(|| self.session_id.clone());
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
                Some(session_id),
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
        if matches!(command, Command::Decide(_)) {
            return self.v2_preconditions(session_id, session_revision, attempt_id, true, None);
        }
        if matches!(command, Command::Rework(_)) {
            return self.v2_preconditions(session_id, session_revision, attempt_id, false, None);
        }
        if matches!(
            command,
            Command::Goal {
                command: GoalCommand::Define(_)
            }
        ) {
            return self.v2_preconditions(session_id, session_revision, None, false, None);
        }
        if matches!(
            command,
            Command::Goal {
                command: GoalCommand::Revise(_)
            }
        ) {
            return self.v2_preconditions(
                session_id,
                session_revision,
                attempt_id,
                false,
                Some(
                    explicit
                        .goal_revision
                        .or(self.goal_revision)
                        .ok_or_else(|| {
                            LocalFailure::response_invalid(
                                "status response omitted the goal revision",
                            )
                        })?,
                ),
            );
        }
        if matches!(
            command,
            Command::Goal {
                command: GoalCommand::AssessCriterion(_)
            }
        ) {
            return self.v2_preconditions(
                session_id,
                session_revision,
                attempt_id,
                true,
                Some(
                    explicit
                        .goal_revision
                        .or(self.goal_revision)
                        .ok_or_else(|| {
                            LocalFailure::response_invalid(
                                "status response omitted the goal revision",
                            )
                        })?,
                ),
            );
        }
        if matches!(command, Command::Start(StartArgs { replace: true, .. })) {
            return PreconditionsV1::new(
                Some(session_id),
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
                Some(session_id),
                Some(session_revision),
                None,
                None,
                None,
                None,
            )
            .map_err(|_| LocalFailure::response_invalid("reset preconditions are invalid"));
        }
        if matches!(command, Command::Reopen(_)) {
            return PreconditionsV1::new(
                Some(session_id),
                Some(session_revision),
                None,
                None,
                None,
                None,
            )
            .map_err(|_| LocalFailure::response_invalid("reopen preconditions are invalid"));
        }
        PreconditionsV1::new(
            Some(session_id),
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

    fn v2_preconditions(
        &self,
        session_id: SessionId,
        session_revision: Revision,
        attempt_id: Option<AttemptId>,
        require_attempt: bool,
        goal_revision: Option<GoalRevisionNumberV2>,
    ) -> Result<PreconditionsV1, LocalFailure> {
        let attempt_id = if require_attempt {
            Some(attempt_id.ok_or_else(|| {
                LocalFailure::response_invalid("status response omitted the active attempt")
            })?)
        } else {
            attempt_id
        };
        let mut preconditions = PreconditionsV1::new(
            Some(session_id),
            Some(session_revision),
            attempt_id,
            None,
            None,
            None,
        )
        .map_err(|_| LocalFailure::response_invalid("Procedure v2 preconditions are invalid"))?;
        if let Some(goal_revision) = goal_revision {
            preconditions = preconditions
                .with_goal_revision(goal_revision)
                .map_err(|_| LocalFailure::response_invalid("goal revision is invalid"))?;
        }
        Ok(preconditions)
    }
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

    const fn any(&self) -> bool {
        self.workspace_id.is_some()
            || self.session_id.is_some()
            || self.session_revision.is_some()
            || self.attempt_id.is_some()
            || self.item_revision.is_some()
            || self.goal_revision.is_some()
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
            "REQUEST_INVALID"
            | "REQUEST_TOO_LARGE"
            | "SOCKET_ENDPOINT_INVALID"
            | "CONFIRMATION_REQUIRED" => (LOCAL_USAGE_EXIT, false),
            "DAEMON_NOT_INSTALLED" | "DAEMON_VERSION_INCOMPATIBLE" | "DAEMON_CONTRACT_MISMATCH" => {
                (LOCAL_DAEMON_EXIT, false)
            }
            "DAEMON_UNAVAILABLE" => (LOCAL_DAEMON_EXIT, true),
            // A registered v2 route this build does not serve. No local command produces it now
            // that `procedure format --write` is implemented; the entry stays because this match is
            // the CLI's copy of the frozen exit classes in `assets/specifications/error-codes.json`,
            // not a list of the failures it happens to raise today.
            "UNSUPPORTED_V2_CAPABILITY" => (LOCAL_DAEMON_EXIT, false),
            "MUTATION_OUTCOME_UNKNOWN" => (4, true),
            "PRESET_NOT_FOUND"
            | "PROCEDURE_NOT_FOUND"
            | "PROCEDURE_INVALID"
            | "PROCEDURE_SCHEMA_UNSUPPORTED" => (1, false),
            "PROCEDURE_DIGEST_MISMATCH" => (4, false),
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
            details: Map::new(),
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

    fn response_invalid(message: impl Into<String>) -> Self {
        Self::catalog("INTERNAL_ERROR", message, "cli")
    }

    fn mutation_outcome_unknown(idempotency_key: &IdempotencyKeyV1) -> Self {
        let mut failure = Self::catalog(
            "MUTATION_OUTCOME_UNKNOWN",
            "mutation outcome is unknown; reconcile by idempotency key",
            "cli",
        );
        failure.details = serde_json::from_value(json!({
            "schema": "podway.mutation-outcome-unknown-details/v1",
            "outcome": "unknown",
            "idempotency_key": idempotency_key.as_str(),
            "reconcile": {
                "command": "job.lookup",
                "idempotency_key": idempotency_key.as_str(),
            },
        }))
        .expect("static mutation outcome details must be an object");
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
                Value::String("podway.procedure-digest-mismatch-details/v1".to_owned()),
            ),
            (
                "expected_procedure_digest".to_owned(),
                Value::String(expected.as_str().to_owned()),
            ),
            (
                "actual_procedure_digest".to_owned(),
                Value::String(actual.as_str().to_owned()),
            ),
            ("admission".to_owned(), json!({"admitted": false})),
        ]);
        failure
    }

    fn with_command(mut self, command: &str) -> Self {
        self.command = command.to_owned();
        self
    }

    fn with_not_admitted_if(mut self, mutation: bool) -> Self {
        if mutation && self.code != "MUTATION_OUTCOME_UNKNOWN" {
            self.details
                .insert("admission".to_owned(), json!({"admitted": false}));
        }
        self
    }

    fn with_correlation(mut self, command: &str, request_id: &str) -> Self {
        self.command = command.to_owned();
        self.request_id = Some(request_id.to_owned());
        self
    }

    fn with_details(mut self, details: Map<String, Value>) -> Self {
        self.details = details;
        self
    }
}

enum RunResult {
    Response(Box<ResponseEnvelopeV1>),
    ResponseV2(Box<ResponseEnvelopeV2>),
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
    LogFollow {
        path: PathBuf,
        initial: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ParseFailureCommandContext {
    route: &'static str,
    mutation: bool,
}

impl ParseFailureCommandContext {
    const fn new(route: &'static str, mutation: bool) -> Self {
        Self { route, mutation }
    }
}

fn parse_failure_command_context(arguments: &[OsString]) -> Option<ParseFailureCommandContext> {
    let matches = Cli::command()
        .ignore_errors(true)
        .try_get_matches_from(arguments)
        .ok()?;
    parse_failure_command_context_from_matches(&matches)
}

fn parse_failure_command_context_from_matches(
    matches: &ArgMatches,
) -> Option<ParseFailureCommandContext> {
    let (command, matches) = matches.subcommand()?;
    let context = match command {
        "help" => ParseFailureCommandContext::new("help", false),
        "version" => ParseFailureCommandContext::new("version", false),
        "completions" => ParseFailureCommandContext::new("completions", false),
        "procedure" => nested_parse_failure_context(
            matches,
            &[
                ("validate", "procedure.validate"),
                ("show", "procedure.show"),
                ("format", "procedure.format"),
                ("vet", "procedure.vet"),
                ("graph", "procedure.graph"),
                ("preview", "procedure.preview"),
                ("lint", "procedure.lint"),
                // The nested table maps `podway procedure <word>`; the bare `check` word is the
                // top-level `item.check` arm below and is deliberately left alone.
                ("check", "procedure.check"),
                ("scaffold", "procedure.scaffold"),
                ("convert", "procedure.convert"),
            ],
        )?,
        "preset" => nested_parse_failure_context(
            matches,
            &[
                ("list", "preset.list"),
                ("show", "preset.show"),
                ("explain", "preset.explain"),
            ],
        )?,
        "daemon" => nested_parse_failure_context(
            matches,
            &[
                ("install", "daemon.install"),
                ("uninstall", "daemon.uninstall"),
                ("start", "daemon.start"),
                ("stop", "daemon.stop"),
                ("restart", "daemon.restart"),
                ("status", "daemon.status"),
                ("logs", "daemon.logs"),
            ],
        )?,
        "init" => ParseFailureCommandContext::new("workspace.init", true),
        "doctor" => ParseFailureCommandContext::new("workspace.doctor", false),
        "workspace" => nested_parse_failure_context(
            matches,
            &[("show", "workspace.show"), ("repair", "workspace.repair")],
        )?,
        "start" => ParseFailureCommandContext::new(
            if matches.get_flag("replace") {
                "session.start_replace"
            } else {
                "session.start"
            },
            !matches.get_flag("dry_run"),
        ),
        "status" => ParseFailureCommandContext::new("session.status", false),
        "next" => ParseFailureCommandContext::new("session.next", false),
        "complete" => ParseFailureCommandContext::new("session.complete", true),
        "decide" => ParseFailureCommandContext::new("session.decide", true),
        "rework" => ParseFailureCommandContext::new("session.rework", true),
        "goal" => {
            let route = nested_parse_failure_context(
                matches,
                &[
                    ("define", "goal.define"),
                    ("revise", "goal.revise"),
                    ("assess-criterion", "goal.assess_criterion"),
                ],
            )?
            .route;
            ParseFailureCommandContext::new(route, true)
        }
        "skip" => ParseFailureCommandContext::new("session.skip", true),
        "retry" => ParseFailureCommandContext::new("session.retry", true),
        "return" => ParseFailureCommandContext::new("session.return", !matches.get_flag("dry_run")),
        "block" => ParseFailureCommandContext::new("session.block", true),
        "unblock" => ParseFailureCommandContext::new("session.unblock", true),
        "cancel" => ParseFailureCommandContext::new("session.cancel", true),
        "reopen" => ParseFailureCommandContext::new("session.reopen", !matches.get_flag("dry_run")),
        "reset" => ParseFailureCommandContext::new(
            if matches.get_flag("all") {
                "workspace.reset_all"
            } else {
                "session.reset"
            },
            !matches.get_flag("dry_run"),
        ),
        "check" => ParseFailureCommandContext::new("item.check", true),
        "uncheck" => ParseFailureCommandContext::new("item.uncheck", true),
        "set" => ParseFailureCommandContext::new("item.set", true),
        "add" => ParseFailureCommandContext::new("item.add", true),
        "remove" => ParseFailureCommandContext::new("item.remove", true),
        "attach" => ParseFailureCommandContext::new("item.attach", true),
        "clear" => ParseFailureCommandContext::new("item.clear", true),
        "job" => nested_parse_failure_context(
            matches,
            &[
                ("list", "job.list"),
                ("lookup", "job.lookup"),
                ("status", "job.status"),
                ("wait", "job.wait"),
                ("cancel", "job.cancel"),
            ],
        )?,
        "__complete" => ParseFailureCommandContext::new("__complete", false),
        _ => return None,
    };
    Some(context)
}

fn nested_parse_failure_context(
    matches: &ArgMatches,
    routes: &[(&str, &'static str)],
) -> Option<ParseFailureCommandContext> {
    let (command, _) = matches.subcommand()?;
    routes
        .iter()
        .find(|(candidate, _)| *candidate == command)
        .map(|(_, route)| ParseFailureCommandContext::new(route, false))
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

    let arguments: Vec<OsString> = env::args_os().collect();
    let json_requested = arguments
        .iter()
        .any(|argument| argument.as_os_str() == OsStr::new("--json"));
    match Cli::try_parse_from(&arguments) {
        Ok(cli) => {
            let route = cli.command.canonical_route();
            let mutation = cli.command.is_mutation();
            let json_output = cli.json;
            let quiet = cli.quiet;
            match execute(cli) {
                Ok(result) => render_result(&result, json_output, quiet),
                Err(failure) => render_local_failure(
                    failure.with_command(route).with_not_admitted_if(mutation),
                    json_output,
                ),
            }
        }
        Err(_) => {
            let failure = LocalFailure::request_invalid("invalid command syntax");
            let failure = match parse_failure_command_context(&arguments) {
                Some(context) => failure
                    .with_command(context.route)
                    .with_not_admitted_if(context.mutation),
                None => failure,
            };
            render_local_failure(failure, json_requested)
        }
    }
}

fn execute(mut cli: Cli) -> Result<RunResult, LocalFailure> {
    if let Command::CompleteDynamic { kind } = &cli.command {
        let kind = kind.clone();
        return dynamic_completion(cli.worktree.take(), cli.socket.take(), cli.dev, &kind);
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
    if cli.if_workspace_uuid.is_none()
        && matches!(
            &cli.command,
            Command::Start(StartArgs {
                dry_run: true,
                replace: false,
                goal: None,
                ..
            })
        )
    {
        return execute_start_dry_run(&cli);
    }
    confirm_if_required(&cli, wire_name)?;

    let target = workspace_target(cli.worktree.take())?;
    let wait_timeout_ms = cli.timeout.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
    let explicit = ExplicitPreconditions::parse(&cli)?;
    let client = daemon_client(wait_timeout_ms, cli.socket.as_deref(), cli.dev)
        .map_err(|failure| failure.with_command(wire_name))?;

    let reset_all_workspace_id =
        if matches!(&cli.command, Command::Reset(ResetArgs { all: true, .. })) {
            let status_request = build_request(
                "session.status",
                &target,
                identity_probe_spec(wait_timeout_ms, &explicit)?,
            )?;
            match request_daemon(&client, &status_request)
                .map_err(|failure| failure.with_command(wire_name))?
            {
                ResponseEnvelopeV2::OutputV1(status) => Some(
                    explicit.workspace_id.clone().unwrap_or(
                        StatusPreflight::from_output(&status)
                            .map_err(|failure| {
                                failure.with_correlation(wire_name, status.request_id().as_str())
                            })?
                            .transport_workspace_id,
                    ),
                ),
                ResponseEnvelopeV2::OutputV2(status) => Some(
                    explicit.workspace_id.clone().unwrap_or(
                        StatusPreflight::from_output_v2(&status)
                            .map_err(|failure| {
                                failure.with_correlation(wire_name, status.request_id().as_str())
                            })?
                            .transport_workspace_id,
                    ),
                ),
                ResponseEnvelopeV2::Error(error) if reset_probe_can_recover(&error) => {
                    explicit.workspace_id.clone()
                }
                ResponseEnvelopeV2::Error(error) => {
                    return re_correlate_preflight_error(&error, wire_name);
                }
            }
        } else {
            None
        };

    if cli.command.needs_preflight()
        && !fully_fenced_start_replace(&cli.command, &explicit)
        && !fully_fenced_v2_start_replace(&cli.command, &explicit)
        && !fully_fenced_v2_mutation(&cli.command, &explicit)
    {
        let status_request = build_request(
            "session.status",
            &target,
            identity_probe_spec(wait_timeout_ms, &explicit)?,
        )?;
        let status_response = request_daemon(&client, &status_request)
            .map_err(|failure| failure.with_command(wire_name))?;
        let preflight = match status_response {
            ResponseEnvelopeV2::OutputV1(status) => StatusPreflight::from_output(&status),
            ResponseEnvelopeV2::OutputV2(status) => StatusPreflight::from_output_v2(&status),
            ResponseEnvelopeV2::Error(error) => {
                return re_correlate_preflight_error(&error, wire_name);
            }
        }
        .map_err(|failure| failure.with_command(wire_name))?;
        let (operation, payload) =
            daemon_payload(&mut cli.command, cli.idempotency_key.as_deref())?;
        let preconditions = preflight
            .facts
            .preconditions(&cli.command, &explicit)
            .map_err(|failure| failure.with_command(wire_name))?;
        let expected_workspace_id = explicit
            .workspace_id
            .clone()
            .unwrap_or(preflight.transport_workspace_id);
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
            .map(|response| RunResult::ResponseV2(Box::new(response)));
    }

    let (operation, mut payload) =
        daemon_payload(&mut cli.command, cli.idempotency_key.as_deref())?;
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
            expected_uuid: reset_all_workspace_id.or_else(|| explicit.workspace_id.clone()),
            idempotency_key: requires_idempotency_key(operation)
                .then(|| mutation_key(cli.idempotency_key))
                .transpose()?,
            preconditions: direct_preconditions(&cli.command, &explicit)?,
            detach: cli.detach,
            wait_timeout_ms,
            payload,
        },
    )?;
    request_daemon(&client, &request).map(|response| RunResult::ResponseV2(Box::new(response)))
}
fn requires_idempotency_key(operation: OperationV1) -> bool {
    matches!(operation, OperationV1::Mutate | OperationV1::Bootstrap)
}

fn fully_fenced_start_replace(command: &Command, explicit: &ExplicitPreconditions) -> bool {
    matches!(
        command,
        Command::Start(StartArgs {
            replace: true,
            dry_run: false,
            ..
        })
    ) && explicit.workspace_id.is_some()
        && explicit.session_id.is_some()
        && explicit.session_revision.is_some()
}

fn fully_fenced_v2_start_replace(command: &Command, explicit: &ExplicitPreconditions) -> bool {
    matches!(
        command,
        Command::Start(StartArgs {
            replace: true,
            goal: Some(_),
            ..
        })
    ) && explicit.workspace_id.is_some()
        && explicit.session_id.is_some()
        && explicit.session_revision.is_some()
}

fn fully_fenced_v2_mutation(command: &Command, explicit: &ExplicitPreconditions) -> bool {
    if explicit.workspace_id.is_none()
        || explicit.session_id.is_none()
        || explicit.session_revision.is_none()
    {
        return false;
    }
    match command {
        Command::Decide(_)
        | Command::Goal {
            command: GoalCommand::AssessCriterion(_),
        } => {
            explicit.attempt_id.is_some()
                && (!matches!(command, Command::Goal { .. }) || explicit.goal_revision.is_some())
        }
        Command::Rework(_)
        | Command::Goal {
            command: GoalCommand::Define(_),
        } => true,
        Command::Goal {
            command: GoalCommand::Revise(_),
        } => explicit.goal_revision.is_some(),
        _ => false,
    }
}

fn reset_probe_can_recover(error: &podway_protocol::ErrorEnvelopeV1) -> bool {
    matches!(
        error.code().as_str(),
        "WORKSPACE_STATE_UNREADABLE" | "WORKSPACE_SCHEMA_UNSUPPORTED"
    )
}

fn identity_probe_spec(
    wait_timeout_ms: u64,
    explicit: &ExplicitPreconditions,
) -> Result<RequestSpec, LocalFailure> {
    Ok(RequestSpec {
        operation: OperationV1::Query,
        expected_uuid: explicit.workspace_id.clone(),
        idempotency_key: None,
        preconditions: PreconditionsV1::new(
            explicit.session_id.clone(),
            None,
            None,
            None,
            None,
            None,
        )
        .map_err(|_| LocalFailure::request_invalid("session identity precondition is invalid"))?,
        detach: false,
        wait_timeout_ms,
        payload: Map::new(),
    })
}

fn direct_preconditions(
    command: &Command,
    explicit: &ExplicitPreconditions,
) -> Result<PreconditionsV1, LocalFailure> {
    match command {
        Command::Start(StartArgs {
            replace: true,
            dry_run: false,
            ..
        }) => PreconditionsV1::new(
            explicit.session_id.clone(),
            explicit.session_revision,
            None,
            None,
            None,
            None,
        )
        .map_err(|_| LocalFailure::request_invalid("start-replace preconditions are invalid")),
        Command::Status(_) | Command::Next(_) => {
            PreconditionsV1::new(explicit.session_id.clone(), None, None, None, None, None).map_err(
                |_| LocalFailure::request_invalid("session identity precondition is invalid"),
            )
        }
        Command::Decide(_) => v2_session_preconditions(explicit, true, false),
        Command::Rework(_) => v2_session_preconditions(explicit, false, false),
        Command::Goal {
            command: GoalCommand::Define(_),
        } => v2_session_preconditions(explicit, false, false),
        Command::Goal {
            command: GoalCommand::Revise(_),
        } => v2_session_preconditions(explicit, false, true),
        Command::Goal {
            command: GoalCommand::AssessCriterion(_),
        } => v2_session_preconditions(explicit, true, true),
        Command::Job {
            command: JobCommand::Cancel { .. },
        } => PreconditionsV1::new(None, None, None, None, None, Some(JobStateV1::Queued)).map_err(
            |_| LocalFailure::request_invalid("job cancellation preconditions are invalid"),
        ),
        _ => Ok(PreconditionsV1::default()),
    }
}

fn v2_session_preconditions(
    explicit: &ExplicitPreconditions,
    require_attempt: bool,
    require_goal_revision: bool,
) -> Result<PreconditionsV1, LocalFailure> {
    let session_id = explicit
        .session_id
        .clone()
        .ok_or_else(|| LocalFailure::request_invalid("--if-session-id is required"))?;
    let session_revision = explicit
        .session_revision
        .ok_or_else(|| LocalFailure::request_invalid("--if-session-revision is required"))?;
    let attempt_id = if require_attempt {
        Some(
            explicit
                .attempt_id
                .clone()
                .ok_or_else(|| LocalFailure::request_invalid("--if-attempt is required"))?,
        )
    } else {
        explicit.attempt_id.clone()
    };
    let mut preconditions = PreconditionsV1::new(
        Some(session_id),
        Some(session_revision),
        attempt_id,
        None,
        None,
        None,
    )
    .map_err(|_| LocalFailure::request_invalid("Procedure v2 preconditions are invalid"))?;
    if require_goal_revision {
        let goal_revision = explicit
            .goal_revision
            .ok_or_else(|| LocalFailure::request_invalid("--if-goal-revision is required"))?;
        preconditions = preconditions
            .with_goal_revision(goal_revision)
            .map_err(|_| LocalFailure::request_invalid("goal revision is invalid"))?;
    }
    Ok(preconditions)
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
        if let Some(expected) = args.expect_procedure_digest.as_deref() {
            let expected = Sha256Digest::new(expected.to_owned()).map_err(|_| {
                LocalFailure::request_invalid(
                    "expected procedure digest must be sha256:<lowercase-hex>",
                )
                .with_command("session.start")
            })?;
            if admitted.digest() != &expected {
                return Err(LocalFailure::procedure_digest_mismatch(
                    &expected,
                    admitted.digest(),
                    "session.start",
                ));
            }
        }
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
    open_offline_procedure(procedure).map(|opened| opened.bytes)
}

/// Opens a procedure named by an ordinary filesystem path, keeping everything a rewrite needs.
///
/// The path is split into a canonicalized parent and a leaf so the hardened descriptor walk applies
/// to a command-line path exactly as it applies to a worktree-relative one.
fn open_offline_procedure(procedure: &Path) -> Result<OpenedProcedure, LocalFailure> {
    let parent = procedure
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = procedure
        .file_name()
        .ok_or_else(|| LocalFailure::procedure_not_found("procedure file is not specified"))?;
    let root = fs::canonicalize(parent)
        .map_err(|_| LocalFailure::procedure_not_found("cannot read procedure file"))?;
    open_descriptor_relative_procedure(
        &root,
        Path::new(file_name),
        LocalFailure::procedure_not_found("cannot read procedure file"),
    )
}

/// A procedure file the hardened walk has opened and read, left addressable for a rewrite.
///
/// Holding the parent descriptor is what lets `format --write` replace the file without ever
/// re-resolving a name: every later operation is `*at`-relative to this one directory, so no
/// component of the path can be swapped between the read and the rename.
struct OpenedProcedure {
    /// The directory the leaf lives in, reached by opening every component `O_NOFOLLOW`.
    parent: OwnedFd,
    /// The final path component, exactly as the request spelled it.
    leaf: OsString,
    /// The document bytes.
    bytes: Vec<u8>,
    /// The leaf's permission bits, taken from the descriptor the bytes were read through — the mode
    /// therefore belongs to the content, with no second lookup for anything to race.
    mode: Mode,
}

fn read_descriptor_relative_procedure(
    root: &Path,
    procedure: &Path,
    root_failure: LocalFailure,
) -> Result<Vec<u8>, LocalFailure> {
    open_descriptor_relative_procedure(root, procedure, root_failure).map(|opened| opened.bytes)
}

/// Opens the directory holding a descriptor-relative procedure path and returns it with the leaf
/// name, so reading and rewriting share one component walk rather than two that can drift.
///
/// Every component is opened `O_CLOEXEC|O_DIRECTORY|O_NOFOLLOW|O_RDONLY`, only
/// [`Component::Normal`] components are accepted — `..`, a root, and a prefix are all
/// `PATH_OUTSIDE_WORKTREE` — and every open failure maps through [`procedure_open_failure`], which
/// turns the `ELOOP` of a symlinked component into the same path rejection.
fn open_descriptor_relative_parent(
    root: &Path,
    procedure: &Path,
    root_failure: LocalFailure,
) -> Result<(OwnedFd, OsString), LocalFailure> {
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
        if components.peek().is_none() {
            return Ok((directory, component.to_owned()));
        }
        directory = openat(
            &directory,
            component,
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map_err(procedure_open_failure)?;
    }
    Err(LocalFailure::procedure_not_found(
        "procedure file is not specified",
    ))
}

/// Opens, size-bounds, and reads a descriptor-relative procedure file.
///
/// The leaf is opened `O_NOFOLLOW`, so a symlink named as the procedure is refused rather than
/// followed, and the descriptor the bytes come from is the descriptor its permission bits come
/// from.
fn open_descriptor_relative_procedure(
    root: &Path,
    procedure: &Path,
    root_failure: LocalFailure,
) -> Result<OpenedProcedure, LocalFailure> {
    let (parent, leaf) = open_descriptor_relative_parent(root, procedure, root_failure)?;
    let descriptor = openat(
        &parent,
        leaf.as_os_str(),
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
    let mode = permission_bits(&metadata);
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_PROCEDURE_DOCUMENT_BYTES_V1 as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LocalFailure::procedure_not_found("cannot read procedure file"))?;
    if bytes.len() > MAX_PROCEDURE_DOCUMENT_BYTES_V1 {
        return Err(LocalFailure::procedure_invalid(
            "procedure exceeds the maximum document size",
        ));
    }
    Ok(OpenedProcedure {
        parent,
        leaf,
        bytes,
        mode,
    })
}

/// The twelve permission bits of a file, as the mode argument the `*at` calls take.
///
/// `mode_t` is 16 bits on some targets and 32 on others; masking first means the value always fits,
/// so the fallback is unreachable and only exists to keep the conversion total.
fn permission_bits(metadata: &fs::Metadata) -> Mode {
    Mode::from_bits_truncate(mode_t::try_from(metadata.mode() & 0o7777).unwrap_or(0o600))
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
        || cli.socket.is_some()
        || cli.detach
        || cli.idempotency_key.is_some()
        || cli.if_workspace_uuid.is_some()
        || cli.if_session_id.is_some()
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
        Command::Version { identity } => {
            reject_local_flags(local_flags, "version accepts no daemon-only flags")?;
            if !identity {
                return Ok(Some(RunResult::VersionSummary {
                    name: "podway",
                    version: format!("v{}", env!("CARGO_PKG_VERSION")),
                    text: format!("podway {}", env!("CARGO_PKG_VERSION")),
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
            if cli.dev {
                return Err(LocalFailure::request_invalid(
                    "--dev cannot be combined with daemon service lifecycle commands",
                ));
            }
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
            if cli.socket.is_some() && !matches!(command, DaemonCommand::Install { .. }) {
                return Err(LocalFailure::request_invalid(
                    "--socket applies only to daemon install or daemon-backed workflow commands",
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
            if matches!(command, DaemonCommand::Logs { follow: true, .. }) && cli.json {
                return Err(LocalFailure::request_invalid(
                    "--follow cannot be combined with --json",
                ));
            }
            Ok(Some(execute_service_lifecycle(
                command,
                cli.socket.as_deref(),
            )?))
        }
        Command::Terminate => {
            if !cli.dev {
                return Err(LocalFailure::request_invalid("terminate requires --dev"));
            }
            if cli.worktree.is_some()
                || cli.socket.is_some()
                || cli.detach
                || cli.idempotency_key.is_some()
                || ExplicitPreconditions::parse(cli)?.any()
                || cli.yes
            {
                return Err(LocalFailure::request_invalid(
                    "dev terminate accepts only --timeout, --json, --quiet, and --no-color",
                ));
            }
            Ok(Some(execute_dev_terminate(
                cli.timeout.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS),
            )?))
        }
        _ => Ok(None),
    }
}

fn execute_dev_terminate(wait_timeout_ms: u64) -> Result<RunResult, LocalFailure> {
    let paths = effective_dev_paths("daemon.terminate")?;
    let socket_path = paths.socket_path().as_path();
    if !socket_path.exists() {
        return Ok(dev_terminate_result());
    }
    let client = daemon_client(wait_timeout_ms, None, true)?;
    let request = build_daemon_terminate_request()?;
    let response = match client.daemon_terminate(&request) {
        Ok(response) => response,
        Err(DaemonClientErrorV1::Connection { source, .. })
            if matches!(
                source.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            remove_stale_dev_socket(socket_path)?;
            return Ok(dev_terminate_result());
        }
        Err(error) => {
            return Err(map_client_error_for_request(error, &request)
                .with_correlation("daemon.terminate", request.request_id().as_str()));
        }
    };
    if matches!(response, ResponseEnvelopeV1::Error(_)) {
        return Ok(RunResult::Response(Box::new(response)));
    }
    let deadline = Instant::now() + Duration::from_millis(wait_timeout_ms);
    while socket_path.exists() {
        if Instant::now() >= deadline {
            return Err(LocalFailure::daemon_unavailable("daemon.terminate"));
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(dev_terminate_result())
}

fn remove_stale_dev_socket(socket_path: &Path) -> Result<(), LocalFailure> {
    let metadata = match fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(LocalFailure::daemon_unavailable("daemon.terminate")),
    };
    if !metadata.file_type().is_socket() || metadata.uid() != geteuid().as_raw() {
        return Err(LocalFailure::request_invalid(
            "dev socket cleanup refused an unexpected filesystem entry",
        ));
    }
    fs::remove_file(socket_path).map_err(|_| LocalFailure::daemon_unavailable("daemon.terminate"))
}

fn dev_terminate_result() -> RunResult {
    local_result(
        "daemon.terminate",
        json!({
            "mode": "dev",
            "termination": "completed",
            "socket_cleanup": "removed",
        }),
        "dev daemon terminated".to_owned(),
    )
}

fn execute_service_lifecycle(
    command: &DaemonCommand,
    socket_path: Option<&Path>,
) -> Result<RunResult, LocalFailure> {
    let command_name = daemon_command_name(command);
    let mut paths = if socket_path.is_some() {
        effective_service_paths(command_name)?
    } else {
        service_runtime_paths(command_name)?
    };
    if let Some(socket_path) = socket_path {
        paths = paths
            .with_socket_path(socket_path)
            .map_err(|error| socket_path_failure(error).with_command(command_name))?;
    }
    let clock = system_service_clock(SystemTime::now(), command_name)?;
    let runner = MacosServiceCommandRunnerV1::new_with_contract_verifier(
        StdServiceFilesystemV1,
        system_launchctl_runner(),
        clock,
        geteuid().as_raw(),
        CliDaemonContractVerifierV1,
    )
    .map_err(|_| LocalFailure::daemon_unavailable(command_name))?;
    let manager = ServiceManagerV1::new(runner, clock, paths.clone());
    execute_service_lifecycle_with_manager(command, &manager, &paths)
}

fn execute_service_lifecycle_with_manager(
    command: &DaemonCommand,
    manager: &impl ServiceManagerContractV1,
    paths: &ServiceRuntimePathsV1,
) -> Result<RunResult, LocalFailure> {
    let command_name = daemon_command_name(command);
    let result = match command {
        DaemonCommand::Install { daemon_path } => {
            let binary = resolve_daemon_executable(daemon_path.as_deref(), command_name)?;
            let expected_binary = binary.as_path().to_path_buf();
            let identity = build_identity_v1();
            let spec = InstallSpecV1::new(
                binary,
                ServiceLabelV1::podwayd(),
                paths.clone(),
                identity.product(),
                identity.contract_manifest_digest(),
            );
            let outcome = manager
                .install(spec)
                .map_err(|error| map_service_error(error, command_name))?;
            if matches!(
                outcome,
                ServiceOutcomeV1::ChangedV1(_) | ServiceOutcomeV1::AlreadyInDesiredStateV1(_)
            ) {
                wait_for_verified_service(
                    paths,
                    expected_binary.as_path(),
                    command_name,
                    SERVICE_HEALTH_TIMEOUT,
                )?;
            }
            service_outcome_result(command_name, outcome)
        }
        DaemonCommand::Uninstall { purge_logs } => service_outcome_result(
            command_name,
            manager
                .uninstall_with_options(UninstallOptionsV1::new(*purge_logs))
                .map_err(|error| map_service_error(error, command_name))?,
        ),
        DaemonCommand::Start => service_outcome_result(
            command_name,
            manager
                .start()
                .map_err(|error| map_service_error(error, command_name))?,
        ),
        DaemonCommand::Stop => service_outcome_result(
            command_name,
            manager
                .stop()
                .map_err(|error| map_service_error(error, command_name))?,
        ),
        DaemonCommand::Restart => service_outcome_result(
            command_name,
            manager
                .restart()
                .map_err(|error| map_service_error(error, command_name))?,
        ),
        DaemonCommand::Status => service_status_result(
            command_name,
            manager
                .status()
                .map_err(|error| map_service_error(error, command_name))?,
            paths,
        )?,
        DaemonCommand::Logs { follow, lines } => {
            let query = LogQueryV1::new(ServiceLogStreamV1::DaemonV1)
                .with_follow(*follow)
                .with_lines(*lines);
            let location = manager
                .logs(query)
                .map_err(|error| map_service_error(error, command_name))?;
            service_logs_result(command_name, location, *follow, *lines)?
        }
    };
    Ok(result)
}

fn socket_path_failure(error: ServicePathErrorV1) -> LocalFailure {
    let (reason, message) = match error {
        ServicePathErrorV1::Empty { .. } => ("empty", "socket path must not be empty"),
        ServicePathErrorV1::Relative { .. } => ("relative", "socket path must be absolute"),
        ServicePathErrorV1::Unnormalized { .. } => (
            "unnormalized",
            "socket path must be normalized and contain valid path characters",
        ),
        ServicePathErrorV1::WorkspaceLocal { .. } => (
            "workspace_local",
            "socket path must not point into workspace-local Podway state",
        ),
        ServicePathErrorV1::SocketPathTooLong { .. } => (
            "path_too_long",
            "socket path exceeds the macOS Unix socket path limit",
        ),
        ServicePathErrorV1::EffectiveUserLookup { .. }
        | ServicePathErrorV1::EffectiveUserNotFound { .. }
        | ServicePathErrorV1::RootUser
        | ServicePathErrorV1::DevHomeConflictsProduction { .. } => (
            "effective_user_unavailable",
            "socket path could not be validated",
        ),
    };
    let mut failure = LocalFailure::catalog("SOCKET_ENDPOINT_INVALID", message, "cli");
    failure
        .details
        .insert("reason".to_owned(), Value::String(reason.to_owned()));
    failure
}

fn service_runtime_paths(command: &str) -> Result<ServiceRuntimePathsV1, LocalFailure> {
    let paths = effective_service_paths(command)?;
    resolve_installed_service_endpoint(paths, command)
}

fn effective_service_paths(command: &str) -> Result<ServiceRuntimePathsV1, LocalFailure> {
    #[cfg(debug_assertions)]
    if let Some(account_root) = env::var_os("PODWAY_TEST_ACCOUNT_ROOT") {
        return ServiceRuntimePathsV1::for_account_home(account_root, geteuid().as_raw())
            .map_err(|_| LocalFailure::daemon_unavailable(command));
    }
    ServiceRuntimePathsV1::for_effective_user()
        .map_err(|_| LocalFailure::daemon_unavailable(command))
}

fn effective_dev_paths(command: &str) -> Result<ServiceRuntimePathsV1, LocalFailure> {
    let dev_home = env::var_os("PODWAY_DEV_HOME").map(PathBuf::from);
    #[cfg(debug_assertions)]
    if let Some(account_root) = env::var_os("PODWAY_TEST_ACCOUNT_ROOT") {
        let account_root = PathBuf::from(account_root);
        let dev_home = dev_home.unwrap_or_else(|| account_root.join(".podway/dev"));
        return ServiceRuntimePathsV1::for_dev_home(account_root, dev_home, geteuid().as_raw())
            .map_err(|_| LocalFailure::daemon_unavailable(command));
    }
    ServiceRuntimePathsV1::for_effective_user_dev(dev_home.as_deref())
        .map_err(|_| LocalFailure::daemon_unavailable(command))
}

fn system_launchctl_runner() -> SystemLaunchctlRunnerV1 {
    #[cfg(debug_assertions)]
    if let Some(executable) = env::var_os("PODWAY_TEST_LAUNCHCTL") {
        return SystemLaunchctlRunnerV1::new(executable);
    }
    SystemLaunchctlRunnerV1::default()
}

fn resolve_installed_service_endpoint(
    paths: ServiceRuntimePathsV1,
    command: &str,
) -> Result<ServiceRuntimePathsV1, LocalFailure> {
    let metadata_path = paths.metadata_index_path().as_path();
    let metadata = match fs::symlink_metadata(metadata_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(paths),
        Err(_) => return Err(LocalFailure::daemon_unavailable(command)),
    };
    let parent = metadata_path
        .parent()
        .ok_or_else(|| LocalFailure::daemon_unavailable(command))?;
    let parent_metadata =
        fs::symlink_metadata(parent).map_err(|_| LocalFailure::daemon_unavailable(command))?;
    if parent_metadata.file_type().is_symlink()
        || !parent_metadata.is_dir()
        || parent_metadata.uid() != geteuid().as_raw()
        || parent_metadata.mode() & 0o777 != 0o700
        || metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
        || metadata.len() > SERVICE_METADATA_MAX_BYTES_V1 as u64
    {
        return Err(LocalFailure::daemon_unavailable(command));
    }
    let bytes = StdServiceFilesystemV1
        .read_file_bounded(metadata_path, SERVICE_METADATA_MAX_BYTES_V1)
        .map_err(|_| LocalFailure::daemon_unavailable(command))?;
    let socket_path = installed_socket_path_from_metadata_v1(&bytes)
        .map_err(|_| LocalFailure::daemon_unavailable(command))?;
    paths
        .with_socket_path(socket_path)
        .map_err(|_| LocalFailure::daemon_unavailable(command))
}

fn resolve_daemon_executable(
    daemon_path: Option<&Path>,
    command: &str,
) -> Result<LocalPlatformPathV1, LocalFailure> {
    if let Some(path) = daemon_path {
        return canonical_daemon_executable(path, command);
    }
    let current_exe = env::current_exe().map_err(|_| LocalFailure::daemon_unavailable(command))?;
    let search_path = env::var_os("PATH");
    resolve_implicit_daemon_executable_from(&current_exe, search_path.as_deref(), command)
}

fn resolve_implicit_daemon_executable_from(
    current_exe: &Path,
    search_path: Option<&OsStr>,
    command: &str,
) -> Result<LocalPlatformPathV1, LocalFailure> {
    let resolved_current_exe =
        fs::canonicalize(current_exe).map_err(|_| LocalFailure::daemon_unavailable(command))?;
    let sibling = resolved_current_exe.with_file_name("podwayd");
    if is_executable_file(&sibling) {
        return canonical_daemon_executable(&sibling, command);
    }

    let path = search_path.ok_or_else(|| LocalFailure::daemon_unavailable(command))?;
    for directory in env::split_paths(path).filter(|directory| directory.is_absolute()) {
        let candidate = directory.join("podwayd");
        if is_executable_file(&candidate) {
            return canonical_daemon_executable(&candidate, command);
        }
    }
    Err(LocalFailure::daemon_unavailable(command))
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.mode() & 0o111 != 0)
}

fn canonical_daemon_executable(
    path: &Path,
    command: &str,
) -> Result<LocalPlatformPathV1, LocalFailure> {
    if !path.is_absolute() {
        return Err(LocalFailure::request_invalid(
            "daemon executable path must be absolute and normalized",
        )
        .with_command(command));
    }
    let canonical =
        fs::canonicalize(path).map_err(|_| LocalFailure::daemon_unavailable(command))?;
    if !is_executable_file(&canonical) {
        return Err(LocalFailure::daemon_unavailable(command));
    }
    LocalPlatformPathV1::new(canonical).map_err(|_| {
        LocalFailure::request_invalid("daemon executable path must be absolute and normalized")
            .with_command(command)
    })
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct DaemonStaticIdentityV1 {
    product: String,
    version: String,
    target: String,
    build_identity: String,
    source_commit: Option<String>,
    contract_manifest_schema: String,
    contract_manifest_digest: String,
    protocol_versions: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default)]
struct CliDaemonContractVerifierV1;

impl DaemonContractVerifierV1 for CliDaemonContractVerifierV1 {
    fn verify(
        &self,
        binary: &Path,
        expected_product: &str,
        expected_manifest_digest: &str,
    ) -> Result<(), ServiceErrorV1> {
        let observed = probe_daemon_identity(binary).ok();
        let actual_product = observed.as_ref().map(|identity| identity.product.clone());
        let actual_manifest_digest = observed
            .as_ref()
            .map(|identity| identity.contract_manifest_digest.clone());
        if actual_product.as_deref() == Some(expected_product)
            && actual_manifest_digest.as_deref() == Some(expected_manifest_digest)
        {
            return Ok(());
        }
        Err(ServiceErrorV1::ContractMismatchV1 {
            expected_product: expected_product.to_owned(),
            actual_product,
            expected_manifest_digest: expected_manifest_digest.to_owned(),
            actual_manifest_digest,
        })
    }
}

fn probe_daemon_identity(binary: &Path) -> Result<DaemonStaticIdentityV1, ServiceErrorV1> {
    let runner = SystemLaunchctlRunnerV1::new(binary).with_bounds(
        DAEMON_VERSION_PROBE_TIMEOUT,
        DAEMON_VERSION_PROBE_OUTPUT_LIMIT,
        DAEMON_VERSION_PROBE_POST_KILL_DRAIN,
    );
    probe_daemon_identity_with_runner(&runner)
}

fn probe_daemon_identity_with_runner(
    runner: &impl LaunchctlRunnerV1,
) -> Result<DaemonStaticIdentityV1, ServiceErrorV1> {
    let output = runner.run(&[
        "version".to_owned(),
        "--json".to_owned(),
        "--identity".to_owned(),
    ])?;
    if output.exit_status != 0 || !output.stderr.is_empty() || output.stdout.contains('\u{fffd}') {
        return Err(ServiceErrorV1::IoV1 {
            operation: None,
            message: "daemon identity probe returned malformed output".to_owned(),
        });
    }
    let response = serde_json::from_str::<ResponseEnvelopeV1>(&output.stdout).map_err(|_| {
        ServiceErrorV1::IoV1 {
            operation: None,
            message: "daemon identity probe returned malformed output".to_owned(),
        }
    })?;
    let ResponseEnvelopeV1::Output(envelope) = response else {
        return Err(ServiceErrorV1::IoV1 {
            operation: None,
            message: "daemon identity probe returned malformed output".to_owned(),
        });
    };
    if envelope.command().as_str() != "version" {
        return Err(ServiceErrorV1::IoV1 {
            operation: None,
            message: "daemon identity probe returned malformed output".to_owned(),
        });
    }
    let object = envelope.result();
    let required_string = |field: &str| {
        object
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| ServiceErrorV1::IoV1 {
                operation: None,
                message: "daemon identity probe returned malformed output".to_owned(),
            })
    };
    let source_commit = match object.get("source_commit") {
        Some(Value::Null) | None => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => {
            return Err(ServiceErrorV1::IoV1 {
                operation: None,
                message: "daemon identity probe returned malformed output".to_owned(),
            });
        }
    };
    let protocol_versions = object
        .get("supported_ipc_ids")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .and_then(|values| {
            values
                .iter()
                .map(Value::as_str)
                .map(|value| value.map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| ServiceErrorV1::IoV1 {
            operation: None,
            message: "daemon identity probe returned malformed output".to_owned(),
        })?;
    Ok(DaemonStaticIdentityV1 {
        product: required_string("product")?,
        version: required_string("version")?,
        target: required_string("target")?,
        build_identity: required_string("build_identity")?,
        source_commit,
        contract_manifest_schema: required_string("contract_manifest_schema")?,
        contract_manifest_digest: required_string("contract_manifest_digest")?,
        protocol_versions,
    })
}

fn wait_for_verified_service(
    paths: &ServiceRuntimePathsV1,
    expected_binary: &Path,
    command: &str,
    timeout: Duration,
) -> Result<(), LocalFailure> {
    let deadline = Instant::now() + timeout;
    let client = DaemonClientV1::new(paths.clone());
    let mut last_identity_failure = None;
    while Instant::now() < deadline {
        let request = build_daemon_status_request()?;
        match client.daemon_status(&request) {
            Ok(ResponseEnvelopeV1::Output(output)) => {
                match validated_live_daemon_status(&output, None, paths, Some(expected_binary)) {
                    Ok(_) => return Ok(()),
                    Err(failure) => last_identity_failure = Some(failure),
                }
            }
            Ok(ResponseEnvelopeV1::Error(error))
                if error.code().as_str() == "DAEMON_CONTRACT_MISMATCH" =>
            {
                last_identity_failure = Some(
                    LocalFailure::catalog("DAEMON_CONTRACT_MISMATCH", error.message(), command)
                        .with_details(error.details().clone()),
                );
            }
            Ok(ResponseEnvelopeV1::Error(_)) => {
                last_identity_failure = Some(LocalFailure::response_invalid(
                    "daemon readiness returned an unexpected error",
                ));
            }
            Err(
                DaemonClientErrorV1::Connection { .. }
                | DaemonClientErrorV1::SocketConfiguration { .. }
                | DaemonClientErrorV1::Timeout { .. }
                | DaemonClientErrorV1::Framing { .. }
                | DaemonClientErrorV1::MissingResponse
                | DaemonClientErrorV1::ResponseDecoding { .. }
                | DaemonClientErrorV1::RequestPossiblyTransmitted { .. },
            ) => {}
            Err(error) => return Err(map_client_error(error).with_command(command)),
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(last_identity_failure.unwrap_or_else(|| LocalFailure::daemon_unavailable(command)))
}

fn service_outcome_result(command: &str, outcome: ServiceOutcomeV1) -> RunResult {
    let outcome = outcome.kind().as_str();
    local_result(
        command,
        json!({ "outcome": outcome }),
        outcome.replace('_', " "),
    )
}

fn service_status_result(
    command: &str,
    status: ServiceStatusV1,
    paths: &ServiceRuntimePathsV1,
) -> Result<RunResult, LocalFailure> {
    let (status, installed, loaded, metadata) = match status {
        ServiceStatusV1::NotInstalledV1(_) => ("not_installed", false, false, None),
        ServiceStatusV1::RunningV1(running) => {
            let _launchctl_pid = running.process_id();
            ("running", true, true, running.metadata().cloned())
        }
        ServiceStatusV1::StoppedV1(stopped) => {
            ("stopped", true, false, stopped.metadata().cloned())
        }
    };
    let static_identity = metadata
        .as_ref()
        .map(|metadata| probe_daemon_identity(metadata.daemon_binary()))
        .transpose()
        .map_err(|error| map_service_error(error, command))?;
    if let Some(actual) = static_identity.as_ref() {
        let expected = build_identity_v1();
        if actual.product != expected.product()
            || actual.contract_manifest_digest != expected.contract_manifest_digest()
        {
            return Err(map_service_error(
                ServiceErrorV1::ContractMismatchV1 {
                    expected_product: expected.product().to_owned(),
                    actual_product: Some(actual.product.clone()),
                    expected_manifest_digest: expected.contract_manifest_digest().to_owned(),
                    actual_manifest_digest: Some(actual.contract_manifest_digest.clone()),
                },
                command,
            ));
        }
    }
    let configured_socket_path =
        installed.then(|| paths.socket_path().as_path().display().to_string());
    let executable_path = metadata
        .as_ref()
        .map(|metadata| metadata.daemon_binary().display().to_string());
    let mut result = json!({
        "schema": "podway.daemon-status-result/v1",
        "status": status,
        "installed": installed,
        "loaded": loaded,
        "reachable": false,
        "product": static_identity.as_ref().map(|identity| identity.product.as_str()),
        "daemon_version": static_identity.as_ref().map(|identity| identity.version.as_str()),
        "target": static_identity.as_ref().map(|identity| identity.target.as_str()),
        "build_identity": static_identity.as_ref().map(|identity| identity.build_identity.as_str()),
        "source_commit": static_identity.as_ref().and_then(|identity| identity.source_commit.as_deref()),
        "contract_manifest_schema": static_identity.as_ref().map(|identity| identity.contract_manifest_schema.as_str()),
        "contract_manifest_digest": static_identity.as_ref().map(|identity| identity.contract_manifest_digest.as_str()),
        "protocol_versions": static_identity.as_ref().map(|identity| identity.protocol_versions.as_slice()).unwrap_or(&[]),
        "pid": Value::Null,
        "process_id": Value::Null,
        "executable_path": executable_path,
        "started_at": Value::Null,
        "uptime_ms": Value::Null,
        "socket_path": configured_socket_path,
        "configured_socket_path": configured_socket_path,
        "effective_socket_path": Value::Null,
        "registered_worktree_count": Value::Null,
        "active_scheduler_count": Value::Null,
        "queued_job_count": Value::Null,
        "running_job_count": Value::Null,
    })
    .as_object()
    .expect("daemon service status is an object")
    .clone();
    if loaded {
        let request = build_daemon_status_request()?;
        let client = DaemonClientV1::new(paths.clone());
        match client.daemon_status(&request) {
            Ok(ResponseEnvelopeV1::Error(error)) => {
                return Ok(RunResult::Response(Box::new(ResponseEnvelopeV1::Error(
                    error,
                ))));
            }
            Ok(ResponseEnvelopeV1::Output(output)) => {
                let live = validated_live_daemon_status(
                    &output,
                    static_identity.as_ref(),
                    paths,
                    metadata.as_ref().map(|metadata| metadata.daemon_binary()),
                )?;
                for (key, value) in live {
                    result.insert(key, value);
                }
                result.insert("reachable".to_owned(), Value::Bool(true));
            }
            Err(
                DaemonClientErrorV1::Connection { .. }
                | DaemonClientErrorV1::SocketConfiguration { .. }
                | DaemonClientErrorV1::Timeout { .. }
                | DaemonClientErrorV1::Framing { .. }
                | DaemonClientErrorV1::MissingResponse
                | DaemonClientErrorV1::ResponseDecoding { .. }
                | DaemonClientErrorV1::RequestPossiblyTransmitted { .. },
            ) => {}
            Err(error) => return Err(map_client_error(error).with_command(command)),
        }
    }
    Ok(local_result(
        command,
        Value::Object(result),
        status.replace('_', " "),
    ))
}

fn validated_live_daemon_status(
    output: &podway_protocol::OutputEnvelopeV1,
    installed: Option<&DaemonStaticIdentityV1>,
    paths: &ServiceRuntimePathsV1,
    expected_executable: Option<&Path>,
) -> Result<Map<String, Value>, LocalFailure> {
    if output.command().as_str() != "daemon.status" {
        return Err(LocalFailure::response_invalid(
            "daemon status response command is invalid",
        ));
    }
    let result = output.result();
    const LIVE_FIELDS: [&str; 16] = [
        "schema",
        "product",
        "daemon_version",
        "target",
        "build_identity",
        "source_commit",
        "contract_manifest_schema",
        "contract_manifest_digest",
        "protocol_versions",
        "pid",
        "process_id",
        "executable_path",
        "started_at",
        "uptime_ms",
        "configured_socket_path",
        "effective_socket_path",
    ];
    if result.len() != LIVE_FIELDS.len()
        || LIVE_FIELDS.iter().any(|field| !result.contains_key(*field))
        || result.get("schema").and_then(Value::as_str) != Some("podway.daemon-status-result/v1")
    {
        return Err(LocalFailure::response_invalid(
            "daemon status response schema is invalid",
        ));
    }
    let string = |field: &str| {
        result
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| LocalFailure::response_invalid("daemon status response is invalid"))
    };
    let product = string("product")?;
    let manifest = string("contract_manifest_digest")?;
    let local = build_identity_v1();
    let (expected_product, expected_manifest) = installed
        .map(|identity| {
            (
                identity.product.as_str(),
                identity.contract_manifest_digest.as_str(),
            )
        })
        .unwrap_or((local.product(), local.contract_manifest_digest()));
    if product != expected_product || manifest != expected_manifest {
        return Err(map_service_error(
            ServiceErrorV1::ContractMismatchV1 {
                expected_product: expected_product.to_owned(),
                actual_product: Some(product.to_owned()),
                expected_manifest_digest: expected_manifest.to_owned(),
                actual_manifest_digest: Some(manifest.to_owned()),
            },
            "daemon.status",
        ));
    }
    for field in [
        "daemon_version",
        "target",
        "build_identity",
        "contract_manifest_schema",
        "executable_path",
        "started_at",
        "configured_socket_path",
        "effective_socket_path",
    ] {
        string(field)?;
    }
    let process_id = string("process_id")?;
    let executable_path = string("executable_path")?;
    let started_at = string("started_at")?;
    let configured_socket_path = string("configured_socket_path")?;
    let effective_socket_path = string("effective_socket_path")?;
    let source_commit_valid = result["source_commit"].is_null()
        || result["source_commit"]
            .as_str()
            .is_some_and(|value| !value.is_empty());
    let protocols_valid = result
        .get("protocol_versions")
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values.len() == local.supported_ipc_ids().len()
                && values
                    .iter()
                    .zip(local.supported_ipc_ids())
                    .all(|(actual, expected)| actual.as_str() == Some(*expected))
        });
    let expected_socket = paths.socket_path().as_path();
    let installed_identity_matches = installed.is_none_or(|identity| {
        result["daemon_version"].as_str() == Some(identity.version.as_str())
            && result["target"].as_str() == Some(identity.target.as_str())
            && result["build_identity"].as_str() == Some(identity.build_identity.as_str())
            && result["source_commit"].as_str() == identity.source_commit.as_deref()
            && result["contract_manifest_schema"].as_str()
                == Some(identity.contract_manifest_schema.as_str())
            && result["protocol_versions"] == serde_json::json!(identity.protocol_versions)
    });
    if Uuid::parse_str(process_id).is_err()
        || !source_commit_valid
        || string("contract_manifest_schema")? != local.contract_manifest_schema()
        || result
            .get("pid")
            .and_then(Value::as_u64)
            .is_none_or(|pid| pid == 0 || pid > u64::from(u32::MAX))
        || result.get("uptime_ms").and_then(Value::as_u64).is_none()
        || !protocols_valid
        || !Path::new(executable_path).is_absolute()
        || expected_executable.is_some_and(|expected| Path::new(executable_path) != expected)
        || !installed_identity_matches
        || Rfc3339MillisV1::new(started_at).is_err()
        || Path::new(configured_socket_path) != expected_socket
        || Path::new(effective_socket_path) != expected_socket
    {
        return Err(LocalFailure::response_invalid(
            "daemon status response is invalid",
        ));
    }
    Ok(result.clone())
}

fn service_logs_result(
    command: &str,
    location: podway_service::LogLocationV1,
    follow: bool,
    lines: Option<u16>,
) -> Result<RunResult, LocalFailure> {
    let path = location.path().as_path().to_path_buf();
    let content = read_recent_log(&path, lines)?;
    if follow {
        Ok(RunResult::LogFollow {
            path,
            initial: content,
        })
    } else {
        let path = path.display().to_string();
        Ok(local_result(
            command,
            json!({ "path": path, "content": content }),
            format!("{path}\n{content}"),
        ))
    }
}

fn read_recent_log(path: &Path, lines: Option<u16>) -> Result<String, LocalFailure> {
    let mut file = File::open(path).map_err(|_| LocalFailure::daemon_unavailable("daemon.logs"))?;
    let mut content = String::new();
    Read::by_ref(&mut file)
        .take(MAX_SERVICE_LOG_READ_BYTES + 1)
        .read_to_string(&mut content)
        .map_err(|_| LocalFailure::daemon_unavailable("daemon.logs"))?;
    if content.len() as u64 > MAX_SERVICE_LOG_READ_BYTES {
        return Err(LocalFailure::daemon_unavailable("daemon.logs"));
    }
    Ok(match lines {
        Some(lines) => content
            .lines()
            .rev()
            .take(usize::from(lines))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n"),
        None => content,
    })
}

fn stream_log_follow(
    path: &Path,
    initial: &str,
    stdout: &mut dyn Write,
) -> Result<(), LocalFailure> {
    if !initial.is_empty() {
        stdout
            .write_all(initial.as_bytes())
            .and_then(|_| stdout.write_all(b"\n"))
            .and_then(|_| stdout.flush())
            .map_err(|_| render_write_failure())?;
    }
    let mut file = File::open(path).map_err(|_| LocalFailure::daemon_unavailable("daemon.logs"))?;
    let mut offset = file
        .seek(SeekFrom::End(0))
        .map_err(|_| LocalFailure::daemon_unavailable("daemon.logs"))?;
    loop {
        thread::sleep(Duration::from_millis(250));
        stream_log_follow_update(path, &mut file, &mut offset, stdout)?;
    }
}

fn stream_log_follow_update(
    path: &Path,
    file: &mut File,
    offset: &mut u64,
    stdout: &mut dyn Write,
) -> Result<(), LocalFailure> {
    let active_metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(LocalFailure::daemon_unavailable("daemon.logs")),
    };
    let file_metadata = file
        .metadata()
        .map_err(|_| LocalFailure::daemon_unavailable("daemon.logs"))?;
    if active_metadata.dev() != file_metadata.dev() || active_metadata.ino() != file_metadata.ino()
    {
        *file = File::open(path).map_err(|_| LocalFailure::daemon_unavailable("daemon.logs"))?;
        *offset = 0;
    }

    let length = file
        .metadata()
        .map_err(|_| LocalFailure::daemon_unavailable("daemon.logs"))?
        .len();
    if length < *offset {
        *offset = file
            .seek(SeekFrom::Start(0))
            .map_err(|_| LocalFailure::daemon_unavailable("daemon.logs"))?;
    }

    let mut buffer = [0_u8; 8192];
    while *offset < length {
        let read = file
            .read(&mut buffer)
            .map_err(|_| LocalFailure::daemon_unavailable("daemon.logs"))?;
        if read == 0 {
            break;
        }
        stdout
            .write_all(&buffer[..read])
            .and_then(|_| stdout.flush())
            .map_err(|_| render_write_failure())?;
        *offset += read as u64;
    }
    Ok(())
}

fn map_service_error(error: ServiceErrorV1, command: &str) -> LocalFailure {
    match error {
        ServiceErrorV1::ContractMismatchV1 {
            expected_product,
            actual_product,
            expected_manifest_digest,
            actual_manifest_digest,
        } => match (actual_product, actual_manifest_digest) {
            (Some(actual_product), Some(actual_manifest_digest)) => LocalFailure::catalog(
                "DAEMON_CONTRACT_MISMATCH",
                "CLI and daemon contract identities differ.",
                command,
            )
            .with_details(
                json!({
                    "expected": {
                        "product": expected_product,
                        "contract_manifest_digest": expected_manifest_digest,
                    },
                    "actual": {
                        "product": actual_product,
                        "contract_manifest_digest": actual_manifest_digest,
                    },
                    "admission": { "admitted": false },
                })
                .as_object()
                .expect("contract mismatch details are an object")
                .clone(),
            ),
            _ => LocalFailure::catalog(
                "DAEMON_VERSION_INCOMPATIBLE",
                "daemon identity probe returned malformed output",
                command,
            ),
        },
        ServiceErrorV1::InvalidExecutableV1 { message } => {
            LocalFailure::catalog("DAEMON_VERSION_INCOMPATIBLE", message, command)
        }
        ServiceErrorV1::InvalidMetadataV1 { .. }
        | ServiceErrorV1::PathSafetyV1(_)
        | ServiceErrorV1::LogUnavailableV1 { .. } => LocalFailure::catalog(
            "DAEMON_NOT_INSTALLED",
            "the daemon service is not installed",
            command,
        ),
        ServiceErrorV1::OperationFailureV1 { source, .. } => map_service_error(*source, command),
        ServiceErrorV1::IoV1 { .. }
        | ServiceErrorV1::LaunchctlFailureV1 { .. }
        | ServiceErrorV1::PermissionDeniedV1 { .. }
        | ServiceErrorV1::StaleOrUnexpectedProcessV1 { .. }
        | ServiceErrorV1::TimeoutV1 { .. }
        | ServiceErrorV1::OutputLimitExceededV1 { .. }
        | ServiceErrorV1::LaunchctlTimeoutV1 { .. } => LocalFailure::daemon_unavailable(command),
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
    let mut result = result
        .as_object()
        .cloned()
        .expect("local result is always an object");
    ensure_command_result_schema_v1(command, &mut result);
    validate_command_result_v1(command, &result)
        .expect("local command result must satisfy its closed protocol contract");
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
    validate_command_result_v2(command, &result)
        .expect("local v2 results must satisfy their closed contract");
    assert!(
        exit_code == 0 || exit_code == 1,
        "a v2 success envelope exits clean or domain, never usage or higher",
    );
    RunResult::LocalV2 {
        command: command.to_owned(),
        result,
        text,
        exit_code,
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
        } => (file, *warnings_as_errors, PROCEDURE_VALIDATE_COMMAND),
        ProcedureCommand::Show { file, .. } => (file, false, "procedure.show"),
        ProcedureCommand::Format { file, check, write } => {
            return execute_procedure_format(file, *check, *write);
        }
        ProcedureCommand::Vet { file } => {
            return execute_procedure_vet(file);
        }
        ProcedureCommand::Graph { file, format } => {
            return execute_procedure_graph(file, format);
        }
        ProcedureCommand::Preview { file } => {
            return execute_procedure_preview(file);
        }
        ProcedureCommand::Lint {
            file,
            warnings_as_errors,
        } => {
            return execute_procedure_lint(file, *warnings_as_errors);
        }
        ProcedureCommand::Check {
            file,
            warnings_as_errors,
        } => {
            return execute_procedure_check(file, *warnings_as_errors);
        }
        ProcedureCommand::Scaffold { template } => {
            return Ok(execute_procedure_scaffold(template));
        }
        ProcedureCommand::Convert { file } => {
            return execute_procedure_convert(file);
        }
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
    // Versioned dispatch for `procedure validate`, placed after the read, the UTF-8 check, and the
    // format decision the v1 path already made and before the v1 parser, so no step of the v1 path
    // moves. The sniff is decode-only and its only positive signal is a document that declares
    // Procedure v2; a v1 document, an unknown schema, and an undecodable one all fall through to
    // `parse_procedure_v1` below, which is why the v1 surface — success bytes and every failure
    // alike — is unchanged by this arm existing.
    if matches!(command, ProcedureCommand::Validate { .. })
        && sniff_procedure_schema(&bytes, format) == Some(PROCEDURE_SCHEMA_V2)
    {
        return Ok(execute_procedure_validate_v2(file, &bytes, format));
    }
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
        ProcedureCommand::Format { .. }
        | ProcedureCommand::Vet { .. }
        | ProcedureCommand::Graph { .. }
        | ProcedureCommand::Preview { .. }
        | ProcedureCommand::Lint { .. }
        | ProcedureCommand::Check { .. }
        | ProcedureCommand::Scaffold { .. }
        | ProcedureCommand::Convert { .. } => {
            unreachable!("the v2 authoring commands dispatch to their own execution paths")
        }
    };
    Ok(local_result(name, result, text))
}

/// The route every `procedure validate` result reports under, named once so the v2 result builder
/// cannot disagree with the route the v1 path reports under.
const PROCEDURE_VALIDATE_COMMAND: &str = "procedure.validate";

/// Validates a Procedure v2 document and reports one bounded diagnostics result.
///
/// Reached only when the schema sniff positively identified Procedure v2, so this never sees a v1
/// document and never has to choose an error surface for one.
///
/// Validate is a two-stage, single-error pipeline: parsing maps the document into the model and
/// validation resolves its closed reference set, and the first failure of either is the whole
/// report. The result therefore carries zero diagnostics or exactly one, and `valid` is the
/// difference between them. `digest` is present exactly when validation produced one, which is
/// exactly when the document is admissible.
///
/// `--warnings-as-errors` is accepted and does nothing here: every diagnostic validate can emit is
/// catalogued as an error, so there is no warning for the policy to promote. The advisory stages
/// that do emit warnings are `procedure lint` and `procedure check`.
fn execute_procedure_validate_v2(
    file: &Path,
    bytes: &[u8],
    format: ProcedureFormatV1,
) -> RunResult {
    let source = std::str::from_utf8(bytes)
        .expect("execute_procedure rejects a non-UTF-8 document before dispatching");
    let source_path = file.display().to_string();
    let context = AuthoringContext::new(&source_path, source, format);

    let admitted = match parse_procedure_document(bytes, format) {
        Ok(ParsedProcedure::V2(parsed)) => validate_procedure_v2(parsed),
        // Unreachable: the sniff and the dispatcher read the same decoded `schema`. Reported as a
        // schema violation rather than a panic, because a diagnostic path must not be able to abort
        // the process even on an impossible branch.
        Ok(ParsedProcedure::V1(_)) => Err(ConfigError::InvalidSchema {
            expected: PROCEDURE_SCHEMA_V2,
            actual: PROCEDURE_SCHEMA_V1.to_owned(),
        }),
        Err(error) => Err(error),
    };

    match admitted {
        Ok(validated) => {
            procedure_validate_v2_diagnostics(&source_path, Some(validated.digest()), Vec::new())
        }
        Err(error) => procedure_validate_v2_diagnostics(
            &source_path,
            None,
            vec![config_error_diagnostic(&error, &context)],
        ),
    }
}

/// The `procedure validate` result for a Procedure v2 document.
///
/// Exit code 0 when the document is admissible and 1 when it is not, which for this command is the
/// same thing as the presence of a diagnostic: validate emits nothing but errors.
fn procedure_validate_v2_diagnostics(
    source_path: &str,
    digest: Option<&Sha256Digest>,
    diagnostics: Vec<podway_core::AuthoringDiagnostic>,
) -> RunResult {
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (AuthoringStage::Validate, diagnostic))
            .collect(),
    );
    let text = match digest {
        Some(digest) if report.diagnostics().is_empty() => {
            format!("{source_path}: valid ({})\n", digest.as_str())
        }
        _ => render_authoring_diagnostics(report.diagnostics()),
    };
    let exit_code = i32::from(!report.valid());
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "validate",
        "procedure_schema": PROCEDURE_SCHEMA_V2,
        "file": source_path,
        "valid": report.valid(),
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.truncated(),
        "diagnostics_total": report.total(),
    }) else {
        unreachable!("the static diagnostics result is a JSON object");
    };
    if let Some(digest) = digest {
        result.insert(
            "digest".to_owned(),
            Value::String(digest.as_str().to_owned()),
        );
    }
    local_result_v2(PROCEDURE_VALIDATE_COMMAND, result, text, exit_code)
}

/// Renders a Procedure v2 source document in canonical authoring form.
///
/// The read is the same hardened descriptor-relative walk `procedure validate` uses, and it runs
/// before the unimplemented-flag rejections so a missing or unsafe path always reports the path
/// failure rather than a capability failure.
///
/// `--check` answers the same question the default mode answers and reports it differently: the
/// pipeline runs identically, and only the last step — comparing the rendering against the source —
/// decides between a source result that says "already canonical" and the single
/// `FORMAT_NOT_CANONICAL` finding. Nothing on either path touches the filesystem after the read, so
/// `--check` is observably non-writing rather than merely intended to be.
///
/// `--write` adds one step after that comparison and nothing before it. The read, the parse, the
/// validation, the construct scan, the emission, and the projection bound all run first, so every
/// way this command can refuse a document is a refusal that has already happened by the time any
/// filesystem mutation is possible: a rejected file is byte-identical afterwards and no temporary
/// file was ever created.
fn execute_procedure_format(
    file: &Path,
    check: bool,
    write: bool,
) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_FORMAT_COMMAND;

    let opened = open_offline_procedure(file).map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(&opened.bytes).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;

    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureFormatV1::Json
    } else {
        ProcedureFormatV1::Yaml
    };
    let source_path = file.display().to_string();
    match format_procedure_v2(FormatRequest {
        source,
        source_path: &source_path,
        format,
    }) {
        Ok(formatted) if check => {
            let context = AuthoringContext::new(&source_path, source, format);
            match formatted.drift_diagnostic(&context) {
                Some(diagnostic) => {
                    Ok(procedure_format_diagnostics(&source_path, vec![diagnostic]))
                }
                None => Ok(procedure_format_source(
                    &source_path,
                    &formatted,
                    "check",
                    canonical_form_summary(&source_path),
                )),
            }
        }
        Ok(formatted) if write => procedure_format_write(&source_path, &formatted, &opened),
        Ok(formatted) => {
            let document = formatted.document().to_owned();
            Ok(procedure_format_source(
                &source_path,
                &formatted,
                "stdout",
                document,
            ))
        }
        Err(FormatFailure::NotProcedureV2) => Err(LocalFailure::catalog(
            "PROCEDURE_SCHEMA_UNSUPPORTED",
            "procedure format requires a podway.procedure/v2 document; run podway procedure convert first",
            NAME,
        )),
        Err(FormatFailure::Diagnostics(diagnostics)) => {
            Ok(procedure_format_diagnostics(&source_path, diagnostics))
        }
    }
}

/// The route every `procedure format` result reports under, named once so the three result
/// builders below cannot disagree with the route the failure paths use.
const PROCEDURE_FORMAT_COMMAND: &str = "procedure.format";

/// How many numbered temporary names a rewrite tries after the unnumbered one.
///
/// The base name already contains the process id, so a collision means two rewrites of the same
/// file are in flight inside one process. Sixteen retries settle that and nothing else; an
/// unbounded search would turn a wedged directory into a hang.
const PROCEDURE_TEMP_NAME_RETRIES: u32 = 16;

/// The pinned one-line verdict for a source that is already in canonical authoring form.
///
/// `--check` and `--write` share it deliberately: a clean file gets the same answer whether or not
/// the caller was prepared to rewrite it, because in both cases nothing happened.
fn canonical_form_summary(source_path: &str) -> String {
    format!("{source_path} is in canonical authoring form\n")
}

/// Rewrites a drifted procedure file in canonical authoring form, or leaves a clean one alone.
///
/// The no-op is a real no-op: an already-canonical file is not rewritten with identical bytes, it
/// is not opened for writing, and no temporary file is created, so its modification time survives
/// and a build system watching the tree sees no work. `changed` reports which of the two happened.
fn procedure_format_write(
    source_path: &str,
    formatted: &FormattedProcedureV2,
    opened: &OpenedProcedure,
) -> Result<RunResult, LocalFailure> {
    if !formatted.changed() {
        return Ok(procedure_format_source(
            source_path,
            formatted,
            "write",
            canonical_form_summary(source_path),
        ));
    }
    replace_procedure_document(opened, formatted.document().as_bytes())?;
    Ok(procedure_format_source(
        source_path,
        formatted,
        "write",
        format!("{source_path} rewritten in canonical authoring form\n"),
    ))
}

/// Replaces one procedure file with `document`, atomically and in place.
///
/// The sequence is the standard durable replace, with every step named relative to the directory
/// descriptor the read already holds: stage the bytes in a sibling temporary file, `fchmod` it to
/// the original's permissions, flush it to the device, rename it over the target, then flush the
/// directory entry. A reader of the target therefore sees either the whole old document or the
/// whole new one, never a truncated file, and the rename can only ever affect the one name the
/// request already resolved.
///
/// Any failure before the rename removes the temporary file and leaves the original untouched.
fn replace_procedure_document(
    opened: &OpenedProcedure,
    document: &[u8],
) -> Result<(), LocalFailure> {
    let (name, file) = create_procedure_temp(opened)?;
    let staged = write_procedure_temp(file, opened.mode, document).and_then(|()| {
        renameat(
            &opened.parent,
            name.as_os_str(),
            &opened.parent,
            opened.leaf.as_os_str(),
        )
        .map_err(|_| procedure_write_failure())
    });
    if let Err(failure) = staged {
        // Best effort by definition: the write already failed, and a leftover temporary file is a
        // smaller problem than reporting a second failure about the cleanup of the first.
        let _ = unlinkat(&opened.parent, name.as_os_str(), UnlinkatFlags::NoRemoveDir);
        return Err(failure);
    }
    // The rename is only durable once the directory entry is. Reporting a failure here is honest
    // even though the content is already in place: re-running `--write` is idempotent and will
    // report the file as already canonical.
    fsync(&opened.parent).map_err(|_| procedure_write_failure())
}

/// Creates the sibling temporary file a rewrite stages into, and returns its name.
///
/// `O_EXCL` means an existing entry is never clobbered and `O_NOFOLLOW` means a symlink planted
/// under the temporary name is never followed, so this can only ever create a fresh regular file
/// next to the target. The leading `.` keeps a half-written document out of an ordinary listing.
fn create_procedure_temp(opened: &OpenedProcedure) -> Result<(OsString, fs::File), LocalFailure> {
    let mut base = OsString::from(".");
    base.push(&opened.leaf);
    base.push(format!(".{}.podway-tmp", std::process::id()));
    let numbered = (0..PROCEDURE_TEMP_NAME_RETRIES).map(|attempt| {
        let mut name = base.clone();
        name.push(format!(".{attempt}"));
        name
    });
    for name in std::iter::once(base.clone()).chain(numbered) {
        match openat(
            &opened.parent,
            name.as_os_str(),
            OFlag::O_CLOEXEC | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_WRONLY,
            opened.mode,
        ) {
            Ok(descriptor) => return Ok((name, fs::File::from(descriptor))),
            Err(Errno::EEXIST) => {}
            Err(_) => return Err(procedure_write_failure()),
        }
    }
    Err(procedure_write_failure())
}

/// Fills the staged temporary file and makes its bytes durable.
fn write_procedure_temp(
    mut file: fs::File,
    mode: Mode,
    document: &[u8],
) -> Result<(), LocalFailure> {
    // `openat`'s mode argument is filtered by the process umask; `fchmod` is not. Without this a
    // `0o664` procedure rewritten under a `0o022` umask would come back `0o644`.
    fchmod(&file, mode).map_err(|_| procedure_write_failure())?;
    file.write_all(document)
        .map_err(|_| procedure_write_failure())?;
    // Flushing before the rename is what makes the replace atomic against a crash rather than only
    // against a concurrent reader: the name never points at bytes that are not on the device.
    file.sync_all().map_err(|_| procedure_write_failure())
}

/// The failure a rewrite reports when the operating system refuses an I/O operation.
///
/// `INTERNAL_ERROR` is the code this CLI already uses for a local I/O failure that says nothing
/// about the request — `render_write_failure` reports a failed stdout write through the same code.
/// Every procedure code would be a false statement here: the document was found, is valid, and is
/// renderable, and the only thing that went wrong is that the filesystem would not take it.
fn procedure_write_failure() -> LocalFailure {
    LocalFailure::catalog(
        "INTERNAL_ERROR",
        "cannot write the procedure file",
        PROCEDURE_FORMAT_COMMAND,
    )
}

/// The `procedure format` success result: the canonical document plus the mode that produced it.
///
/// `document` is present in every mode because the source result schema requires it, which is the
/// right requirement: a `--check` client that learns the file has drifted with no way to see the
/// canonical form would have to run the command a second time to act on the answer.
fn procedure_format_source(
    source_path: &str,
    formatted: &FormattedProcedureV2,
    mode: &str,
    text: String,
) -> RunResult {
    let Value::Object(result) = json!({
        "schema": "podway.procedure-source-result/v1",
        "operation": "format",
        "target_schema": "podway.procedure/v2",
        "target_digest": formatted.digest().as_str(),
        "document": formatted.document(),
        "file": source_path,
        "mode": mode,
        "changed": formatted.changed(),
    }) else {
        unreachable!("the static source result is a JSON object");
    };
    local_result_v2(PROCEDURE_FORMAT_COMMAND, result, text, 0)
}

/// The `procedure format` findings result, shared by every stage that can produce one.
fn procedure_format_diagnostics(
    source_path: &str,
    diagnostics: Vec<podway_core::AuthoringDiagnostic>,
) -> RunResult {
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (AuthoringStage::Format, diagnostic))
            .collect(),
    );
    let text = render_authoring_diagnostics(report.diagnostics());
    let Value::Object(result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "format",
        "procedure_schema": "podway.procedure/v2",
        "file": source_path,
        "valid": report.valid(),
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.truncated(),
        "diagnostics_total": report.total(),
    }) else {
        unreachable!("the static diagnostics result is a JSON object");
    };
    local_result_v2(PROCEDURE_FORMAT_COMMAND, result, text, 1)
}

/// The route every `procedure vet` result reports under.
const PROCEDURE_VET_COMMAND: &str = "procedure.vet";

/// Runs mandatory graph-wide analysis over a validated Procedure v2 document.
///
/// Vet is unconditionally read-only. Parsing or closed-reference validation stops the pipeline
/// with that single diagnostic; a validated model always carries its digest, including when graph
/// analysis rejects it. Every vet finding is an error, so no warning policy is part of this route.
fn execute_procedure_vet(file: &Path) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_VET_COMMAND;

    let opened = open_offline_procedure(file).map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(&opened.bytes).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureFormatV1::Json
    } else {
        ProcedureFormatV1::Yaml
    };
    let source_path = file.display().to_string();

    if sniff_procedure_schema(source.as_bytes(), format) == Some(PROCEDURE_SCHEMA_V1) {
        return Err(procedure_vet_schema_failure());
    }

    let context = AuthoringContext::new(&source_path, source, format);
    let parsed = match parse_procedure_document(source.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
        Ok(ParsedProcedure::V1(_)) => return Err(procedure_vet_schema_failure()),
        Err(error) => {
            return Ok(procedure_vet_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
            ));
        }
    };
    let validated = match validate_procedure_v2(parsed) {
        Ok(validated) => validated,
        Err(error) => {
            return Ok(procedure_vet_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
            ));
        }
    };

    let findings = vet_procedure_v2(&validated, &context);
    Ok(procedure_vet_diagnostics(
        &source_path,
        Some(validated.digest()),
        AuthoringStage::Vet,
        findings,
    ))
}

fn procedure_vet_schema_failure() -> LocalFailure {
    LocalFailure::catalog(
        "PROCEDURE_SCHEMA_UNSUPPORTED",
        "procedure vet requires a podway.procedure/v2 document; run podway procedure convert first",
        PROCEDURE_VET_COMMAND,
    )
}

fn procedure_vet_diagnostics(
    source_path: &str,
    digest: Option<&Sha256Digest>,
    stage: AuthoringStage,
    diagnostics: Vec<podway_core::AuthoringDiagnostic>,
) -> RunResult {
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (stage, diagnostic))
            .collect(),
    );
    let text = if report.diagnostics().is_empty() {
        format!("{source_path}: graph vetting passed\n")
    } else {
        render_authoring_diagnostics(report.diagnostics())
    };
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "vet",
        "procedure_schema": PROCEDURE_SCHEMA_V2,
        "file": source_path,
        "valid": report.valid(),
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.truncated(),
        "diagnostics_total": report.total(),
    }) else {
        unreachable!("the static diagnostics result is a JSON object");
    };
    if let Some(digest) = digest {
        result.insert(
            "digest".to_owned(),
            Value::String(digest.as_str().to_owned()),
        );
    }
    let exit_code = i32::from(!report.valid());
    local_result_v2(PROCEDURE_VET_COMMAND, result, text, exit_code)
}

/// The route every `procedure graph` result reports under.
const PROCEDURE_GRAPH_COMMAND: &str = "procedure.graph";

/// Projects a validated and vetted Procedure v2 graph in the selected deterministic format.
///
/// This route is unconditionally read-only. It deliberately repeats vetting against the exact
/// bytes opened for this invocation: a previous successful `procedure vet` result cannot admit a
/// file that changed afterwards. Parsing or closed-reference validation has no canonical digest;
/// every later rejection does.
fn execute_procedure_graph(file: &Path, format_name: &str) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_GRAPH_COMMAND;

    debug_assert!(
        matches!(format_name, "json" | "mermaid" | "puml" | "dot"),
        "Clap admits only implemented graph formats"
    );
    let opened = open_offline_procedure(file).map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(&opened.bytes).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureFormatV1::Json
    } else {
        ProcedureFormatV1::Yaml
    };
    let source_path = file.display().to_string();

    if sniff_procedure_schema(source.as_bytes(), format) == Some(PROCEDURE_SCHEMA_V1) {
        return Err(procedure_graph_schema_failure());
    }

    let context = AuthoringContext::new(&source_path, source, format);
    let parsed = match parse_procedure_document(source.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
        Ok(ParsedProcedure::V1(_)) => return Err(procedure_graph_schema_failure()),
        Err(error) => {
            return Ok(procedure_graph_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
            ));
        }
    };
    let validated = match validate_procedure_v2(parsed) {
        Ok(validated) => validated,
        Err(error) => {
            return Ok(procedure_graph_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
            ));
        }
    };

    let findings = vet_procedure_v2(&validated, &context);
    if !findings.is_empty() {
        return Ok(procedure_graph_diagnostics(
            &source_path,
            Some(validated.digest()),
            AuthoringStage::Vet,
            findings,
        ));
    }

    let (result_format, projection) = match format_name {
        "json" => (
            "json",
            project_procedure_v2_graph(&validated).map(|projection| {
                (
                    projection.projection().to_owned(),
                    projection.projection_digest().clone(),
                )
            }),
        ),
        "mermaid" => (
            "mermaid",
            normalize_procedure_v2_graph(&validated)
                .and_then(|graph| project_procedure_v2_mermaid(&graph))
                .map(|projection| {
                    (
                        projection.projection().to_owned(),
                        projection.projection_digest().clone(),
                    )
                }),
        ),
        "puml" => (
            "plantuml",
            normalize_procedure_v2_graph(&validated)
                .and_then(|graph| project_procedure_v2_plantuml(&graph))
                .map(|projection| {
                    (
                        projection.projection().to_owned(),
                        projection.projection_digest().clone(),
                    )
                }),
        ),
        "dot" => (
            "dot",
            normalize_procedure_v2_graph(&validated)
                .and_then(|graph| project_procedure_v2_dot(&graph))
                .map(|projection| {
                    (
                        projection.projection().to_owned(),
                        projection.projection_digest().clone(),
                    )
                }),
        ),
        _ => unreachable!("Clap admits only implemented graph formats"),
    };
    let (projection, projection_digest) = match projection {
        Ok(projection) => projection,
        Err(error) => {
            return Ok(procedure_graph_diagnostics(
                &source_path,
                Some(validated.digest()),
                AuthoringStage::Vet,
                vec![config_error_diagnostic(&error, &context)],
            ));
        }
    };
    let text = format!("{projection}\n");
    let Value::Object(result) = json!({
        "schema": "podway.procedure-graph-result/v1",
        "procedure_schema": PROCEDURE_SCHEMA_V2,
        "procedure_digest": validated.digest().as_str(),
        "format": result_format,
        "projection_digest": projection_digest.as_str(),
        "projection": projection,
    }) else {
        unreachable!("the static graph result is a JSON object");
    };
    Ok(local_result_v2(PROCEDURE_GRAPH_COMMAND, result, text, 0))
}

fn procedure_graph_schema_failure() -> LocalFailure {
    LocalFailure::catalog(
        "PROCEDURE_SCHEMA_UNSUPPORTED",
        "procedure graph requires a podway.procedure/v2 document; run podway procedure convert first",
        PROCEDURE_GRAPH_COMMAND,
    )
}

fn procedure_graph_diagnostics(
    source_path: &str,
    digest: Option<&Sha256Digest>,
    stage: AuthoringStage,
    diagnostics: Vec<podway_core::AuthoringDiagnostic>,
) -> RunResult {
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (stage, diagnostic))
            .collect(),
    );
    let text = render_authoring_diagnostics(report.diagnostics());
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "graph",
        "procedure_schema": PROCEDURE_SCHEMA_V2,
        "file": source_path,
        "valid": report.valid(),
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.truncated(),
        "diagnostics_total": report.total(),
    }) else {
        unreachable!("the static diagnostics result is a JSON object");
    };
    if let Some(digest) = digest {
        result.insert(
            "digest".to_owned(),
            Value::String(digest.as_str().to_owned()),
        );
    }
    local_result_v2(PROCEDURE_GRAPH_COMMAND, result, text, 1)
}

/// The route every `procedure preview` result reports under.
const PROCEDURE_PREVIEW_COMMAND: &str = "procedure.preview";

/// Aggregates the complete local admission preview without touching daemon or runtime state.
fn execute_procedure_preview(file: &Path) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_PREVIEW_COMMAND;

    if !is_worktree_relative_procedure_path(file) {
        return Err(
            LocalFailure::request_invalid("procedure must be worktree-relative").with_command(NAME),
        );
    }
    let Some(source_path) = file.to_str() else {
        return Err(
            LocalFailure::request_invalid("procedure path must be UTF-8").with_command(NAME),
        );
    };
    let opened = open_descriptor_relative_procedure(
        Path::new("."),
        file,
        LocalFailure::catalog("PATH_OUTSIDE_WORKTREE", "cannot open worktree", "procedure"),
    )
    .map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(&opened.bytes).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureFormatV1::Json
    } else {
        ProcedureFormatV1::Yaml
    };
    if sniff_procedure_schema(source.as_bytes(), format) == Some(PROCEDURE_SCHEMA_V1) {
        return Err(LocalFailure::catalog(
            "PROCEDURE_SCHEMA_UNSUPPORTED",
            "procedure preview requires a podway.procedure/v2 document; run podway procedure convert first",
            NAME,
        ));
    }

    let report = preview_procedure_v2(FormatRequest {
        source,
        source_path,
        format,
    });
    let checks = report.checks();
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-preview-result/v1",
        "file": source_path,
        "admissible": report.admissible(),
        "checks": {
            "validate": checks.validate(),
            "vet": checks.vet(),
            "lint": checks.lint(),
        },
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.diagnostics_truncated(),
        "diagnostics_total": report.diagnostics_total(),
    }) else {
        unreachable!("the static preview result is a JSON object");
    };

    let text = if let Some(details) = report.details() {
        let summary = details.summary();
        let graph = details.graph();
        let nodes = graph
            .nodes()
            .iter()
            .map(|node| {
                json!({
                    "graph_node_id": node.graph_node_id(),
                    "node_definition_id": node.node_definition_id(),
                    "node_type": node.node_type().as_str(),
                    "terminal": node.terminal(),
                    "skippable": node.skippable(),
                })
            })
            .collect::<Vec<_>>();
        let edges = graph
            .edges()
            .iter()
            .map(|edge| {
                let mut edge_value = json!({
                    "from_graph_node_id": edge.from_graph_node_id(),
                    "to_graph_node_id": edge.to_graph_node_id(),
                    "effect": edge.effect(),
                });
                if let Some(option_id) = edge.option_id() {
                    edge_value["option_id"] = Value::String(option_id.to_owned());
                }
                edge_value
            })
            .collect::<Vec<_>>();
        let start_suggestion = details.start_suggestion();
        result.extend(
            json!({
                "procedure_schema": details.procedure_schema(),
                "procedure_id": details.procedure_id(),
                "procedure_version": details.procedure_version(),
                "purpose": details.purpose(),
                "procedure_digest": details.procedure_digest().as_str(),
                "goal_tracking": details.goal_tracking(),
                "goal_assessment_graph_node_ids": details.goal_assessment_graph_node_ids(),
                "summary": {
                    "definition_count": summary.definition_count(),
                    "graph_node_count": summary.graph_node_count(),
                    "action_node_count": summary.action_node_count(),
                    "decision_node_count": summary.decision_node_count(),
                    "route_count": summary.route_count(),
                    "cycle_count": summary.cycle_count(),
                    "evidence_reference_count": summary.evidence_reference_count(),
                    "skippable_node_count": summary.skippable_node_count(),
                    "manual_rework_target_count": summary.manual_rework_target_count(),
                },
                "graph": {
                    "entry_graph_node_id": graph.entry_graph_node_id(),
                    "terminal_graph_node_ids": graph.terminal_graph_node_ids(),
                    "nodes": nodes,
                    "edges": edges,
                },
                "mermaid": details.mermaid(),
                "start_suggestion": {
                    "command": start_suggestion.command(),
                    "argv": start_suggestion.argv(),
                },
            })
            .as_object()
            .expect("the static preview details are a JSON object")
            .clone(),
        );

        let mut text = render_authoring_diagnostics(report.diagnostics());
        let assessments = if details.goal_assessment_graph_node_ids().is_empty() {
            "none".to_owned()
        } else {
            details.goal_assessment_graph_node_ids().join(",")
        };
        let terminals = graph.terminal_graph_node_ids().join(",");
        text.push_str(&format!(
            concat!(
                "{}: Procedure v2 preview admissible\n",
                "procedure: {} {}@{}\n",
                "purpose: {}\n",
                "procedure-digest: {}\n",
                "checks: validate={}, vet={}, lint={}\n",
                "goal-tracking: {}, goal-assessments: {}\n",
                "summary: definitions={}, graph-nodes={}, actions={}, decisions={}, ",
                "decision-routes={}, cyclic-regions={}, evidence-references={}, ",
                "skippable-nodes={}, manual-rework-targets={}\n",
                "graph: entry={}, terminals={}\n",
                "graph-nodes:\n"
            ),
            file.display(),
            details.procedure_schema(),
            details.procedure_id(),
            render_preview_text_value(details.procedure_version()),
            render_preview_text_value(details.purpose()),
            details.procedure_digest().as_str(),
            checks.validate(),
            checks.vet(),
            checks.lint(),
            details.goal_tracking(),
            assessments,
            summary.definition_count(),
            summary.graph_node_count(),
            summary.action_node_count(),
            summary.decision_node_count(),
            summary.route_count(),
            summary.cycle_count(),
            summary.evidence_reference_count(),
            summary.skippable_node_count(),
            summary.manual_rework_target_count(),
            graph.entry_graph_node_id(),
            terminals,
        ));
        for node in graph.nodes() {
            text.push_str(&format!(
                "  {}: definition={}, type={}, terminal={}, skippable={}\n",
                node.graph_node_id(),
                node.node_definition_id(),
                node.node_type().as_str(),
                node.terminal(),
                node.skippable(),
            ));
        }
        text.push_str("graph-edges:\n");
        for edge in graph.edges() {
            text.push_str(&format!(
                "  {} -> {}: {}{}\n",
                edge.from_graph_node_id(),
                edge.to_graph_node_id(),
                edge.option_id()
                    .map_or_else(String::new, |option| format!("{option} · ")),
                edge.effect(),
            ));
        }
        text.push_str(&format!(
            "\n{}\n\nstart: {}\n",
            details.mermaid(),
            render_preview_start_command(start_suggestion.argv()),
        ));
        text
    } else {
        render_authoring_diagnostics(report.diagnostics())
    };
    let exit_code = i32::from(!report.admissible());
    Ok(local_result_v2(NAME, result, text, exit_code))
}

fn is_worktree_relative_procedure_path(path: &Path) -> bool {
    !path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn render_preview_text_value(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') {
            rendered.extend(character.escape_default());
        } else {
            rendered.push(character);
        }
    }
    rendered
}

/// Renders a structured preview suggestion as copyable POSIX shell source for human output.
fn render_preview_start_command(argv: &[String]) -> String {
    argv.iter()
        .map(|argument| {
            if !argument.is_empty()
                && argument
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"@%_+=:,./-".contains(&byte))
            {
                argument.clone()
            } else {
                format!("'{}'", argument.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The route every `procedure lint` result reports under, named once so the two result builders
/// below cannot disagree with the route the failure paths use.
const PROCEDURE_LINT_COMMAND: &str = "procedure.lint";

/// Reports the advisory authoring findings of a Procedure v2 document.
///
/// Lint is a read: the hardened descriptor-relative walk that `procedure format` uses opens the
/// file, and nothing afterwards touches the filesystem at all.
///
/// The pipeline is deliberately shorter than the formatter's. Parsing and closed-reference
/// validation run, and then the rules run on the validated model; the supported-construct scan and
/// the comment pass do not, because lint never re-emits the document, so a source construct the
/// formatter could not reproduce is still a source lint can read. A document that fails parsing or
/// validation is reported through that failure and is *not* linted: every rule reads a resolved
/// model, and advisory findings about a document Podway is already rejecting would bury the
/// rejection.
///
/// `--warnings-as-errors` is a policy about the invocation, not a statement about the document. It
/// moves the exit code from 0 to 1 and changes nothing else: the result body, every `severity`, and
/// `valid` are byte-identical with and without it.
fn execute_procedure_lint(
    file: &Path,
    warnings_as_errors: bool,
) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_LINT_COMMAND;

    let opened = open_offline_procedure(file).map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(&opened.bytes).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureFormatV1::Json
    } else {
        ProcedureFormatV1::Yaml
    };
    let source_path = file.display().to_string();

    // A document that declares the v1 schema is refused before the dispatching parser runs, so a
    // malformed v1 document is a wrong-schema command failure rather than a v2 authoring finding
    // about a document that never claimed to be v2.
    if sniff_procedure_schema(source.as_bytes(), format) == Some(PROCEDURE_SCHEMA_V1) {
        return Err(procedure_lint_schema_failure());
    }

    let context = AuthoringContext::new(&source_path, source, format);
    let parsed = match parse_procedure_document(source.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
        Ok(ParsedProcedure::V1(_)) => return Err(procedure_lint_schema_failure()),
        Err(error) => {
            return Ok(procedure_lint_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
                1,
            ));
        }
    };
    let validated = match validate_procedure_v2(parsed) {
        Ok(validated) => validated,
        Err(error) => {
            return Ok(procedure_lint_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
                1,
            ));
        }
    };

    let findings = lint_procedure_v2(&validated, &context);
    let exit_code = i32::from(warnings_as_errors && !findings.is_empty());
    Ok(procedure_lint_diagnostics(
        &source_path,
        Some(validated.digest()),
        AuthoringStage::Lint,
        findings,
        exit_code,
    ))
}

/// The failure a Procedure v1 input reports. The diagnostics result schema pins
/// `procedure_schema` to `podway.procedure/v2`, so a v1 document has no representable findings
/// document and must be refused as a wrong-schema command failure instead.
fn procedure_lint_schema_failure() -> LocalFailure {
    LocalFailure::catalog(
        "PROCEDURE_SCHEMA_UNSUPPORTED",
        "procedure lint requires a podway.procedure/v2 document; run podway procedure convert first",
        PROCEDURE_LINT_COMMAND,
    )
}

/// The pinned one-line verdict for a document with no advisory findings.
fn no_lint_findings_summary(source_path: &str) -> String {
    format!("{source_path}: no lint findings\n")
}

/// The `procedure lint` result, shared by the advisory report and by the two document-level
/// failures that stop the pipeline before the rules run.
///
/// `digest` is present exactly when validation produced one, which is exactly when the findings are
/// advisory: a client can therefore tell "these warnings describe procedure `sha256:…`" from "this
/// document has no digest because it is not admissible" without inspecting severities.
fn procedure_lint_diagnostics(
    source_path: &str,
    digest: Option<&Sha256Digest>,
    stage: AuthoringStage,
    diagnostics: Vec<podway_core::AuthoringDiagnostic>,
    exit_code: i32,
) -> RunResult {
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (stage, diagnostic))
            .collect(),
    );
    let text = if report.diagnostics().is_empty() {
        no_lint_findings_summary(source_path)
    } else {
        render_authoring_diagnostics(report.diagnostics())
    };
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "lint",
        "procedure_schema": "podway.procedure/v2",
        "file": source_path,
        "valid": report.valid(),
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.truncated(),
        "diagnostics_total": report.total(),
    }) else {
        unreachable!("the static diagnostics result is a JSON object");
    };
    if let Some(digest) = digest {
        result.insert(
            "digest".to_owned(),
            Value::String(digest.as_str().to_owned()),
        );
    }
    local_result_v2(PROCEDURE_LINT_COMMAND, result, text, exit_code)
}

/// The route every `procedure check` result reports under, named once so the result builder below
/// cannot disagree with the route the failure paths use.
const PROCEDURE_CHECK_COMMAND: &str = "procedure.check";

/// Runs every authoring stage over a Procedure v2 document and reports one merged verdict.
///
/// Check is the aggregate gate of section 11.5, not a new analysis: it runs the same parse,
/// validation, canonical rendering, vet, and lint stages the individual commands run, and its value
/// is that a caller gets all of their findings in one bounded, deterministically ordered result
/// instead of running four commands and merging them by hand. The drift finding is produced by the
/// same constructor `format --check` uses, so the two commands can never disagree about whether a
/// file has drifted or about where.
///
/// The vet stage is shared with `procedure vet`: it includes the structural graph rules and
/// wire-budget proofs, so the aggregate and standalone commands cannot disagree.
///
/// Only the absence of a *model* stops the pipeline. A document that parses and validates is
/// vetted and linted even when it has drifted or cannot be rendered at all, because a stale format
/// must not hide a graph finding.
///
/// Exit behaviour: 0 when nothing was found, 1 when any finding carries error severity — a drifted
/// file included, since `FORMAT_NOT_CANONICAL` is catalogued as an error — and 1 under
/// `--warnings-as-errors` when any finding at all was reported. As with lint, the flag moves the
/// exit code and nothing else: the result body, every `severity`, and `valid` are byte-identical
/// with and without it.
fn execute_procedure_check(
    file: &Path,
    warnings_as_errors: bool,
) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_CHECK_COMMAND;

    let opened = open_offline_procedure(file).map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(&opened.bytes).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureFormatV1::Json
    } else {
        ProcedureFormatV1::Yaml
    };
    let source_path = file.display().to_string();

    // A document that declares the v1 schema is refused before the pipeline runs: the diagnostics
    // result pins `procedure_schema` to `podway.procedure/v2`, so a v1 document has no
    // representable findings result and must be a wrong-schema command failure instead.
    if sniff_procedure_schema(source.as_bytes(), format) == Some(PROCEDURE_SCHEMA_V1) {
        return Err(LocalFailure::catalog(
            "PROCEDURE_SCHEMA_UNSUPPORTED",
            "procedure check requires a podway.procedure/v2 document; run podway procedure convert first",
            NAME,
        ));
    }

    let report = check_procedure_v2(FormatRequest {
        source,
        source_path: &source_path,
        format,
    });
    let exit_code =
        i32::from(!report.valid() || (warnings_as_errors && !report.diagnostics().is_empty()));
    let text = if report.diagnostics().is_empty() {
        all_checks_passed_summary(&source_path)
    } else {
        render_authoring_diagnostics(report.diagnostics())
    };
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "check",
        "procedure_schema": "podway.procedure/v2",
        "file": source_path,
        "valid": report.valid(),
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.truncated(),
        "diagnostics_total": report.total(),
    }) else {
        unreachable!("the static diagnostics result is a JSON object");
    };
    // Present exactly when the document is admissible, which is exactly when the findings describe
    // a procedure rather than explain why there is none.
    if let Some(digest) = report.digest() {
        result.insert(
            "digest".to_owned(),
            Value::String(digest.as_str().to_owned()),
        );
    }
    Ok(local_result_v2(
        PROCEDURE_CHECK_COMMAND,
        result,
        text,
        exit_code,
    ))
}

/// The pinned one-line verdict for a document every authoring stage accepted.
fn all_checks_passed_summary(source_path: &str) -> String {
    format!("{source_path}: all authoring checks passed\n")
}

/// The route every `procedure scaffold` result reports under.
const PROCEDURE_SCAFFOLD_COMMAND: &str = "procedure.scaffold";

/// Emits one authoring starting point.
///
/// This is the only local procedure command that reads nothing: there is no file argument, so there
/// is no path to resolve, no descriptor to open, and no way for the command to fail. It always
/// exits 0.
///
/// `--template` is closed at the parser, so `template` is always a name
/// [`ScaffoldTemplate::from_name`] resolves; an unknown value never reaches here, it is a usage
/// failure Clap reports with exit code 2.
///
/// The digest is derived from the emitted text through the same parse-and-validate path every other
/// Procedure v2 digest comes from rather than being carried beside the template as a second
/// constant. That makes the command self-checking — a template edited into something inadmissible
/// fails here instead of shipping a document whose advertised digest describes nothing — and it
/// keeps one derivation of the digest in the build.
fn execute_procedure_scaffold(template: &str) -> RunResult {
    let template =
        ScaffoldTemplate::from_name(template).expect("the parser closes --template to known names");
    let document = scaffold_procedure_v2(template);
    let parsed = match parse_procedure_document(document.as_bytes(), ProcedureFormatV1::Yaml) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
        Ok(ParsedProcedure::V1(_)) => unreachable!("a scaffold template declares the v2 schema"),
        Err(error) => unreachable!("a scaffold template must parse: {error}"),
    };
    let validated =
        validate_procedure_v2(parsed).expect("a scaffold template is a valid Procedure v2 model");
    let Value::Object(result) = json!({
        "schema": "podway.procedure-source-result/v1",
        "operation": "scaffold",
        "target_schema": "podway.procedure/v2",
        "target_digest": validated.digest().as_str(),
        "document": document,
        "template": template.name(),
    }) else {
        unreachable!("the static source result is a JSON object");
    };
    local_result_v2(PROCEDURE_SCAFFOLD_COMMAND, result, document.to_owned(), 0)
}

/// The route every `procedure convert` result reports under, named once so the two result builders
/// below cannot disagree with the route the failure paths use.
const PROCEDURE_CONVERT_COMMAND: &str = "procedure.convert";

/// Renders a Procedure v1 document as a Procedure v2 authoring candidate.
///
/// Convert reads and never writes. It does not touch the v1 file, does not create the v2 file, and
/// does not start a session: the candidate goes to stdout, and deciding where it belongs — and
/// whether the synthesized `purpose` and `intent` values say the right thing — is the author's
/// review step, not this command's.
///
/// Two schema gates bracket the pipeline, and they are deliberately asymmetric. A document that
/// already declares v2 is refused outright: there is nothing to convert, and reporting the refusal
/// as an authoring finding would claim the document is defective when it is simply finished. A
/// document that declares anything else — v1, an unknown schema, or nothing readable at all — goes
/// to `parse_procedure_v1`, which is the byte-locked v1 admission path every other v1 command uses,
/// so a malformed v1 file reports exactly the error `procedure validate` reports for it.
///
/// There is no `--warnings-as-errors`, and the default v1 warning policy applies: a v1 semantic
/// warning describes the v1 procedure, and refusing to *show* an author what their procedure looks
/// like in v2 because v1 already had an advisory finding about it would help nobody. The warnings
/// are still reachable — `podway procedure validate` reports them for the same file — and the
/// candidate has its own, better-targeted advisory pass in `podway procedure check`.
fn execute_procedure_convert(file: &Path) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_CONVERT_COMMAND;

    let bytes = read_offline_procedure(file).map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(&bytes).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureFormatV1::Json
    } else {
        ProcedureFormatV1::Yaml
    };

    if sniff_procedure_schema(source.as_bytes(), format) == Some(PROCEDURE_SCHEMA_V2) {
        return Err(LocalFailure::catalog(
            "PROCEDURE_SCHEMA_UNSUPPORTED",
            "procedure convert requires a podway.procedure/v1 document; this document already declares podway.procedure/v2",
            NAME,
        ));
    }

    // Not the schema-dispatching parser: a v1 document is admitted by the v1 parser directly, which
    // is what keeps the v1 error surface byte-identical to `procedure validate`'s for the same file.
    let validated = parse_procedure_v1(&bytes, format)
        .map_err(|error| procedure_config_failure(error).with_command(NAME))?;

    let source_path = file.display().to_string();
    let context = AuthoringContext::new(&source_path, source, format);
    match convert_procedure_v1_to_v2(&validated, &context) {
        Ok(converted) => Ok(procedure_convert_source(&converted)),
        Err(diagnostics) => Ok(procedure_convert_diagnostics(&source_path, diagnostics)),
    }
}

/// The `procedure convert` success result: the candidate, and the digest of each end of the
/// conversion.
///
/// It names no file, carries no `mode`, and reports no `changed` flag — the source result schema's
/// `convert` branch forbids all three, correctly: the candidate did not come from a v2 file, was
/// not written to one, and has nothing to have changed against.
fn procedure_convert_source(converted: &ConvertedProcedureV2) -> RunResult {
    let Value::Object(result) = json!({
        "schema": "podway.procedure-source-result/v1",
        "operation": "convert",
        "target_schema": "podway.procedure/v2",
        "target_digest": converted.digest().as_str(),
        "document": converted.document(),
        "source_schema": PROCEDURE_SCHEMA_V1,
        "source_digest": converted.source_digest().as_str(),
    }) else {
        unreachable!("the static source result is a JSON object");
    };
    local_result_v2(
        PROCEDURE_CONVERT_COMMAND,
        result,
        converted.document().to_owned(),
        0,
    )
}

/// The `procedure convert` findings result: every v1 value Procedure v2 will not accept.
///
/// `procedure_schema` is `podway.procedure/v2` because the candidate is what is being diagnosed —
/// these are the reasons no admissible v2 document exists — even though every `field` names a path
/// in the v1 source, which is the document the author has to edit. No `digest` is reported: there
/// is no procedure to have one.
fn procedure_convert_diagnostics(
    source_path: &str,
    diagnostics: Vec<podway_core::AuthoringDiagnostic>,
) -> RunResult {
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (AuthoringStage::Validate, diagnostic))
            .collect(),
    );
    let text = render_authoring_diagnostics(report.diagnostics());
    let Value::Object(result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "convert",
        "procedure_schema": "podway.procedure/v2",
        "file": source_path,
        "valid": report.valid(),
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.truncated(),
        "diagnostics_total": report.total(),
    }) else {
        unreachable!("the static diagnostics result is a JSON object");
    };
    local_result_v2(PROCEDURE_CONVERT_COMMAND, result, text, 1)
}

/// The stable one-line-per-finding authoring report.
///
/// The format is `<source_path>:<line>:<column> <severity> <code> <message>`, mirroring the
/// position-first convention every editor's error parser already understands. The machine-readable
/// form — locations, hints, and graph identities included — is the JSON result.
fn render_authoring_diagnostics(diagnostics: &[podway_core::AuthoringDiagnostic]) -> String {
    let mut text = String::new();
    for diagnostic in diagnostics {
        text.push_str(&format!(
            "{}:{}:{} {} {} {}\n",
            diagnostic.source_path(),
            diagnostic.location().line(),
            diagnostic.location().column(),
            diagnostic.severity().as_str(),
            diagnostic.code().as_str(),
            diagnostic.message(),
        ));
    }
    text
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
    let lookup = matches!(
        command,
        Command::Job {
            command: JobCommand::Lookup
        }
    );
    if lookup && cli.idempotency_key.is_none() {
        return Err(LocalFailure::request_invalid(
            "job lookup requires --idempotency-key",
        ));
    }
    if !command.is_mutation() && (cli.detach || (cli.idempotency_key.is_some() && !lookup)) {
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
    if matches!(
        command,
        Command::Decide(_) | Command::Rework(_) | Command::Goal { .. }
    ) && !fully_fenced_v2_mutation(command, &explicit)
    {
        return Err(LocalFailure::request_invalid(
            "reserved Procedure v2 mutations require all command-specific explicit preconditions",
        ));
    }
    if matches!(
        command,
        Command::Start(StartArgs {
            replace: true,
            goal: Some(_),
            ..
        })
    ) && !fully_fenced_v2_start_replace(command, &explicit)
    {
        return Err(LocalFailure::request_invalid(
            "goal-bearing start replacement requires explicit workspace and session identity preconditions",
        ));
    }
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
    if explicit.any_transition() && !command.needs_preflight() {
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
            Command::Goal {
                command: GoalCommand::Revise(_) | GoalCommand::AssessCriterion(_)
            }
        )
    {
        return Err(LocalFailure::request_invalid(
            "--if-goal-revision applies only to goal revise and goal assess-criterion",
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
    if let (true, Command::Reset(args)) = (wire == "workspace.reset_all", command) {
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
        Command::Start(args) => {
            validate_optional_goal(&args.goal, &args.criterion, args.actor.as_deref())?;
        }
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
    lookup_idempotency_key: Option<&str>,
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
            if let Some(digest) = &args.expect_procedure_digest {
                payload.insert(
                    "expected_procedure_digest".to_owned(),
                    Value::String(digest.clone()),
                );
            }
            if let Some(goal) = &args.goal {
                payload.insert("goal".to_owned(), Value::String(goal.clone()));
                payload.insert(
                    "criteria".to_owned(),
                    Value::Array(criteria_json(&args.criterion)),
                );
                if let Some(actor) = &args.actor {
                    payload.insert("actor".to_owned(), Value::String(actor.clone()));
                }
            }
            if args.dry_run {
                payload.insert("dry_run".to_owned(), Value::Bool(true));
            } else if args.replace {
                payload.insert("confirmed".to_owned(), Value::Bool(true));
            }
        }
        Command::Status(args) => {
            read_payload(&mut payload, &args.read);
            if args.compact {
                payload.insert("compact".to_owned(), Value::Bool(true));
            }
        }
        Command::Next(args) => read_payload(&mut payload, args),
        Command::Complete => {}
        Command::Decide(args) => {
            payload.insert("option_id".to_owned(), Value::String(args.option.clone()));
            payload.insert("reason".to_owned(), Value::String(args.reason.clone()));
            insert_actor(&mut payload, args.actor.as_deref());
        }
        Command::Rework(args) => {
            payload.insert(
                "target_graph_node_id".to_owned(),
                Value::String(args.to.clone()),
            );
            payload.insert("reason".to_owned(), Value::String(args.reason.clone()));
            insert_actor(&mut payload, args.actor.as_deref());
        }
        Command::Goal {
            command: GoalCommand::Define(args),
        } => {
            insert_goal_definition(
                &mut payload,
                &args.goal,
                &args.criterion,
                args.actor.as_deref(),
            );
        }
        Command::Goal {
            command: GoalCommand::Revise(args),
        } => {
            insert_goal_definition(
                &mut payload,
                &args.goal,
                &args.criterion,
                args.actor.as_deref(),
            );
            payload.insert(
                "target_graph_node_id".to_owned(),
                Value::String(args.rework_to.clone()),
            );
            payload.insert("reason".to_owned(), Value::String(args.reason.clone()));
            if args.reactivate {
                payload.insert("reactivate".to_owned(), Value::Bool(true));
            }
        }
        Command::Goal {
            command: GoalCommand::AssessCriterion(args),
        } => {
            payload.insert(
                "criterion_id".to_owned(),
                Value::String(args.criterion_id.clone()),
            );
            payload.insert("status".to_owned(), Value::String(args.status.clone()));
            payload.insert("reason".to_owned(), Value::String(args.reason.clone()));
            if !args.evidence.is_empty() {
                payload.insert("evidence".to_owned(), json!(args.evidence));
            }
            if !args.item.is_empty() {
                payload.insert("items".to_owned(), json!(args.item));
            }
            insert_actor(&mut payload, args.actor.as_deref());
        }
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
            JobCommand::Lookup => {
                let key = IdempotencyKeyV1::new(
                    lookup_idempotency_key
                        .ok_or_else(|| {
                            LocalFailure::request_invalid("job lookup requires --idempotency-key")
                        })?
                        .to_owned(),
                )
                .map_err(|_| LocalFailure::request_invalid("invalid idempotency key"))?;
                payload.insert(
                    "idempotency_key".to_owned(),
                    Value::String(key.as_str().to_owned()),
                );
            }
            JobCommand::Status { job_id }
            | JobCommand::Wait { job_id }
            | JobCommand::Cancel { job_id } => {
                payload.insert("job_id".to_owned(), Value::String(job_id.clone()));
            }
        },
        Command::CompleteDynamic { .. }
        | Command::Help { .. }
        | Command::Version { .. }
        | Command::Completions { .. }
        | Command::Procedure { .. }
        | Command::Preset { .. }
        | Command::Daemon { .. }
        | Command::Terminate => {
            return Err(LocalFailure::request_invalid("unsupported daemon command"));
        }
    }
    Ok((operation, payload))
}

fn criteria_json(criteria: &[String]) -> Vec<Value> {
    criteria
        .iter()
        .map(|criterion| {
            let (criterion_id, statement) = criterion
                .split_once('=')
                .expect("criterion shape is validated before payload construction");
            json!({ "criterion_id": criterion_id, "statement": statement })
        })
        .collect()
}

fn insert_actor(payload: &mut Map<String, Value>, actor: Option<&str>) {
    if let Some(actor) = actor {
        payload.insert("actor".to_owned(), Value::String(actor.to_owned()));
    }
}

fn insert_goal_definition(
    payload: &mut Map<String, Value>,
    goal: &str,
    criteria: &[String],
    actor: Option<&str>,
) {
    payload.insert("goal".to_owned(), Value::String(goal.to_owned()));
    payload.insert("criteria".to_owned(), Value::Array(criteria_json(criteria)));
    insert_actor(payload, actor);
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
    match command {
        Command::Set(args) if args.stdin => {
            args.value = Some(read_stdin_text()?);
        }
        _ => {}
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

fn daemon_client(
    wait_timeout_ms: u64,
    socket_path: Option<&Path>,
    dev_mode: bool,
) -> Result<DaemonClientV1, LocalFailure> {
    if dev_mode && socket_path.is_some() {
        return Err(LocalFailure::request_invalid(
            "--dev and --socket are mutually exclusive",
        ));
    }
    let paths = if dev_mode {
        effective_dev_paths("cli")?
    } else {
        effective_service_paths("cli")?
    };
    let paths = if socket_path.is_some() || dev_mode {
        paths
    } else {
        resolve_installed_service_endpoint(paths, "cli")?
    };
    let read_timeout = Duration::from_millis(wait_timeout_ms.saturating_add(1_000))
        .max(DEFAULT_DAEMON_CONNECT_TIMEOUT_V1);
    let timeouts = DaemonClientTimeoutsV1::new(
        DEFAULT_DAEMON_CONNECT_TIMEOUT_V1,
        read_timeout,
        DEFAULT_DAEMON_WRITE_TIMEOUT_V1,
    )
    .map_err(|_| LocalFailure::daemon_unavailable("cli"))?;
    let paths = match socket_path {
        Some(socket_path) => paths
            .with_socket_path(socket_path)
            .map_err(socket_path_failure)?,
        None => paths,
    };
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

fn build_daemon_status_request() -> Result<RequestEnvelopeV1, LocalFailure> {
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(Uuid::new_v4().to_string())
            .map_err(|_| LocalFailure::daemon_unavailable("daemon.status"))?,
        client: ClientInfoV1::new("podway", env!("CARGO_PKG_VERSION"), std::process::id())
            .map_err(|_| LocalFailure::daemon_unavailable("daemon.status"))?,
        operation: OperationV1::Control,
        command: CommandNameV1::new("daemon.status")
            .map_err(|_| LocalFailure::request_invalid("invalid daemon status command"))?,
        workspace: None,
        idempotency_key: None,
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0)
            .map_err(|_| LocalFailure::request_invalid("invalid daemon status options"))?,
        payload: Map::new(),
    })
    .map_err(|_| LocalFailure::request_invalid("invalid daemon status request"))
}

fn build_daemon_terminate_request() -> Result<RequestEnvelopeV1, LocalFailure> {
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(Uuid::new_v4().to_string())
            .map_err(|_| LocalFailure::daemon_unavailable("daemon.terminate"))?,
        client: ClientInfoV1::new("podway", env!("CARGO_PKG_VERSION"), std::process::id())
            .map_err(|_| LocalFailure::daemon_unavailable("daemon.terminate"))?,
        operation: OperationV1::Control,
        command: CommandNameV1::new("daemon.terminate")
            .map_err(|_| LocalFailure::request_invalid("invalid daemon terminate command"))?,
        workspace: None,
        idempotency_key: None,
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0)
            .map_err(|_| LocalFailure::request_invalid("invalid daemon terminate options"))?,
        payload: Map::new(),
    })
    .map_err(|_| LocalFailure::request_invalid("invalid daemon terminate request"))
}

fn mutation_key(value: Option<String>) -> Result<IdempotencyKeyV1, LocalFailure> {
    IdempotencyKeyV1::new(value.unwrap_or_else(|| Uuid::new_v4().to_string()))
        .map_err(|_| LocalFailure::request_invalid("invalid idempotency key"))
}

fn request_daemon(
    client: &DaemonClientV1,
    request: &RequestEnvelopeV1,
) -> Result<ResponseEnvelopeV2, LocalFailure> {
    client.request_v2(request).map_err(|error| {
        map_client_error_for_request(error, request)
            .with_correlation(request.command().as_str(), request.request_id().as_str())
    })
}

fn map_client_error_for_request(
    error: DaemonClientErrorV1,
    request: &RequestEnvelopeV1,
) -> LocalFailure {
    let is_mutation = matches!(
        request.operation(),
        OperationV1::Mutate | OperationV1::Bootstrap
    );
    if is_mutation && error.request_may_have_been_transmitted() {
        return request.idempotency_key().map_or_else(
            || LocalFailure::response_invalid("mutation response was lost without a request key"),
            LocalFailure::mutation_outcome_unknown,
        );
    }
    let mut failure = map_client_error(error);
    if is_mutation {
        failure.details.insert(
            "admission".to_owned(),
            json!({
                "admitted": false,
            }),
        );
    }
    failure
}

fn re_correlate_preflight_error(
    error: &podway_protocol::ErrorEnvelopeV1,
    command: &str,
) -> Result<RunResult, LocalFailure> {
    let mut envelope = serde_json::to_value(error)
        .map_err(|_| LocalFailure::response_invalid("status preflight error cannot be read"))?;
    {
        let envelope = envelope
            .as_object_mut()
            .ok_or_else(|| LocalFailure::response_invalid("status preflight error is invalid"))?;
        envelope.insert("command".to_owned(), Value::String(command.to_owned()));
        let details = envelope
            .get_mut("details")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| LocalFailure::response_invalid("status preflight error is invalid"))?;
        details.remove("job_id");
        details.remove("job_sequence");
        details.insert("admission".to_owned(), json!({"admitted": false}));
    }
    let error = serde_json::from_value(envelope)
        .map_err(|_| LocalFailure::response_invalid("status preflight error is invalid"))?;
    Ok(RunResult::Response(Box::new(ResponseEnvelopeV1::Error(
        error,
    ))))
}

fn map_client_error(error: DaemonClientErrorV1) -> LocalFailure {
    match error {
        DaemonClientErrorV1::RequestPossiblyTransmitted { source } => map_client_error(*source),
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
        | DaemonClientErrorV1::EndpointSecurity { .. }
        | DaemonClientErrorV1::PeerIdentity { .. }
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

fn parse_positive_u64(value: &str) -> Result<u64, String> {
    let value = value
        .parse::<u64>()
        .map_err(|_| "value must be a positive integer".to_owned())?;
    if value == 0 {
        return Err("value must be a positive integer".to_owned());
    }
    Ok(value)
}

trait LocalEnvelopeClock {
    fn now(&self) -> SystemTime;
}

#[derive(Clone, Copy, Debug)]
struct SystemServiceClock(UnixMillis);

fn system_service_clock(
    now: SystemTime,
    command: &str,
) -> Result<SystemServiceClock, LocalFailure> {
    let milliseconds = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            LocalFailure::response_invalid("system clock is before the Unix epoch")
                .with_command(command)
        })?
        .as_millis();
    let milliseconds = u64::try_from(milliseconds).map_err(|_| {
        LocalFailure::response_invalid("system clock is out of range").with_command(command)
    })?;
    Ok(SystemServiceClock(UnixMillis::new(milliseconds)))
}

impl ServiceClockV1 for SystemServiceClock {
    fn now(&self) -> UnixMillis {
        self.0
    }
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
        RunResult::ResponseV2(response) => render_response_v2_with_clock_and_writers(
            response,
            json_output,
            quiet,
            clock,
            stdout,
            stderr,
        ),
        RunResult::VersionSummary {
            name,
            version,
            text,
        } => {
            if json_output {
                let output = json!({ "name": name, "version": version });
                if serde_json::to_writer(&mut *stdout, &output).is_err()
                    || writeln!(stdout).is_err()
                {
                    return LOCAL_CLIENT_EXIT;
                }
            } else if !quiet && writeln!(stdout, "{text}").is_err() {
                return LOCAL_CLIENT_EXIT;
            }
            0
        }
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
                let request_id = RequestIdV1::new(Uuid::new_v4().to_string())
                    .expect("UUID-v4 request identifiers satisfy the public protocol");
                let command = CommandNameV1::new(command.clone())
                    .expect("local command names satisfy the public protocol");
                let output = OutputEnvelopeV1::new(OutputEnvelopeInputV1 {
                    request_id,
                    command,
                    generated_at,
                    workspace: None,
                    job: None,
                    session: None,
                    result: result.clone(),
                    warnings: Vec::new(),
                })
                .expect("local results satisfy the public output protocol");
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
        RunResult::LocalV2 {
            command,
            result,
            text,
            exit_code,
        } => {
            if json_output {
                let generated_at = match local_generated_at(clock) {
                    Ok(timestamp) => timestamp,
                    Err(failure) => return render_clock_failure_to(failure, stderr),
                };
                let request_id = RequestIdV1::new(Uuid::new_v4().to_string())
                    .expect("UUID-v4 request identifiers satisfy the public protocol");
                let command = CommandNameV1::new(command.clone())
                    .expect("local command names satisfy the public protocol");
                let output = OutputEnvelopeV2::new(OutputEnvelopeInputV2 {
                    request_id,
                    command,
                    generated_at,
                    workspace: None,
                    job: None,
                    session: None,
                    result: result.clone(),
                    warnings: Vec::new(),
                })
                .expect("local v2 results satisfy the public output protocol");
                if serde_json::to_writer(&mut *stdout, &output).is_err()
                    || writeln!(stdout).is_err()
                {
                    return LOCAL_CLIENT_EXIT;
                }
            // The text payload is the exact byte projection — a formatted document already ends in
            // its own newline — so it is written verbatim rather than through `writeln!`.
            } else if !quiet && !text.is_empty() && write!(stdout, "{text}").is_err() {
                return LOCAL_CLIENT_EXIT;
            }
            *exit_code
        }
        RunResult::LogFollow { path, initial } => {
            if json_output {
                return render_local_failure_with_clock_and_writers(
                    LocalFailure::request_invalid("--follow cannot be combined with --json"),
                    true,
                    clock,
                    stdout,
                    stderr,
                );
            }
            match stream_log_follow(path, initial, stdout) {
                Ok(()) => 0,
                Err(failure) => render_local_failure_with_clock_and_writers(
                    failure, false, clock, stdout, stderr,
                ),
            }
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
    let output_validation = match response {
        ResponseEnvelopeV1::Output(output) => Some(validate_typed_output_result(output)),
        ResponseEnvelopeV1::Error(_) => None,
    };
    if let Some(Err(failure)) = output_validation {
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
    } else {
        let human_result = (!quiet || matches!(response, ResponseEnvelopeV1::Error(_)))
            .then(|| render_human_response(response, stdout, stderr));
        if let Some(Err(failure)) = human_result {
            return render_local_failure_with_clock_and_writers(
                failure, false, clock, stdout, stderr,
            );
        }
    }
    match response {
        ResponseEnvelopeV1::Output(_) => 0,
        ResponseEnvelopeV1::Error(error) => i32::from(error.exit_code().get()),
    }
}

fn render_response_v2_with_clock_and_writers(
    response: &ResponseEnvelopeV2,
    json_output: bool,
    quiet: bool,
    clock: &impl LocalEnvelopeClock,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if json_output {
        if serde_json::to_writer(&mut *stdout, response).is_err() || writeln!(stdout).is_err() {
            return LOCAL_CLIENT_EXIT;
        }
    } else {
        let rendered = match response {
            ResponseEnvelopeV2::OutputV1(output) if !quiet => {
                render_human_response(&ResponseEnvelopeV1::Output(output.clone()), stdout, stderr)
            }
            ResponseEnvelopeV2::OutputV2(output) if !quiet => {
                render_human_output_v2(output, stdout)
            }
            ResponseEnvelopeV2::Error(error) => {
                render_human_response(&ResponseEnvelopeV1::Error(error.clone()), stdout, stderr)
            }
            ResponseEnvelopeV2::OutputV1(_) | ResponseEnvelopeV2::OutputV2(_) => Ok(()),
        };
        if let Err(failure) = rendered {
            return render_local_failure_with_clock_and_writers(
                failure, false, clock, stdout, stderr,
            );
        }
    }
    match response {
        ResponseEnvelopeV2::OutputV1(_) | ResponseEnvelopeV2::OutputV2(_) => 0,
        ResponseEnvelopeV2::Error(error) => i32::from(error.exit_code().get()),
    }
}

fn render_human_output_v2(
    output: &OutputEnvelopeV2,
    stdout: &mut dyn Write,
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
    let result = serde_json::to_string_pretty(output.result())
        .map_err(|_| LocalFailure::response_invalid("cannot render Procedure v2 result"))?;
    write_text_line(stdout, format_args!("result: {result}"))?;
    render_warnings(stdout, output.warnings())
}

fn validate_typed_output_result(
    output: &podway_protocol::OutputEnvelopeV1,
) -> Result<(), LocalFailure> {
    match output.command().as_str() {
        "session.status"
            if output.result().get("schema").and_then(Value::as_str)
                == Some("podway.compact-status-result/v1") =>
        {
            CompactStatusResultV1::from_result_map(output.result())
                .map(|_| ())
                .map_err(|_| typed_result_failure(output))
        }
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
    mut failure: LocalFailure,
    json_output: bool,
    clock: &impl LocalEnvelopeClock,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if json_output {
        ensure_error_details_schema_v1(failure.code, &mut failure.details);
        let generated_at = match local_generated_at(clock) {
            Ok(timestamp) => timestamp,
            Err(clock_failure) => return render_clock_failure_to(clock_failure, stderr),
        };
        let request_id = failure
            .request_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let output = json!({ "schema": "podway.error/v1", "request_id": request_id, "command": failure.command, "generated_at": generated_at.as_str(), "code": failure.code, "message": failure.message, "retryable": failure.retryable, "exit_code": failure.exit_code, "details": failure.details });
        let output = match serde_json::from_value::<ResponseEnvelopeV1>(output) {
            Ok(output) => output,
            Err(_) => {
                failure = LocalFailure::response_invalid(
                    "the local client could not construct a valid error response",
                );
                let fallback = json!({
                    "schema": "podway.error/v1",
                    "request_id": Uuid::new_v4().to_string(),
                    "command": "cli",
                    "generated_at": generated_at.as_str(),
                    "code": failure.code,
                    "message": failure.message,
                    "retryable": failure.retryable,
                    "exit_code": failure.exit_code,
                    "details": {}
                });
                serde_json::from_value::<ResponseEnvelopeV1>(fallback)
                    .expect("the static local fallback error is protocol-valid")
            }
        };
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
                render_output_metadata(stdout, output)?;
                if output.result().get("schema").and_then(Value::as_str)
                    == Some("podway.compact-status-result/v1")
                {
                    let status = CompactStatusResultV1::from_result_map(output.result())
                        .map_err(|_| typed_result_failure(output))?;
                    render_compact_status_text(stdout, &status)?;
                } else {
                    let status = StatusResultV1::from_result_map(output.result())
                        .map_err(|_| typed_result_failure(output))?;
                    render_status_text(stdout, &status)?;
                }
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

fn render_compact_status_text(
    stdout: &mut dyn Write,
    status: &CompactStatusResultV1,
) -> Result<(), LocalFailure> {
    write_text_line(
        stdout,
        format_args!(
            "procedure: {} version={} digest={}",
            status.procedure.id,
            status.procedure.version,
            status.procedure.digest.as_str()
        ),
    )?;
    write_text_line(
        stdout,
        format_args!(
            "session: {} {:?} revision={}",
            status.session.id.as_str(),
            status.session.lifecycle,
            status.session.revision.get()
        ),
    )?;
    match &status.current {
        Some(current) => write_text_line(
            stdout,
            format_args!(
                "current: {} attempt={} id={} ready_to_complete={}",
                current.stage_id.as_str(),
                current.attempt_number,
                current.attempt_id.as_str(),
                current.ready_to_complete
            ),
        )?,
        None => write_text_line(stdout, format_args!("current: none"))?,
    }
    for item in &status.items {
        write_text_line(
            stdout,
            format_args!(
                "item: {} {:?} required={} satisfied={} revision={}",
                item.id.as_str(),
                item.item_type,
                item.required,
                item.satisfied,
                item.revision.get()
            ),
        )?;
    }
    for blocker in &status.blockers {
        write_text_line(
            stdout,
            format_args!(
                "blocker: {} attempt={} state={:?}",
                blocker.id.as_str(),
                blocker.attempt_id.as_str(),
                blocker.state
            ),
        )?;
    }
    write_text_line(
        stdout,
        format_args!(
            "queue: pending_mutations={} queued_count={} running_job_id=- latest_workspace_sequence={}",
            status.queue.pending_mutations,
            status.queue.queued_count,
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

fn dynamic_completion(
    worktree: Option<PathBuf>,
    socket_path: Option<PathBuf>,
    dev_mode: bool,
    kind: &str,
) -> Result<RunResult, LocalFailure> {
    let target = match workspace_target(worktree) {
        Ok(target) => target,
        Err(_) => {
            return Ok(empty_dynamic_completion());
        }
    };
    let client = match daemon_client(200, socket_path.as_deref(), dev_mode) {
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
        Ok(ResponseEnvelopeV2::OutputV1(output)) => dynamic_candidates(output.result(), kind),
        Ok(ResponseEnvelopeV2::OutputV2(_)) | Ok(ResponseEnvelopeV2::Error(_)) | Err(_) => {
            Vec::new()
        }
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
            "Podway coordinates durable worktree-local procedures.\n\nTrust boundary:\n  Podway trusts same-user processes connecting through its local socket.\n  It provides no authentication or workspace access key.\n  It does not protect against malicious same-user processes.\n\nDaemon endpoint:\n  Daemon-backed commands accept --socket <absolute-path>.\n  Without --socket, Podway selects the installed or default per-user endpoint.\n\nUsage:\n  podway help <route>\n\nExamples:\n  podway start --preset sw-dev --task 'add retry backoff'\n  podway status --json\n  podway next"
        }
        "workflow" => {
            "Workflow:\n  podway start --preset sw-dev --task 'implement feature'\n  podway next\n  podway set goal 'Implement the requested feature.'\n  podway add acceptance-criteria 'The requested behavior is verified.'\n  podway complete\n\nProcedure v2 decisions use podway decide; manual graph re-entry uses podway rework."
        }
        "rework" => {
            "Rework:\n  Procedure v1: podway return --to implement --reason 'review found a gap' --dry-run\n  Procedure v1: podway reopen --to implement --reason 'follow-up'\n  Procedure v2: podway rework --to implement --reason 'review found a gap'\n\nThe v1 return and reopen verbs are never aliases for Procedure v2 rework."
        }
        "automation" => {
            "Automation:\n  podway complete --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 12 --if-attempt <uuid> --idempotency-key task-42 --json"
        }
        "procedures" => {
            "Procedures:\n  podway procedure validate .podway/procedures/custom.yaml\n  podway start --procedure .podway/procedures/custom.yaml --task 'perform work'"
        }
        "daemon" => {
            "Daemon lifecycle grammar:\n  podway daemon status\n  podway daemon install --daemon-path /absolute/podwayd\n  podway daemon logs --lines 100"
        }
        "artifacts" => {
            "Artifacts:\n  podway attach verification-reference report.md --media-type text/markdown\n  podway attach verification-reference --reference build:42 --digest sha256:<hex> --size 42 --media-type text/plain"
        }
        "help" => "Usage:\n  podway help [<route>]\n\nExample:\n  podway help session.start",
        "version" => {
            "Usage:\n  podway version [--json] [--identity]\n\nExamples:\n  podway version\n  podway version --json\n  podway version --json --identity"
        }
        "completions" => {
            "Usage:\n  podway completions <bash|zsh|fish>\n\nExample:\n  podway completions bash"
        }
        "procedure.validate" => {
            "Usage:\n  podway procedure validate <file> [--warnings-as-errors]\n\nExample:\n  podway procedure validate .podway/procedures/custom.yaml"
        }
        "procedure.show" => {
            "Usage:\n  podway procedure show <file> [--canonical]\n\nExample:\n  podway procedure show .podway/procedures/custom.yaml --canonical"
        }
        "procedure.format" => {
            "Usage:\n  podway procedure format <file> [--check] [--write]\n\nRenders a Procedure v2 document in canonical authoring form on stdout. --check\nreports drift and writes nothing; --write replaces the named file atomically.\n\nExample:\n  podway procedure format .podway/procedures/custom.yaml"
        }
        "procedure.vet" => {
            "Usage:\n  podway procedure vet <file>\n\nRuns mandatory graph-wide semantic and resource-budget checks over a Procedure\nv2 document without writing anything.\n\nExample:\n  podway procedure vet .podway/procedures/custom.yaml"
        }
        "procedure.graph" => {
            "Usage:\n  podway procedure graph <file> --format <json|mermaid|puml|dot>\n\nEmits a deterministic JSON, Mermaid, PlantUML, or DOT projection of a validated\nand vetted Procedure v2 graph without writing anything.\n\nExample:\n  podway procedure graph .podway/procedures/custom.yaml --format dot"
        }
        "procedure.preview" => {
            "Usage:\n  podway procedure preview <worktree-relative-file>\n\nRuns validation, graph vetting, and advisory lint in memory, then prints the\nnormalized graph, Mermaid review projection, and an exact session start suggestion\nwhen the Procedure v2 document is admissible. The file must use the same UTF-8,\nworktree-relative, no-parent spelling accepted by start. Reads only and never\ncontacts the daemon.\n\nExample:\n  podway procedure preview .podway/procedures/custom.yaml"
        }
        "procedure.lint" => {
            "Usage:\n  podway procedure lint <file> [--warnings-as-errors]\n\nReports advisory authoring findings for a Procedure v2 document. Every finding is\na warning, so the file stays valid and the exit code stays 0 unless\n--warnings-as-errors makes any finding fatal.\n\nExample:\n  podway procedure lint .podway/procedures/custom.yaml --warnings-as-errors"
        }
        "procedure.check" => {
            "Usage:\n  podway procedure check <file> [--warnings-as-errors]\n\nRuns every authoring stage over a Procedure v2 document — canonical formatting,\nvalidation, graph vetting, and lint — and reports one merged result. Exits 1 on\nany error, including formatting drift; --warnings-as-errors also fails on\nadvisory findings.\n\nExample:\n  podway procedure check .podway/procedures/custom.yaml --warnings-as-errors"
        }
        "procedure.scaffold" => {
            "Usage:\n  podway procedure scaffold [--template minimal]\n\nWrites a minimal Procedure v2 authoring starting point to stdout and reads\nnothing. The emitted document is already in canonical authoring form and already\npasses every authoring stage, so redirecting it to a file and running\nprocedure check on that file reports nothing.\n\nExample:\n  podway procedure scaffold --template minimal > .podway/procedures/custom.yaml"
        }
        "procedure.convert" => {
            "Usage:\n  podway procedure convert <file>\n\nRenders a Procedure v1 document as a Procedure v2 authoring candidate on stdout.\nReads only; never writes a file and never starts a session. Each stage becomes\none action node in a linear chain, and the synthesized purpose and intent values\nare marked with review comments. A v1 value Procedure v2 cannot hold is reported\nagainst its v1 path instead of being truncated.\n\nExample:\n  podway procedure convert legacy.yaml > .podway/procedures/legacy-v2.yaml"
        }
        "preset.list" => "Usage:\n  podway preset list\n\nExample:\n  podway preset list",
        "preset.show" => {
            "Usage:\n  podway preset show <name>\n\nExample:\n  podway preset show sw-dev"
        }
        "preset.explain" => {
            "Usage:\n  podway preset explain <name>\n\nExample:\n  podway preset explain sw-dev"
        }
        "daemon.install" => {
            "Usage:\n  podway daemon install [--daemon-path <path>] [--socket <absolute-path>]\n\nExample:\n  podway daemon install --daemon-path /absolute/podwayd --socket /absolute/podwayd.sock"
        }
        "daemon.uninstall" => {
            "Usage:\n  podway daemon uninstall [--purge-logs] [--yes]\n\nExample:\n  podway daemon uninstall --yes"
        }
        "daemon.start" => "Usage:\n  podway daemon start\n\nExample:\n  podway daemon start",
        "daemon.stop" => "Usage:\n  podway daemon stop\n\nExample:\n  podway daemon stop",
        "daemon.restart" => "Usage:\n  podway daemon restart\n\nExample:\n  podway daemon restart",
        "daemon.status" => "Usage:\n  podway daemon status\n\nExample:\n  podway daemon status",
        "daemon.terminate" => {
            "Usage:\n  podway --dev terminate\n\nExample:\n  podway --dev terminate"
        }
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
            "Usage:\n  podway start (--preset <name> | --procedure <file> [--expect-procedure-digest <sha256:hex>]) --task <title> [--goal <text> --criterion <id>=<statement>...] [--actor <text>] [--if-workspace-uuid <uuid>] [--dry-run]\n\nExamples:\n  podway start --preset sw-dev --task 'implement feature'\n  podway start --procedure .podway/procedures/custom.yaml --expect-procedure-digest sha256:<hex> --task 'implement feature'\n  podway start --procedure workflow.yaml --expect-procedure-digest sha256:<hex> --task 'ship safely' --goal 'Ship safely.' --criterion tested='Tests pass.'\n  podway start --preset sw-dev --task 'preview procedure' --dry-run"
        }
        "session.decide" => {
            "Usage:\n  podway decide --option <id> --reason <text> [--actor <text>] --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision <n> --if-attempt <uuid>\n\nExample:\n  podway decide --option approve --reason 'The evidence supports this route.' --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 7 --if-attempt <uuid>"
        }
        "session.rework" => {
            "Usage:\n  podway rework --to <graph-node-id> --reason <text> [--actor <text>] --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision <n> [--if-attempt <uuid>]\n\nExample:\n  podway rework --to implement --reason 'Review found a gap.' --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 7"
        }
        "goal.define" => {
            "Usage:\n  podway goal define --goal <text> --criterion <id>=<statement>... [--actor <text>] --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision <n>\n\nExample:\n  podway goal define --goal 'Ship safely.' --criterion tested='Tests pass.' --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 7"
        }
        "goal.revise" => {
            "Usage:\n  podway goal revise --goal <text> --criterion <id>=<statement>... --rework-to <graph-node-id> --reason <text> [--actor <text>] [--reactivate] --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision <n> --if-goal-revision <n> [--if-attempt <uuid>]\n\nExample:\n  podway goal revise --goal 'Ship after restart.' --criterion restart-safe='Restart passes.' --rework-to implement --reason 'Scope changed.' --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 7 --if-goal-revision 1"
        }
        "goal.assess_criterion" => {
            "Usage:\n  podway goal assess-criterion <criterion-id> --status <satisfied|unsatisfied|not_applicable> --reason <text> [--evidence <graph-node-id>]... [--item <item-id>]... [--actor <text>] --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision <n> --if-attempt <uuid> --if-goal-revision <n>\n\nExample:\n  podway goal assess-criterion tested --status satisfied --reason 'The test passed.' --evidence test --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 7 --if-attempt <uuid> --if-goal-revision 1"
        }
        "session.start_replace" => {
            "Usage:\n  podway start (--preset <name> | --procedure <file> [--expect-procedure-digest <sha256:hex>]) --task <title> --replace [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--dry-run] [--yes]\n  podway start (--preset <name> | --procedure <file> [--expect-procedure-digest <sha256:hex>]) --task <title> --goal <text> --criterion <id>=<statement>... [--actor <text>] --replace --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision <n> [--dry-run] [--yes]\n\nExamples:\n  podway start --preset sw-dev --task 'replace task' --replace --yes\n  podway start --procedure .podway/procedures/custom.yaml --expect-procedure-digest sha256:<hex> --task 'replace task' --replace --yes\n  podway start --procedure workflow.yaml --expect-procedure-digest sha256:<hex> --task 'replace goal' --goal 'Ship safely.' --criterion tested='Tests pass.' --replace --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 7 --yes\n  podway start --preset sw-dev --task 'preview replacement' --replace --dry-run"
        }
        "session.status" => {
            "Usage:\n  podway status [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--verbose] [--wait-for-idle [--compact] | --after-job <uuid>]\n\nExamples:\n  podway status --verbose\n  podway status --wait-for-idle --compact"
        }
        "session.next" => {
            "Usage:\n  podway next [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--wait-for-idle | --after-job <uuid>]\n\nExample:\n  podway next"
        }
        "session.complete" => {
            "Usage:\n  podway complete [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--if-attempt <uuid>]\n\nExample:\n  podway complete --if-session-revision 12 --if-attempt <uuid>"
        }
        "session.skip" => {
            "Usage:\n  podway skip [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--if-attempt <uuid>] [--reason <text>]\n\nExample:\n  podway skip --reason 'not applicable'"
        }
        "session.retry" => {
            "Usage:\n  podway retry --reason <text> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--if-attempt <uuid>]\n\nExample:\n  podway retry --reason 'rerun after fixing input'"
        }
        "session.return" => {
            "Usage:\n  podway return --to <stage-id> --reason <text> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--if-attempt <uuid>] [--dry-run]\n\nExample:\n  podway return --to implement --reason 'review found a gap' --dry-run"
        }
        "session.block" => {
            "Usage:\n  podway block --reason <text> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--if-attempt <uuid>]\n\nExample:\n  podway block --reason 'waiting for API owner'"
        }
        "session.unblock" => {
            "Usage:\n  podway unblock (<blocker-id> | --all) [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--if-attempt <uuid>]\n\nExample:\n  podway unblock --all"
        }
        "session.cancel" => {
            "Usage:\n  podway cancel --reason <text> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--if-attempt <uuid>]\n\nExample:\n  podway cancel --reason 'task no longer needed'"
        }
        "session.reopen" => {
            "Usage:\n  podway reopen --to <stage-id> --reason <text> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--dry-run]\n\nExample:\n  podway reopen --to implement --reason 'follow-up' --dry-run"
        }
        "session.reset" => {
            "Usage:\n  podway reset [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--dry-run] [--yes]\n\nExample:\n  podway reset --yes"
        }
        "workspace.reset_all" => {
            "Usage:\n  podway reset --all --force --yes [--if-workspace-uuid <uuid>]\n\nExample:\n  podway reset --all --force --yes"
        }
        "item.check" => {
            "Usage:\n  podway check <item-id> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-attempt <uuid>] [--if-item-revision <n>]\n\nExample:\n  podway check baseline-established"
        }
        "item.uncheck" => {
            "Usage:\n  podway uncheck <item-id> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-attempt <uuid>] [--if-item-revision <n>]\n\nExample:\n  podway uncheck baseline-established"
        }
        "item.set" => {
            "Usage:\n  podway set <item-id> (<value> | --stdin) [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-attempt <uuid>] [--if-item-revision <n>]\n\nExample:\n  podway set implementation-summary 'completed work'"
        }
        "item.add" => {
            "Usage:\n  podway add <item-id> <value> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-attempt <uuid>] [--if-item-revision <n>]\n\nExample:\n  podway add affected-components daemon"
        }
        "item.remove" => {
            "Usage:\n  podway remove <item-id> <value> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-attempt <uuid>] [--if-item-revision <n>] [--ignore-missing]\n\nExample:\n  podway remove affected-components daemon"
        }
        "item.attach" => {
            "Usage:\n  podway attach <item-id> (<path> [--media-type <type>] | --reference <ref> --digest <sha256> --size <bytes> --media-type <type>) [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-attempt <uuid>] [--if-item-revision <n>]\n\nExample:\n  podway attach verification-reference report.md --media-type text/markdown"
        }
        "item.clear" => {
            "Usage:\n  podway clear <item-id> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-attempt <uuid>] [--if-item-revision <n>]\n\nExample:\n  podway clear constraints"
        }
        "job.list" => {
            "Usage:\n  podway job list [--state <queued|running|succeeded|failed|cancelled>]\n\nExample:\n  podway job list --state queued"
        }
        "job.lookup" => {
            "Usage:\n  podway job lookup --idempotency-key <key>\n\nExample:\n  podway job lookup --idempotency-key mutation-123"
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
        ffi::OsString,
        fs::{self, File},
        io::{self, Seek, SeekFrom, Write},
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use super::{
        Cli, Command, LocalEnvelopeClock, LocalFailure, ParseFailureCommandContext,
        build_identity_v1, local_generated_at, local_result, map_service_error,
        parse_failure_command_context, parse_timeout_millis, probe_daemon_identity,
        probe_daemon_identity_with_runner, render_local_failure_with_clock_and_writers,
        render_result_with_clock_and_writers, resolve_daemon_executable,
        resolve_implicit_daemon_executable_from, resolve_installed_service_endpoint,
        service_outcome_result, service_status_result, stream_log_follow_update,
        system_service_clock,
    };
    use clap::{Parser, error::ErrorKind};
    use serde_json::json;
    static VERSION_PROBE_SCRIPT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct VersionProbeScript {
        directory: PathBuf,
        path: PathBuf,
    }

    impl VersionProbeScript {
        fn new(body: &str) -> Self {
            let sequence = VERSION_PROBE_SCRIPT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "podway-cli-version-probe-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&directory).expect("create version probe fixture directory");
            let path = directory.join("podwayd");
            fs::write(&path, format!("#!/bin/sh\n{body}"))
                .expect("write version probe fixture script");
            let mut permissions = fs::metadata(&path)
                .expect("read version probe fixture permissions")
                .permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&path, permissions)
                .expect("make version probe fixture script executable");
            Self { directory, path }
        }
    }

    impl Drop for VersionProbeScript {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn installed_endpoint_resolution_uses_only_private_durable_metadata() {
        use podway_service::ServiceRuntimePathsV1;

        let fixture = VersionProbeScript::new("exit 0");
        let state = fixture.directory.join("state");
        fs::create_dir(&state).expect("metadata parent must be created");
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700))
            .expect("metadata parent must be private");
        let paths = ServiceRuntimePathsV1::from_directories(
            fixture.directory.join("launch-agents"),
            &state,
            fixture.directory.join("logs"),
            fixture.directory.join("run"),
        )
        .expect("metadata fixture paths must be valid");
        let installed_socket = fixture.directory.join("installed.sock");
        let metadata = serde_json::to_vec(&json!({
            "version": 1,
            "label": "dev.podway.podwayd",
            "daemon_binary": fixture.path.display().to_string(),
            "daemon_identity": "0".repeat(64),
            "socket_path": installed_socket.display().to_string(),
            "artifact_role": "production_daemon",
            "installed_at": 1,
            "updated_at": 1,
            "publication_state": "receipt_durable",
            "generation": "1".repeat(64),
        }))
        .expect("metadata fixture must serialize");
        let metadata_path = paths.metadata_index_path().as_path();
        fs::write(metadata_path, &metadata).expect("metadata fixture must be written");
        fs::set_permissions(metadata_path, fs::Permissions::from_mode(0o600))
            .expect("metadata fixture must be private");

        let resolved = resolve_installed_service_endpoint(paths.clone(), "cli")
            .expect("private durable metadata must select its endpoint");
        assert_eq!(resolved.socket_path().as_path(), installed_socket);

        fs::set_permissions(metadata_path, fs::Permissions::from_mode(0o644))
            .expect("insecure metadata mode fixture must be installed");
        assert!(resolve_installed_service_endpoint(paths, "cli").is_err());
    }

    #[test]
    fn daemon_identity_probes_are_bounded_and_fail_closed() {
        use podway_core::UnixMillis;
        use podway_service::{
            ServiceErrorV1, ServiceInstallMetadataV1, ServiceRunningV1, ServiceRuntimePathsV1,
            ServiceStatusV1, ServiceStoppedV1, SystemLaunchctlRunnerV1,
        };

        let expected = build_identity_v1();
        let root = std::env::temp_dir().join(format!(
            "podway-cli-status-{}-{}",
            std::process::id(),
            VERSION_PROBE_SCRIPT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let runtime = root.join("runtime");
        fs::create_dir_all(&runtime).expect("status fixture runtime");
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
            .expect("status fixture runtime must be private");
        let paths = ServiceRuntimePathsV1::from_directories(
            root.join("LaunchAgents"),
            root.join("ApplicationSupport"),
            root.join("Logs"),
            &runtime,
        )
        .expect("service status fixture paths");
        let valid_envelope = json!({
            "schema": "podway.output/v1",
            "request_id": "123e4567-e89b-42d3-a456-426614174000",
            "command": "version",
            "generated_at": "2026-08-03T00:00:00.000Z",
            "result": expected,
            "warnings": [],
        });
        let success = VersionProbeScript::new(&format!(
            "printf '%s\\n' '{}'",
            serde_json::to_string(&valid_envelope).expect("valid identity fixture serializes")
        ));
        let observed = probe_daemon_identity(&success.path).expect("valid probe output");
        assert_eq!(observed.product, expected.product());
        assert_eq!(observed.version, expected.version());
        assert_eq!(
            observed.contract_manifest_digest,
            expected.contract_manifest_digest()
        );

        let malformed_v011 = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/contract/v0.1.1-daemon-version-output.json"
        ));
        let malformed_v011 =
            VersionProbeScript::new(&format!("printf '%s' '{}'", malformed_v011.trim_end()));
        assert!(
            probe_daemon_identity(&malformed_v011.path).is_err(),
            "the exact v0.1.1 daemon identity must fail for missing result.schema"
        );

        let mut malformed = Vec::new();
        malformed.push(("bare result", valid_envelope["result"].clone()));
        let mut wrong_result_schema = valid_envelope.clone();
        wrong_result_schema["result"]["schema"] = json!("podway.status-result/v1");
        malformed.push(("wrong result schema", wrong_result_schema));
        let mut unknown_result_field = valid_envelope.clone();
        unknown_result_field["result"]["unknown"] = json!(true);
        malformed.push(("unknown result field", unknown_result_field));
        let mut missing_result_field = valid_envelope.clone();
        missing_result_field["result"]
            .as_object_mut()
            .expect("identity result object")
            .remove("target");
        malformed.push(("missing result field", missing_result_field));
        let mut wrong_outer_schema = valid_envelope.clone();
        wrong_outer_schema["schema"] = json!("podway.error/v1");
        malformed.push(("wrong outer schema", wrong_outer_schema));
        let mut wrong_command = valid_envelope.clone();
        wrong_command["command"] = json!("daemon.status");
        malformed.push(("wrong command", wrong_command));
        for (name, value) in malformed {
            let fixture = VersionProbeScript::new(&format!(
                "printf '%s\\n' '{}'",
                serde_json::to_string(&value).expect("malformed fixture serializes")
            ));
            assert!(
                probe_daemon_identity(&fixture.path).is_err(),
                "probe must reject {name}"
            );
        }
        let installed = ServiceInstallMetadataV1::new(
            &success.path,
            paths.socket_path().as_path(),
            UnixMillis::new(1),
            UnixMillis::new(1),
        )
        .expect("installed daemon fixture metadata");
        let stopped = service_status_result(
            "daemon.status",
            ServiceStatusV1::StoppedV1(ServiceStoppedV1::new(UnixMillis::new(1), Some(installed))),
            &paths,
        )
        .expect("stopped installed daemon status");
        match stopped {
            super::RunResult::Local { result, .. } => {
                assert_eq!(result["schema"], "podway.daemon-status-result/v1");
                assert_eq!(result["status"], "stopped");
                assert_eq!(result["product"], expected.product());
                assert_eq!(
                    result["contract_manifest_digest"],
                    expected.contract_manifest_digest()
                );
                assert_eq!(
                    result["configured_socket_path"],
                    paths.socket_path().as_path().display().to_string()
                );
                assert!(result["process_id"].is_null());
                assert!(result["pid"].is_null());
                assert!(result["started_at"].is_null());
                assert!(result["effective_socket_path"].is_null());
            }
            _ => panic!("stopped daemon status must remain a local result"),
        }
        fs::remove_dir_all(root).expect("remove status fixture root");

        for body in [
            "printf '{}\\n'; exit 7",
            "kill -TERM $$",
            "i=0; while [ \"$i\" -lt 5000 ]; do printf x; i=$((i + 1)); done",
            "i=0; while [ \"$i\" -lt 5000 ]; do printf x >&2; i=$((i + 1)); done",
            "printf '{}\\n'; printf unexpected >&2",
            "printf '{}\\n{}\\n'",
            "printf '\\377\\n'",
            "(sleep 10) & printf '{}\\n'",
        ] {
            let fixture = VersionProbeScript::new(body);
            assert!(
                probe_daemon_identity(&fixture.path).is_err(),
                "probe must reject script: {body}"
            );
            let metadata = ServiceInstallMetadataV1::new(
                &fixture.path,
                "/tmp/podwayd.sock",
                UnixMillis::new(1),
                UnixMillis::new(1),
            )
            .expect("version probe fixture metadata");
            assert_eq!(
                service_status_result(
                    "daemon.status",
                    ServiceStatusV1::RunningV1(ServiceRunningV1::new(
                        UnixMillis::new(1),
                        Some(42),
                        Some(metadata),
                    )),
                    &paths,
                )
                .err()
                .expect("invalid probe must fail daemon status")
                .code,
                "DAEMON_UNAVAILABLE"
            );
        }

        let stalled = VersionProbeScript::new("sleep 10");
        let stalled_runner = SystemLaunchctlRunnerV1::new(&stalled.path).with_bounds(
            Duration::from_millis(100),
            4 * 1024,
            Duration::from_millis(100),
        );
        assert!(matches!(
            probe_daemon_identity_with_runner(&stalled_runner),
            Err(ServiceErrorV1::LaunchctlTimeoutV1 { timeout_ms: 100 })
        ));
    }

    #[test]
    fn parser_accepts_canonical_session_start_and_attachment_forms() {
        let terminate = Cli::try_parse_from(["podway", "--dev", "terminate"]).unwrap();
        assert!(terminate.dev);
        assert!(matches!(terminate.command, Command::Terminate));
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
    fn parser_failure_context_recovers_routes_and_admission_semantics() {
        let context = |arguments: &[&str]| {
            let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
            parse_failure_command_context(&arguments)
        };

        assert_eq!(
            context(&["podway", "start", "--preset", "sw-dev"]),
            Some(ParseFailureCommandContext::new("session.start", true))
        );
        assert_eq!(
            context(&["podway", "start", "--preset", "sw-dev", "--replace"]),
            Some(ParseFailureCommandContext::new(
                "session.start_replace",
                true
            ))
        );
        assert_eq!(
            context(&["podway", "start", "--preset", "sw-dev", "--dry-run"]),
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
            context(&["podway", "job", "lookup", "--unknown"]),
            Some(ParseFailureCommandContext::new("job.lookup", false))
        );
        assert_eq!(context(&["podway", "unknown-command"]), None);
    }

    #[test]
    fn daemon_status_help_flag_is_rejected_without_dispatch() {
        let error = Cli::try_parse_from(["podway", "daemon", "status", "--help"])
            .expect_err("the disabled help flag must stop in clap");
        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn timeout_parser_accepts_documented_units_only() {
        assert_eq!(parse_timeout_millis("500ms"), Ok(500));
        assert_eq!(parse_timeout_millis("30s"), Ok(30_000));
        assert_eq!(parse_timeout_millis("2m"), Ok(120_000));
        assert!(parse_timeout_millis("30").is_err());
        assert!(parse_timeout_millis("1h").is_err());
    }
    #[test]
    fn service_clock_failures_are_explicit_and_command_scoped() {
        let before_epoch = UNIX_EPOCH
            .checked_sub(Duration::from_millis(1))
            .expect("Unix epoch must support a preceding instant");
        let failure = system_service_clock(before_epoch, "daemon.start")
            .expect_err("pre-epoch service clock must fail");
        assert_eq!(failure.code, "INTERNAL_ERROR");
        assert_eq!(failure.command, "daemon.start");
        assert!(failure.message.contains("before the Unix epoch"));
    }

    #[test]
    fn log_follow_reopens_the_active_path_after_rotation() {
        let directory = std::env::temp_dir().join(format!(
            "podway-cli-log-follow-{}-{}",
            std::process::id(),
            VERSION_PROBE_SCRIPT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).expect("create log fixture directory");
        let path = directory.join("podwayd.log");
        let rotated = directory.join("podwayd.log.1");
        fs::write(&path, b"initial\n").expect("write initial log");
        let mut file = File::open(&path).expect("open active log");
        let mut offset = file.seek(SeekFrom::End(0)).expect("seek active log");
        let mut output = Vec::new();

        let mut append = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open active log for append");
        append.write_all(b"append-one\n").expect("append first log");
        stream_log_follow_update(&path, &mut file, &mut offset, &mut output)
            .expect("read appended log");

        fs::rename(&path, &rotated).expect("rotate active log");
        fs::write(&path, b"new-active\n").expect("recreate active log");
        stream_log_follow_update(&path, &mut file, &mut offset, &mut output)
            .expect("read recreated active log");

        let mut append = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open recreated log for append");
        append
            .write_all(b"append-two\n")
            .expect("append second log");
        stream_log_follow_update(&path, &mut file, &mut offset, &mut output)
            .expect("read append after rotation");

        assert_eq!(output, b"append-one\nnew-active\nappend-two\n");
        drop(file);
        fs::remove_dir_all(directory).expect("remove log fixture directory");
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
    #[test]
    fn daemon_service_results_and_errors_use_local_contracts() {
        use podway_core::UnixMillis;
        use podway_service::{
            ServiceChangedV1, ServiceErrorV1, ServiceNotInstalledV1, ServiceOutcomeV1,
            ServiceRunningV1, ServiceRuntimePathsV1, ServiceStatusV1,
        };
        let root = std::env::temp_dir().join(format!(
            "podway-cli-local-status-{}-{}",
            std::process::id(),
            VERSION_PROBE_SCRIPT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let runtime = root.join("runtime");
        fs::create_dir_all(&runtime).expect("local status fixture runtime");
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
            .expect("local status fixture runtime must be private");
        let paths = ServiceRuntimePathsV1::from_directories(
            root.join("LaunchAgents"),
            root.join("ApplicationSupport"),
            root.join("Logs"),
            &runtime,
        )
        .expect("service status fixture paths");

        let changed = service_outcome_result(
            "daemon.start",
            ServiceOutcomeV1::ChangedV1(ServiceChangedV1::new(UnixMillis::new(1), None)),
        );
        let status = service_status_result(
            "daemon.status",
            ServiceStatusV1::RunningV1(ServiceRunningV1::new(UnixMillis::new(1), Some(42), None)),
            &paths,
        )
        .expect("status without daemon metadata must remain available");

        match changed {
            super::RunResult::Local {
                command, result, ..
            } => {
                assert_eq!(command, "daemon.start");
                assert_eq!(result["outcome"], "changed");
            }
            _ => panic!("service outcomes must remain local results"),
        }
        match status {
            super::RunResult::Local {
                command, result, ..
            } => {
                assert_eq!(command, "daemon.status");
                assert_eq!(result["status"], "running");
                assert!(result["process_id"].is_null());
                assert!(result["pid"].is_null());
                assert_eq!(result["reachable"], false);
            }
            _ => panic!("service status must remain local results"),
        }

        let not_installed = map_service_error(
            ServiceErrorV1::LogUnavailableV1 {
                message: "missing".to_owned(),
            },
            "daemon.logs",
        );
        assert_eq!(not_installed.code, "DAEMON_NOT_INSTALLED");
        let unavailable = map_service_error(
            ServiceErrorV1::IoV1 {
                operation: None,
                message: "launchctl missing".to_owned(),
            },
            "daemon.start",
        );
        assert_eq!(unavailable.code, "DAEMON_UNAVAILABLE");
        let mismatch = map_service_error(
            ServiceErrorV1::ContractMismatchV1 {
                expected_product: "podway".to_owned(),
                actual_product: Some("other".to_owned()),
                expected_manifest_digest: "sha256:expected".to_owned(),
                actual_manifest_digest: Some("sha256:actual".to_owned()),
            },
            "daemon.install",
        );
        assert_eq!(mismatch.code, "DAEMON_CONTRACT_MISMATCH");
        assert_eq!(mismatch.exit_code, 3);
        assert!(!mismatch.retryable);
        assert_eq!(mismatch.details["expected"]["product"], "podway");
        assert_eq!(mismatch.details["actual"]["product"], "other");
        assert_eq!(mismatch.details["admission"]["admitted"], false);

        let malformed_identity = map_service_error(
            ServiceErrorV1::ContractMismatchV1 {
                expected_product: "podway".to_owned(),
                actual_product: None,
                expected_manifest_digest: format!("sha256:{}", "a".repeat(64)),
                actual_manifest_digest: None,
            },
            "daemon.install",
        );
        assert_eq!(malformed_identity.code, "DAEMON_VERSION_INCOMPATIBLE");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            render_local_failure_with_clock_and_writers(
                malformed_identity,
                true,
                &FixedClock(UNIX_EPOCH + Duration::from_millis(12)),
                &mut stdout,
                &mut stderr,
            ),
            3
        );
        serde_json::from_slice::<podway_protocol::ResponseEnvelopeV1>(&stdout)
            .expect("local malformed-identity output must remain protocol-valid");
        assert!(stderr.is_empty());

        let stopped = service_status_result(
            "daemon.status",
            ServiceStatusV1::NotInstalledV1(ServiceNotInstalledV1::new(UnixMillis::new(1))),
            &paths,
        )
        .expect("not-installed status must remain available");
        match stopped {
            super::RunResult::Local { result, .. } => {
                assert_eq!(result["status"], "not_installed");
            }
            _ => panic!("service status must remain local results"),
        }
        fs::remove_dir_all(root).expect("remove local status fixture root");
    }
    #[test]
    fn daemon_install_path_resolution_rejects_non_platform_paths() {
        let fixture = VersionProbeScript::new("exit 0");
        let binary = resolve_daemon_executable(Some(&fixture.path), "daemon.install")
            .expect("existing absolute daemon path must be accepted");
        assert_eq!(
            binary.as_path(),
            std::fs::canonicalize(&fixture.path).expect("fixture path must canonicalize")
        );

        let failure =
            resolve_daemon_executable(Some(std::path::Path::new("podwayd")), "daemon.install")
                .expect_err("relative daemon path must be rejected");
        assert_eq!(failure.code, "REQUEST_INVALID");
        assert_eq!(failure.command, "daemon.install");

        let path_fixture = VersionProbeScript::new("exit 0");
        let from_path = resolve_implicit_daemon_executable_from(
            std::path::Path::new("/bin/sh"),
            Some(path_fixture.directory.as_os_str()),
            "daemon.install",
        )
        .expect("controlled PATH daemon must be selected after a missing sibling");
        assert_eq!(
            from_path.as_path(),
            std::fs::canonicalize(&path_fixture.path).expect("PATH fixture canonical path")
        );

        let cli = fixture.directory.join("podway");
        std::fs::write(&cli, "#!/bin/sh\nexit 0\n").expect("CLI fixture must be written");
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700))
            .expect("CLI fixture must be executable");
        let sibling = resolve_implicit_daemon_executable_from(
            &cli,
            Some(path_fixture.directory.as_os_str()),
            "daemon.install",
        )
        .expect("resolved CLI sibling must precede PATH");
        assert_eq!(
            sibling.as_path(),
            std::fs::canonicalize(&fixture.path).expect("sibling fixture canonical path")
        );

        let symlink_directory = path_fixture.directory.join("cli-link");
        std::fs::create_dir(&symlink_directory).expect("CLI symlink directory must be created");
        let cli_symlink = symlink_directory.join("podway");
        std::os::unix::fs::symlink(&cli, &cli_symlink).expect("CLI symlink must be created");
        let symlink_sibling = resolve_implicit_daemon_executable_from(
            &cli_symlink,
            Some(path_fixture.directory.as_os_str()),
            "daemon.install",
        )
        .expect("resolved CLI symlink sibling must precede PATH");
        assert_eq!(
            symlink_sibling.as_path(),
            std::fs::canonicalize(&fixture.path).expect("symlink sibling canonical path")
        );
    }
}

#[cfg(test)]
mod phase6_health_tests {
    use std::{
        fs,
        io::{Read, Write},
        net::Shutdown,
        os::unix::fs::PermissionsExt,
        os::unix::net::UnixListener,
        path::Path,
        time::Duration,
    };

    use podway_core::UnixMillis;
    use podway_protocol::{
        build_identity_v1, decode_request_payload_v1, decode_single_frame_v1, encode_frame_v1,
    };
    use podway_service::{ServiceRunningV1, ServiceRuntimePathsV1, ServiceStatusV1};

    use super::{service_status_result, wait_for_verified_service};

    #[test]
    fn raw_socket_listener_is_not_verified_daemon_health() {
        let runtime =
            std::env::temp_dir().join(format!("podway-cli-health-{}", std::process::id()));
        let _ = fs::remove_dir_all(&runtime);
        fs::create_dir_all(&runtime).expect("health fixture runtime");
        fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))
            .expect("health fixture runtime must be private");
        let paths = ServiceRuntimePathsV1::from_directories(
            runtime.join("LaunchAgents"),
            runtime.join("ApplicationSupport"),
            runtime.join("Logs"),
            &runtime,
        )
        .expect("health fixture service paths");
        let listener =
            UnixListener::bind(paths.socket_path().as_path()).expect("health fixture socket");
        fs::set_permissions(
            paths.socket_path().as_path(),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("health fixture socket must be private");

        let responder = std::thread::spawn(move || {
            for _ in 0..2 {
                let (stream, _) = listener.accept().expect("health probe connection");
                drop(stream);
            }
        });
        assert!(
            wait_for_verified_service(
                &paths,
                Path::new("/Applications/Podway/podwayd"),
                "daemon.install",
                Duration::from_millis(50),
            )
            .is_err(),
            "a listener that does not complete the identity handshake is not healthy"
        );
        let result = service_status_result(
            "daemon.status",
            ServiceStatusV1::RunningV1(ServiceRunningV1::new(UnixMillis::new(1), Some(42), None)),
            &paths,
        )
        .expect("status without daemon metadata must remain available");
        match result {
            super::RunResult::Local { result, .. } => {
                assert_eq!(result["reachable"], false);
                assert_eq!(result["loaded"], true);
            }
            _ => panic!("service status must remain local"),
        }

        responder.join().expect("health fixture responder");
        fs::remove_dir_all(runtime).expect("remove health fixture");
    }

    #[test]
    fn readiness_waits_through_stale_identity_for_a_new_daemon_process() {
        let runtime = std::env::temp_dir().join(format!(
            "podway-cli-upgrade-readiness-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&runtime);
        fs::create_dir_all(&runtime).expect("upgrade fixture directory must be created");
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
            .expect("upgrade fixture directory must be private");
        let paths = ServiceRuntimePathsV1::from_directories(
            runtime.join("LaunchAgents"),
            runtime.join("ApplicationSupport"),
            runtime.join("Logs"),
            &runtime,
        )
        .expect("upgrade fixture paths");
        let listener =
            UnixListener::bind(paths.socket_path().as_path()).expect("upgrade fixture socket");
        fs::set_permissions(
            paths.socket_path().as_path(),
            fs::Permissions::from_mode(0o600),
        )
        .expect("upgrade fixture socket must be private");

        let expected_binary = runtime.join("installed/podwayd");
        let responder_paths = paths.clone();
        let responder_binary = expected_binary.clone();
        let stale_process_id = "123e4567-e89b-42d3-a456-426614174001";
        let current_process_id = "123e4567-e89b-42d3-a456-426614174002";
        let responder = std::thread::spawn(move || {
            let identity = build_identity_v1();
            for (product, process_id) in [
                ("stale-podway", stale_process_id),
                (identity.product(), current_process_id),
            ] {
                let (mut connection, _) = listener.accept().expect("readiness connection");
                let mut wire = Vec::new();
                connection
                    .read_to_end(&mut wire)
                    .expect("readiness request must be readable");
                let request = decode_request_payload_v1(
                    decode_single_frame_v1(&wire).expect("readiness request frame"),
                )
                .expect("readiness request payload");
                let response = serde_json::json!({
                    "schema": "podway.output/v1",
                    "request_id": request.request_id().as_str(),
                    "command": request.command().as_str(),
                    "generated_at": "2026-07-25T00:00:00.000Z",
                    "result": {
                        "schema": "podway.daemon-status-result/v1",
                        "product": product,
                        "daemon_version": identity.version(),
                        "target": identity.target(),
                        "build_identity": identity.build_identity(),
                        "source_commit": identity.source_commit(),
                        "contract_manifest_schema": identity.contract_manifest_schema(),
                        "contract_manifest_digest": identity.contract_manifest_digest(),
                        "protocol_versions": identity.supported_ipc_ids(),
                        "pid": 4242,
                        "process_id": process_id,
                        "executable_path": responder_binary,
                        "started_at": "2026-07-25T00:00:00.000Z",
                        "uptime_ms": 1,
                        "configured_socket_path": responder_paths.socket_path().as_path(),
                        "effective_socket_path": responder_paths.socket_path().as_path(),
                    },
                    "warnings": [],
                });
                let frame = encode_frame_v1(
                    &serde_json::to_vec(&response).expect("readiness response must serialize"),
                )
                .expect("readiness response frame");
                connection
                    .write_all(&frame)
                    .expect("readiness response must be written");
                connection
                    .shutdown(Shutdown::Write)
                    .expect("readiness response must finish");
            }
            (stale_process_id, current_process_id)
        });

        wait_for_verified_service(
            &paths,
            expected_binary.as_path(),
            "daemon.install",
            Duration::from_secs(2),
        )
        .expect("readiness must recover after the stale daemon is replaced");
        let (observed_stale, observed_current) = responder.join().expect("readiness responder");
        assert_ne!(
            observed_stale, observed_current,
            "the accepted daemon must have a new process UUID"
        );
        fs::remove_dir_all(runtime).expect("upgrade fixture must be removed");
    }
}
