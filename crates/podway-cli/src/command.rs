//! Public command-line surface for the Podway command contract.

mod completion;
mod direct;
mod retirement;

use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self},
    io::{self, IsTerminal, Read, Write},
    os::{
        fd::OwnedFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use clap::{ArgAction, ArgMatches, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use nix::{
    errno::Errno,
    fcntl::{OFlag, open, openat, renameat},
    sys::stat::{Mode, fchmod, mode_t},
    unistd::{UnlinkatFlags, fsync, unlinkat},
};
use podway_config::{
    AuthoringContext, AuthoringStage, ConfigError, FormatFailure, FormatRequest,
    FormattedProcedureV2, MAX_PROCEDURE_DOCUMENT_BYTES, ParsedProcedure, ParsedProcedureV2,
    ProcedureDocumentFormat, ScaffoldTemplate, check_procedure_v2, config_error_diagnostic,
    finalize_diagnostics, format_procedure_v2, lint_procedure_v2, normalize_procedure_v2_graph,
    parse_procedure_document, preview_procedure_v2, project_procedure_v2_dot,
    project_procedure_v2_graph, project_procedure_v2_mermaid, project_procedure_v2_plantuml,
    scaffold_procedure_v2, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{
    ActorAttributionV2, AttemptId, CriterionAssessmentReasonV2, CriterionId, GoalCriterionV2,
    GoalDefinitionV2, GoalRevisionNumberV2, GoalRevisionReasonV2, GoalStatementV2, GraphNodeId,
    GraphPlacementV2, ItemConditionV2, ItemId, ItemPredicateOperatorV2, OptionGuardV2, OptionId,
    PROCEDURE_SCHEMA_V2, PredicateScalarV2, ReasonV2, Revision, SessionId, Sha256Digest,
    WorkspaceId,
};
use podway_presets::{PresetError, catalog_v2};
use podway_protocol::{
    CommandNameV1, ItemRecordManyInputV1, MAX_DIRECT_INPUT_BYTES_V1,
    MAX_SLICE_ITEM_TEXT_SCALARS_V1, RequestIdV1, Rfc3339MillisV1, build_identity_v1,
    decode_item_record_many_input_v1, ensure_procedure_independent_result_schema_v1,
    validate_command_result_v2, validate_procedure_independent_result_v1,
};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use completion::Shell;

const MAX_DIRECT_COMMAND_TIMEOUT_MILLIS_V1: u64 = 3_600_000;
const LOCAL_USAGE_EXIT: i32 = 2;
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

    /// Bound the complete direct operation, including lock acquisition.
    #[arg(long, global = true, value_name = "DURATION", value_parser = parse_timeout_millis)]
    timeout: Option<u64>,

    /// Disable text color. Podway v1 currently emits uncolored text.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    no_color: bool,

    /// Suppress nonessential text output.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    quiet: bool,

    /// Reuse this idempotency key instead of generating a UUID-v4 key.
    #[arg(long, global = true, value_name = "KEY", allow_hyphen_values = true)]
    idempotency_key: Option<String>,

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
    Begin(BeginArgs),
    Status(StatusArgs),
    Next(ReadArgs),
    Observe(ReadArgs),
    /// Read one bounded page of a selected evidence item.
    Evidence {
        #[command(subcommand)]
        command: EvidenceCommand,
    },
    /// Read delegated work or record a durable acceptance.
    Delegation {
        #[command(subcommand)]
        command: DelegationCommand,
    },
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
    Disposition {
        #[command(subcommand)]
        command: DispositionCommand,
    },
    Archive {
        #[command(subcommand)]
        command: Option<ArchiveCommand>,
    },
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
    /// Atomically record or clear a bounded set of typed item values from JSON stdin.
    Record(RecordArgs),
    /// Look up the durable outcome of a direct mutation.
    Outcome {
        #[command(subcommand)]
        command: OutcomeCommand,
    },
    /// Internal best-effort candidates for generated shell completion scripts.
    #[command(name = "__complete", hide = true)]
    CompleteDynamic {
        #[arg(default_value = "items")]
        kind: String,
    },
}

#[derive(Debug, Subcommand)]
enum OutcomeCommand {
    Lookup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "snake_case")]
enum ExistingSessionPolicy {
    Preserve,
    Delete,
}

#[derive(Clone, Debug)]
enum StartTerminalDisposition {
    HandedOff {
        summary: String,
        reference: String,
        actor: Option<String>,
    },
    NotRequired {
        reason: String,
        actor: Option<String>,
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
    #[arg(long, value_enum, value_name = "POLICY")]
    on_existing: Option<ExistingSessionPolicy>,
    #[arg(long, value_name = "TEXT")]
    supersede_reason: Option<String>,
    #[arg(long, value_name = "TEXT")]
    actor: Option<String>,
    #[arg(long, value_name = "TEXT")]
    progress_summary: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    dry_run: bool,
    #[arg(skip)]
    auto_archive_terminal: bool,
    #[arg(skip)]
    terminal_disposition: Option<StartTerminalDisposition>,
    #[arg(skip)]
    delete_confirmed: bool,
}

#[derive(Debug, Args)]
struct BeginArgs {
    #[arg(long, value_name = "TEXT", requires = "criterion")]
    goal: Option<String>,
    #[arg(long, value_name = "ID=STATEMENT", action = ArgAction::Append, requires = "goal")]
    criterion: Vec<String>,
    #[arg(long, value_name = "TEXT", requires = "goal")]
    actor: Option<String>,
}

#[derive(Debug, Subcommand)]
enum DispositionCommand {
    HandedOff {
        #[arg(long, value_name = "TEXT")]
        summary: String,
        #[arg(long, value_name = "TEXT")]
        reference: String,
        #[arg(long, value_name = "TEXT")]
        actor: Option<String>,
    },
    NotRequired {
        #[arg(long, value_name = "TEXT")]
        reason: String,
        #[arg(long, value_name = "TEXT")]
        actor: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum EvidenceCommand {
    /// Read one bounded page of an item selected by a declared evidence reference.
    Read(EvidenceReadArgs),
}

#[derive(Debug, Args)]
struct EvidenceReadArgs {
    #[arg(long, value_name = "GRAPH_NODE_ID")]
    source: String,
    #[arg(long, value_name = "ITEM_ID")]
    item: String,
    #[arg(long, value_name = "TOKEN")]
    page_token: Option<String>,
}

#[derive(Debug, Args, Default)]
struct ReadArgs {
    #[arg(long, action = ArgAction::SetTrue)]
    verbose: bool,
}

#[derive(Debug, Args, Default)]
struct StatusArgs {
    #[command(flatten)]
    read: ReadArgs,
    #[arg(
        long,
        action = ArgAction::SetTrue,
        conflicts_with = "verbose"
    )]
    compact: bool,
    #[arg(
        long,
        value_name = "TRACE_SEQUENCE",
        value_parser = clap::value_parser!(u64).range(1..),
        requires = "verbose",
        conflicts_with = "compact"
    )]
    history_before: Option<u64>,
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
struct ResetArgs {
    #[arg(long, action = ArgAction::SetTrue)]
    all: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    force: bool,
    #[arg(long, value_name = "TEXT")]
    progress_summary: Option<String>,
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

#[derive(Debug, Args)]
struct RecordArgs {
    #[arg(long, required = true, action = ArgAction::SetTrue)]
    stdin: bool,
    #[arg(skip)]
    input: Option<ItemRecordManyInputV1>,
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
enum WorkspaceCommand {
    Show,
    Repair,
    Remove {
        #[arg(long, required = true, action = ArgAction::SetTrue)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ArchiveCommand {
    List,
    Show {
        #[arg(long, value_name = "UUID")]
        session_id: String,
        #[arg(long, action = ArgAction::SetTrue)]
        verbose: bool,
        #[arg(long, value_name = "N", requires = "verbose")]
        history_before: Option<u64>,
    },
    Purge {
        #[arg(long, value_name = "UUID")]
        session_id: String,
        #[arg(skip)]
        expected_session_revision: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
enum DelegationCommand {
    /// Read the immutable work package for the current delegated attempt.
    Package,
    /// Record final evidence and settle a delegated attempt from bounded JSON stdin.
    Accept(DelegationAcceptArgs),
    /// Read one immutable receipt from a current or retained session.
    Receipt {
        #[arg(long, value_name = "SESSION_ID")]
        session: String,
        #[arg(long, value_name = "ATTEMPT_ID")]
        attempt: String,
    },
}

#[derive(Debug, Args)]
struct DelegationAcceptArgs {
    #[arg(long, required = true, action = ArgAction::SetTrue)]
    stdin: bool,
    #[arg(skip)]
    input: Option<Value>,
}

impl Command {
    const fn direct_route(&self) -> Option<&'static str> {
        match self {
            Self::Init { .. } => Some("workspace.init"),
            Self::Doctor { .. } => Some("workspace.doctor"),
            Self::Workspace {
                command: WorkspaceCommand::Show,
            } => Some("workspace.show"),
            Self::Workspace {
                command: WorkspaceCommand::Repair,
            } => Some("workspace.repair"),
            Self::Workspace {
                command: WorkspaceCommand::Remove { .. },
            } => Some("workspace.remove"),
            Self::Start(args)
                if args.on_existing.is_some()
                    || args.auto_archive_terminal
                    || args.terminal_disposition.is_some() =>
            {
                Some("session.start_replace")
            }
            Self::Start(_) => Some("session.start"),
            Self::Begin(_) => Some("session.begin"),
            Self::Status(_) | Self::CompleteDynamic { .. } => Some("session.status"),
            Self::Next(_) => Some("session.next"),
            Self::Observe(_) => Some("session.observe"),
            Self::Evidence { .. } => Some("evidence.read"),
            Self::Delegation {
                command: DelegationCommand::Package,
            } => Some("delegation.package"),
            Self::Delegation {
                command: DelegationCommand::Receipt { .. },
            } => Some("delegation.receipt"),
            Self::Delegation {
                command: DelegationCommand::Accept(_),
            } => Some("delegation.accept"),
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
            Self::Block { .. } => Some("session.block"),
            Self::Unblock { .. } => Some("session.unblock"),
            Self::Cancel { .. } => Some("session.cancel"),
            Self::Disposition { .. } => Some("session.terminal_disposition"),
            Self::Reset(args) if args.all => Some("workspace.reset_all"),
            Self::Reset(_) => Some("session.reset"),
            Self::Archive { command: None } => Some("session.archive"),
            Self::Archive {
                command: Some(ArchiveCommand::List),
            } => Some("session.archive_list"),
            Self::Archive {
                command: Some(ArchiveCommand::Show { .. }),
            } => Some("session.archive_show"),
            Self::Archive {
                command: Some(ArchiveCommand::Purge { .. }),
            } => Some("session.archive_purge"),
            Self::Check { .. } => Some("item.check"),
            Self::Uncheck { .. } => Some("item.uncheck"),
            Self::Set(_) => Some("item.set"),
            Self::Add { .. } => Some("item.add"),
            Self::Remove { .. } => Some("item.remove"),
            Self::Attach(_) => Some("item.attach"),
            Self::Clear { .. } => Some("item.clear"),
            Self::Record(_) => Some("item.record_many"),
            Self::Outcome { .. } => Some("outcome.lookup"),
            Self::Help { .. }
            | Self::Version { .. }
            | Self::Completions { .. }
            | Self::Procedure { .. }
            | Self::Preset { .. } => None,
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
            Self::Preset {
                command: PresetCommand::List,
            } => "preset.list",
            Self::Preset {
                command: PresetCommand::Show { .. },
            } => "preset.show",
            Self::Preset {
                command: PresetCommand::Explain { .. },
            } => "preset.explain",
            Self::CompleteDynamic { .. } => "__complete",
            command => command
                .direct_route()
                .expect("non-local commands have a canonical wire route"),
        }
    }

    const fn is_mutation(&self) -> bool {
        match self {
            Self::Init { .. }
            | Self::Complete
            | Self::Begin(_)
            | Self::Decide(_)
            | Self::Rework(_)
            | Self::Goal { .. }
            | Self::Skip { .. }
            | Self::Retry { .. }
            | Self::Block { .. }
            | Self::Unblock { .. }
            | Self::Cancel { .. }
            | Self::Disposition { .. }
            | Self::Archive { command: None }
            | Self::Check { .. }
            | Self::Uncheck { .. }
            | Self::Set(_)
            | Self::Add { .. }
            | Self::Remove { .. }
            | Self::Attach(_)
            | Self::Clear { .. }
            | Self::Record(_) => true,
            Self::Delegation {
                command: DelegationCommand::Accept(_),
            }
            | Self::Workspace {
                command: WorkspaceCommand::Repair | WorkspaceCommand::Remove { .. },
            }
            | Self::Archive {
                command: Some(ArchiveCommand::Purge { .. }),
            } => true,
            Self::Start(args) => !args.dry_run,
            Self::Reset(args) => !args.dry_run,
            _ => false,
        }
    }

    const fn needs_preflight(&self) -> bool {
        match self {
            Self::Complete
            | Self::Begin(_)
            | Self::Decide(_)
            | Self::Rework(_)
            | Self::Goal { .. }
            | Self::Skip { .. }
            | Self::Retry { .. }
            | Self::Block { .. }
            | Self::Unblock { .. }
            | Self::Cancel { .. }
            | Self::Disposition { .. }
            | Self::Archive { command: None }
            | Self::Check { .. }
            | Self::Uncheck { .. }
            | Self::Set(_)
            | Self::Add { .. }
            | Self::Remove { .. }
            | Self::Attach(_)
            | Self::Clear { .. }
            | Self::Record(_) => true,
            Self::Start(args) => {
                args.on_existing.is_some()
                    || args.auto_archive_terminal
                    || args.terminal_disposition.is_some()
            }
            Self::Reset(args) => !args.all,
            Self::Archive {
                command: Some(ArchiveCommand::Purge { .. }),
            } => true,
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
                | Self::Begin(_)
                | Self::Status(_)
                | Self::Next(_)
                | Self::Observe(_)
                | Self::Evidence { .. }
                | Self::Delegation { .. }
                | Self::Outcome { .. }
                | Self::Complete
                | Self::Decide(_)
                | Self::Rework(_)
                | Self::Goal { .. }
                | Self::Skip { .. }
                | Self::Retry { .. }
                | Self::Block { .. }
                | Self::Unblock { .. }
                | Self::Cancel { .. }
                | Self::Disposition { .. }
                | Self::Archive { .. }
                | Self::Reset(_)
                | Self::Workspace {
                    command: WorkspaceCommand::Remove { .. },
                }
                | Self::Check { .. }
                | Self::Uncheck { .. }
                | Self::Set(_)
                | Self::Add { .. }
                | Self::Remove { .. }
                | Self::Attach(_)
                | Self::Clear { .. }
                | Self::Record(_)
        )
    }

    const fn accepts_session_identity(&self) -> bool {
        match self {
            Self::Start(args) => {
                args.on_existing.is_some()
                    || args.auto_archive_terminal
                    || args.terminal_disposition.is_some()
            }
            Self::Begin(_)
            | Self::Status(_)
            | Self::Next(_)
            | Self::Observe(_)
            | Self::Evidence { .. }
            | Self::Delegation {
                command: DelegationCommand::Package | DelegationCommand::Accept(_),
            }
            | Self::Complete
            | Self::Decide(_)
            | Self::Rework(_)
            | Self::Goal { .. }
            | Self::Skip { .. }
            | Self::Retry { .. }
            | Self::Block { .. }
            | Self::Unblock { .. }
            | Self::Cancel { .. }
            | Self::Disposition { .. }
            | Self::Archive { command: None }
            | Self::Reset(ResetArgs { all: false, .. })
            | Self::Check { .. }
            | Self::Uncheck { .. }
            | Self::Set(_)
            | Self::Add { .. }
            | Self::Remove { .. }
            | Self::Attach(_)
            | Self::Clear { .. }
            | Self::Record(_) => true,
            _ => false,
        }
    }

    const fn is_destructive(&self) -> bool {
        match self {
            Self::Start(args) => {
                !args.dry_run && matches!(args.on_existing, Some(ExistingSessionPolicy::Delete))
            }
            Self::Reset(args) => !args.dry_run && (args.all || args.progress_summary.is_some()),
            Self::Workspace {
                command: WorkspaceCommand::Remove { .. },
            } => true,
            Self::Archive {
                command: Some(ArchiveCommand::Purge { .. }),
            } => true,
            _ => false,
        }
    }

    const fn is_dry_run(&self) -> bool {
        matches!(
            self,
            Self::Start(StartArgs { dry_run: true, .. })
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

#[derive(Clone, Debug)]
struct StatusFacts {
    session_id: SessionId,
    session_revision: Revision,
    attempt_id: Option<AttemptId>,
    item_revisions: Vec<(String, Revision)>,
    goal_revision: Option<GoalRevisionNumberV2>,
    lifecycle: String,
}

#[derive(Clone, Debug)]
struct StatusPreflight {
    transport_workspace_id: WorkspaceId,
    facts: StatusFacts,
}

impl StatusPreflight {
    fn from_direct(output: &podway_protocol::OutputEnvelopeV4) -> Result<Self, LocalFailure> {
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

    fn from_parts(
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
        "init" => ParseFailureCommandContext::new("workspace.init", true),
        "doctor" => ParseFailureCommandContext::new("workspace.doctor", false),
        "workspace" => nested_parse_failure_context(
            matches,
            &[
                ("show", "workspace.show"),
                ("repair", "workspace.repair"),
                ("remove", "workspace.remove"),
            ],
        )?,
        "start" => ParseFailureCommandContext::new(
            if matches.contains_id("on_existing") {
                "session.start_replace"
            } else {
                "session.start"
            },
            !matches.get_flag("dry_run"),
        ),
        "begin" => ParseFailureCommandContext::new("session.begin", true),
        "status" => ParseFailureCommandContext::new("session.status", false),
        "next" => ParseFailureCommandContext::new("session.next", false),
        "observe" => ParseFailureCommandContext::new("session.observe", false),
        "evidence" => ParseFailureCommandContext::new("evidence.read", false),
        "delegation" => {
            let route = nested_parse_failure_context(
                matches,
                &[
                    ("package", "delegation.package"),
                    ("accept", "delegation.accept"),
                    ("receipt", "delegation.receipt"),
                ],
            )?
            .route;
            ParseFailureCommandContext::new(route, route == "delegation.accept")
        }
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
        "block" => ParseFailureCommandContext::new("session.block", true),
        "unblock" => ParseFailureCommandContext::new("session.unblock", true),
        "cancel" => ParseFailureCommandContext::new("session.cancel", true),
        "disposition" => ParseFailureCommandContext::new("session.terminal_disposition", true),
        "archive" => match matches.subcommand() {
            None => ParseFailureCommandContext::new("session.archive", true),
            Some(("list", _)) => ParseFailureCommandContext::new("session.archive_list", false),
            Some(("show", _)) => ParseFailureCommandContext::new("session.archive_show", false),
            Some(("purge", _)) => ParseFailureCommandContext::new("session.archive_purge", true),
            _ => return None,
        },
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
        "record" => ParseFailureCommandContext::new("item.record_many", true),
        "outcome" => nested_parse_failure_context(matches, &[("lookup", "outcome.lookup")])?,
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
            match execute(cli) {
                Ok(result) => render_result(&result, json_output, quiet),
                Err(failure) => render_local_failure(failure.with_command(route), json_output),
            }
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

fn render_retirement(retired: retirement::RetiredSurface, json_output: bool) -> i32 {
    let guidance = match &retired.replacement {
        Some(replacement) => format!("This command is retired. Use {replacement} instead."),
        None => "This command or flag is retired. No replacement exists.".to_owned(),
    };
    if !json_output {
        return if writeln!(io::stderr().lock(), "error: COMMAND_RETIRED: {guidance}").is_ok() {
            LOCAL_USAGE_EXIT
        } else {
            LOCAL_CLIENT_EXIT
        };
    }
    let generated_at = match local_generated_at(&SystemLocalEnvelopeClock) {
        Ok(timestamp) => timestamp,
        Err(failure) => return render_clock_failure_to(failure, &mut io::stderr().lock()),
    };
    let value = json!({
        "schema": "podway.error/v2",
        "request_id": Uuid::new_v4().to_string(),
        "command": retired.command,
        "generated_at": generated_at.as_str(),
        "code": "COMMAND_RETIRED",
        "message": guidance,
        "retryable": false,
        "exit_code": LOCAL_USAGE_EXIT,
        "details": {
            "schema": "podway.command-retired-details/v1",
            "retired": retired.name,
            "replacement": retired.replacement,
            "guidance": guidance,
        },
    });
    let Ok(error) = podway_protocol::ErrorEnvelopeV2::from_value(value) else {
        return LOCAL_CLIENT_EXIT;
    };
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &error).is_err() || writeln!(stdout).is_err() {
        LOCAL_CLIENT_EXIT
    } else {
        LOCAL_USAGE_EXIT
    }
}

fn normalize_top_level_help_alias(arguments: &mut [OsString]) {
    let mut help_index = None;
    for (index, argument) in arguments.iter().enumerate().skip(1) {
        match argument.as_os_str() {
            value if value == OsStr::new("-h") || value == OsStr::new("--help") => {
                if help_index.replace(index).is_some() {
                    return;
                }
            }
            value
                if value == OsStr::new("--json")
                    || value == OsStr::new("--no-color")
                    || value == OsStr::new("--quiet") => {}
            _ => return,
        }
    }
    if let Some(index) = help_index {
        arguments[index] = OsString::from("help");
    }
}

fn execute(cli: Cli) -> Result<RunResult, LocalFailure> {
    direct::execute(cli)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExistingStartResolution {
    Continue,
    Replace,
}

fn resolve_existing_start(
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

fn execute_start_dry_run(cli: &Cli) -> Result<RunResult, LocalFailure> {
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

fn procedure_format(path: &Path) -> ProcedureDocumentFormat {
    if path.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    }
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
    if metadata.len() > MAX_PROCEDURE_DOCUMENT_BYTES as u64 {
        return Err(LocalFailure::procedure_invalid(
            "procedure exceeds the maximum document size",
        ));
    }
    let mode = permission_bits(&metadata);
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_PROCEDURE_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LocalFailure::procedure_not_found("cannot read procedure file"))?;
    if bytes.len() > MAX_PROCEDURE_DOCUMENT_BYTES {
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
        || cli.idempotency_key.is_some()
        || cli.if_workspace_uuid.is_some()
        || cli.if_session_id.is_some()
        || cli.if_session_revision.is_some()
        || cli.if_attempt.is_some()
        || cli.if_item_revision.is_some()
        || cli.yes;
    match &cli.command {
        Command::Help { topic } => {
            reject_local_flags(local_flags, "help accepts no worktree request flags")?;
            let text = help_text(topic.as_deref())?;
            Ok(Some(local_result(
                "help",
                json!({ "topic": topic, "text": text }),
                text,
            )))
        }
        Command::Version { identity } => {
            reject_local_flags(local_flags, "version accepts no worktree request flags")?;
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
            reject_local_flags(local_flags, "completions accepts no worktree request flags")?;
            Ok(Some(local_result(
                "completions",
                json!({ "shell": format!("{shell:?}").to_lowercase(), "script": shell.script() }),
                shell.script(),
            )))
        }
        Command::Preset { command } => {
            reject_local_flags(
                local_flags,
                "preset commands accept no worktree request flags",
            )?;
            Ok(Some(execute_preset(command)?))
        }
        Command::Procedure { command } => {
            reject_local_flags(
                local_flags,
                "procedure commands accept no worktree request flags",
            )?;
            Ok(Some(execute_procedure(command)?))
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
    let mut result = result
        .as_object()
        .cloned()
        .expect("local result is always an object");
    ensure_procedure_independent_result_schema_v1(command, &mut result);
    validate_procedure_independent_result_v1(command, &result)
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
    if result.get("schema").and_then(Value::as_str) == Some("podway.session-start-result/v4") {
        assert!(
            podway_protocol::validate_daemon_free_document_v1(
                "podway.session-start-result/v4",
                &Value::Object(result.clone())
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
    RunResult::LocalV2 {
        command: command.to_owned(),
        result,
        text,
        exit_code,
    }
}

fn execute_preset(command: &PresetCommand) -> Result<RunResult, LocalFailure> {
    match command {
        PresetCommand::List => {
            let presets: Vec<Value> = catalog_v2()
                .list()
                .iter()
                .map(|preset| preset.metadata)
                .map(|metadata| {
                    json!({ "id": metadata.id, "name": metadata.name, "version": metadata.version, "description": metadata.description })
                })
                .collect();
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
            let preset = catalog_v2().lookup(name).ok_or_else(|| {
                LocalFailure::preset_not_found("unknown preset").with_command("preset.show")
            })?;
            let admitted = preset
                .validate()
                .map_err(|error| preset_failure(error).with_command("preset.show"))?;
            let procedure: Value = serde_json::from_str(admitted.canonical_json().as_str())
                .map_err(|_| {
                    LocalFailure::procedure_invalid(
                        "embedded Procedure v2 canonical JSON is invalid",
                    )
                    .with_command("preset.show")
                })?;
            Ok(local_result(
                "preset.show",
                json!({
                    "preset": preset.metadata.id,
                    "metadata": preset.metadata,
                    "digest": admitted.digest().as_str(),
                    "procedure": procedure,
                    "warnings": [],
                }),
                preset.yaml.to_owned(),
            ))
        }
        PresetCommand::Explain { name } => {
            let preset = catalog_v2().lookup(name).ok_or_else(|| {
                LocalFailure::preset_not_found("unknown preset").with_command("preset.explain")
            })?;
            let admitted = preset
                .validate()
                .map_err(|error| preset_failure(error).with_command("preset.explain"))?;
            let document: Value = serde_json::from_str(admitted.canonical_json().as_str())
                .map_err(|_| {
                    LocalFailure::procedure_invalid(
                        "embedded Procedure v2 canonical JSON is invalid",
                    )
                    .with_command("preset.explain")
                })?;
            let definitions = document["node_definitions"]
                .as_object()
                .expect("validated Procedure v2 has node definitions");
            let nodes: Vec<Value> = document["graph"]["nodes"]
                .as_array()
                .expect("validated Procedure v2 has graph nodes")
                .iter()
                .map(|node| {
                    let definition = node["use"]
                        .as_str()
                        .and_then(|id| definitions.get(id))
                        .expect("validated graph node resolves its definition");
                    json!({
                        "id": node["id"],
                        "title": definition["title"],
                        "type": definition["type"],
                    })
                })
                .collect();
            let text = format!(
                "{}\n{}\nGraph nodes: {}",
                preset.metadata.name,
                preset.metadata.description,
                nodes
                    .iter()
                    .filter_map(|node| node.get("id").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            Ok(local_result(
                "preset.explain",
                json!({ "preset": preset.metadata, "nodes": nodes }),
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
    };
    let bytes = read_offline_procedure(file).map_err(|failure| failure.with_command(name))?;
    std::str::from_utf8(&bytes).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(name)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    if matches!(command, ProcedureCommand::Validate { .. }) {
        return Ok(execute_procedure_validate_v2(file, &bytes, format));
    }
    debug_assert!(!warnings_as_errors);
    let ParsedProcedure::V2(parsed) = parse_procedure_document(&bytes, format)
        .map_err(|error| procedure_config_failure(error).with_command(name))?;
    let validated = validate_procedure_v2(parsed)
        .map_err(|error| procedure_config_failure(error).with_command(name))?;
    let result = json!({
        "file": file.display().to_string(),
        "digest": validated.digest().as_str(),
        "canonical_json": validated.canonical_json().as_str(),
        "procedure_schema": PROCEDURE_SCHEMA_V2,
    });
    let text = match command {
        ProcedureCommand::Validate { .. } => format!(
            "{} ({})",
            validated.parsed().name(),
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
        | ProcedureCommand::Scaffold { .. } => {
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
    format: ProcedureDocumentFormat,
) -> RunResult {
    let source = std::str::from_utf8(bytes)
        .expect("execute_procedure rejects a non-UTF-8 document before dispatching");
    let source_path = file.display().to_string();
    let context = AuthoringContext::new(&source_path, source, format);

    let admitted = match parse_procedure_document(bytes, format) {
        Ok(ParsedProcedure::V2(parsed)) => validate_procedure_v2(parsed),
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
        _ => render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total()),
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
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
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
            "procedure format requires a podway.procedure/v2 document",
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
    let text =
        render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total());
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
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    let source_path = file.display().to_string();

    let context = AuthoringContext::new(&source_path, source, format);
    let parsed = match parse_procedure_document(source.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
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
        render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total())
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
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    let source_path = file.display().to_string();

    let context = AuthoringContext::new(&source_path, source, format);
    let parsed = match parse_procedure_document(source.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
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
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-graph-result/v1",
        "procedure_schema": PROCEDURE_SCHEMA_V2,
        "procedure_digest": validated.digest().as_str(),
        "format": result_format,
        "projection_digest": projection_digest.as_str(),
        "projection": projection,
    }) else {
        unreachable!("the static graph result is a JSON object");
    };
    let delegated = delegated_placements_v1(validated.parsed());
    if !delegated.is_empty() {
        result.insert(
            "schema".to_owned(),
            json!("podway.procedure-graph-result/v2"),
        );
        result.insert(
            "delegated_placement_count".to_owned(),
            json!(delegated.len()),
        );
        result.insert("delegated_placements".to_owned(), json!(delegated));
    }
    Ok(local_result_v2(PROCEDURE_GRAPH_COMMAND, result, text, 0))
}

fn delegated_placements_v1(procedure: &ParsedProcedureV2) -> Vec<String> {
    procedure
        .graph()
        .placements()
        .iter()
        .filter_map(|placement| match placement {
            GraphPlacementV2::Action(action) if action.delegation().is_some() => {
                Some(action.id().as_str().to_owned())
            }
            _ => None,
        })
        .collect()
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
    let text =
        render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total());
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
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
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
    if let Ok(ParsedProcedure::V2(parsed)) = parse_procedure_document(source.as_bytes(), format) {
        let delegated = delegated_placements_v1(&parsed);
        if !delegated.is_empty() {
            result.insert(
                "schema".to_owned(),
                json!("podway.procedure-preview-result/v2"),
            );
            result.insert(
                "delegated_placement_count".to_owned(),
                json!(delegated.len()),
            );
            result.insert("delegated_placements".to_owned(), json!(delegated));
        }
    }

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
                    "items": node.items().iter().map(|item| {
                        let mut value = Map::new();
                        value.insert("item_id".to_owned(), json!(item.item_id()));
                        value.insert("required".to_owned(), json!(item.required()));
                        if let Some(condition) = item.required_when() {
                            value.insert("required_when".to_owned(), condition_json(condition));
                        }
                        Value::Object(value)
                    }).collect::<Vec<_>>(),
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
                if let Some(guards) = edge.guards() {
                    edge_value["guards"] = guards_json(guards);
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

        let mut text = render_authoring_diagnostics(
            report.diagnostics(),
            report.diagnostics_truncated(),
            report.diagnostics_total(),
        );
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
        render_authoring_diagnostics(
            report.diagnostics(),
            report.diagnostics_truncated(),
            report.diagnostics_total(),
        )
    };
    let exit_code = i32::from(!report.admissible());
    Ok(local_result_v2(NAME, result, text, exit_code))
}

fn condition_json(condition: &ItemConditionV2) -> Value {
    Value::Array(
        condition
            .predicates()
            .iter()
            .map(|predicate| {
                let mut value = Map::new();
                value.insert("item".to_owned(), json!(predicate.item().as_str()));
                if predicate.field_outcome() {
                    value.insert("field".to_owned(), json!("outcome"));
                }
                match predicate.operator() {
                    ItemPredicateOperatorV2::Equals(expected) => {
                        value.insert("equals".to_owned(), predicate_scalar_json(expected));
                    }
                    ItemPredicateOperatorV2::NotEquals(expected) => {
                        value.insert("not_equals".to_owned(), predicate_scalar_json(expected));
                    }
                    ItemPredicateOperatorV2::Empty => {
                        value.insert("empty".to_owned(), json!(true));
                    }
                    ItemPredicateOperatorV2::NonEmpty => {
                        value.insert("non_empty".to_owned(), json!(true));
                    }
                    ItemPredicateOperatorV2::AtLeast(expected) => {
                        value.insert("at_least".to_owned(), json!(expected));
                    }
                    ItemPredicateOperatorV2::AtMost(expected) => {
                        value.insert("at_most".to_owned(), json!(expected));
                    }
                }
                Value::Object(value)
            })
            .collect(),
    )
}

fn predicate_scalar_json(value: &PredicateScalarV2) -> Value {
    match value {
        PredicateScalarV2::Boolean(value) => json!(value),
        PredicateScalarV2::Integer(value) => json!(value),
        PredicateScalarV2::Text(value) => json!(value),
    }
}

fn guards_json(guards: &OptionGuardV2) -> Value {
    Value::Array(
        guards
            .predicates()
            .iter()
            .map(|predicate| {
                let mut value = Map::new();
                value.insert(
                    "evidence".to_owned(),
                    json!({
                        "node": predicate.source_node().as_str(),
                        "item": predicate.item().as_str(),
                    }),
                );
                if predicate.field_outcome() {
                    value.insert("field".to_owned(), json!("outcome"));
                }
                match predicate.operator() {
                    ItemPredicateOperatorV2::Equals(expected) => {
                        value.insert("equals".to_owned(), predicate_scalar_json(expected));
                    }
                    ItemPredicateOperatorV2::NotEquals(expected) => {
                        value.insert("not_equals".to_owned(), predicate_scalar_json(expected));
                    }
                    ItemPredicateOperatorV2::Empty => {
                        value.insert("empty".to_owned(), json!(true));
                    }
                    ItemPredicateOperatorV2::NonEmpty => {
                        value.insert("non_empty".to_owned(), json!(true));
                    }
                    ItemPredicateOperatorV2::AtLeast(expected) => {
                        value.insert("at_least".to_owned(), json!(expected));
                    }
                    ItemPredicateOperatorV2::AtMost(expected) => {
                        value.insert("at_most".to_owned(), json!(expected));
                    }
                }
                Value::Object(value)
            })
            .collect(),
    )
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
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    let source_path = file.display().to_string();

    let context = AuthoringContext::new(&source_path, source, format);
    let parsed = match parse_procedure_document(source.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
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
        render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total())
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
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    let source_path = file.display().to_string();

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
        render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total())
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
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(document.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap_or_else(|error| unreachable!("a scaffold template must parse: {error}"));
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

/// The stable one-line-per-finding authoring report.
///
/// The format is `<source_path>:<line>:<column> <severity> <code> <message>`, mirroring the
/// position-first convention every editor's error parser already understands. The machine-readable
/// form — locations, hints, and graph identities included — is the JSON result.
fn render_authoring_diagnostics(
    diagnostics: &[podway_core::AuthoringDiagnostic],
    truncated: bool,
    total: u32,
) -> String {
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
    if truncated {
        text.push_str(&format!(
            "diagnostics truncated: showing {} of {total}\n",
            diagnostics.len(),
        ));
    }
    text
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
    if explicit.any_transition()
        && !command.needs_preflight()
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
    if !cli.command.is_destructive() || cli.yes {
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
    if millis > MAX_DIRECT_COMMAND_TIMEOUT_MILLIS_V1 {
        return Err(format!(
            "duration exceeds the maximum of {MAX_DIRECT_COMMAND_TIMEOUT_MILLIS_V1}ms"
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
        RunResult::Direct {
            response,
            procedure_check_path,
        } => {
            let value = match &**response {
                podway_engine::direct_api::DirectResponseV1::Success(output) => output.as_value(),
                podway_engine::direct_api::DirectResponseV1::Failure(error) => error.as_value(),
            };
            if json_output {
                if serde_json::to_writer(&mut *stdout, value).is_err() || writeln!(stdout).is_err()
                {
                    return LOCAL_CLIENT_EXIT;
                }
            } else {
                let rendered = match &**response {
                    podway_engine::direct_api::DirectResponseV1::Success(output) if !quiet => {
                        render_human_output_direct(output, stdout)
                    }
                    podway_engine::direct_api::DirectResponseV1::Failure(error) => {
                        render_human_error_direct(error, procedure_check_path.as_deref(), stderr)
                    }
                    _ => Ok(()),
                };
                if rendered.is_err() {
                    return LOCAL_CLIENT_EXIT;
                }
            }
            match &**response {
                podway_engine::direct_api::DirectResponseV1::Success(_) => 0,
                podway_engine::direct_api::DirectResponseV1::Failure(error) => {
                    error.as_value()["exit_code"]
                        .as_i64()
                        .unwrap_or(i64::from(LOCAL_CLIENT_EXIT)) as i32
                }
            }
        }
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
                let Ok(output) = podway_protocol::OutputEnvelopeV4::new(
                    podway_protocol::OutputEnvelopeInputV4 {
                        request_id,
                        command,
                        generated_at,
                        workspace: None,
                        session: None,
                        result: result.clone(),
                        warnings: Vec::new(),
                    },
                ) else {
                    return LOCAL_CLIENT_EXIT;
                };
                let output = output.into_value();
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
                let Ok(output) = podway_protocol::OutputEnvelopeV4::new(
                    podway_protocol::OutputEnvelopeInputV4 {
                        request_id,
                        command,
                        generated_at,
                        workspace: None,
                        session: None,
                        result: result.clone(),
                        warnings: Vec::new(),
                    },
                ) else {
                    return LOCAL_CLIENT_EXIT;
                };
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
    }
}

fn render_clock_failure_to(failure: LocalFailure, stderr: &mut dyn Write) -> i32 {
    if writeln!(stderr, "error: {}", failure.message).is_err() {
        LOCAL_CLIENT_EXIT
    } else {
        failure.exit_code
    }
}

fn render_human_error_direct(
    error: &podway_protocol::ErrorEnvelopeV2,
    procedure_check_path: Option<&str>,
    stderr: &mut dyn Write,
) -> Result<(), LocalFailure> {
    let error = error.as_value();
    write_text_line(
        stderr,
        format_args!(
            "error: {}: {}",
            error["code"].as_str().unwrap_or_default(),
            error["message"].as_str().unwrap_or_default()
        ),
    )?;
    if error["code"]
        .as_str()
        .unwrap_or_default()
        .starts_with("DELEGATION_")
        && let Some(reason) = error["details"].get("reason").and_then(Value::as_str)
    {
        write_text_line(stderr, format_args!("reason: {reason}"))?;
    }
    if error["code"].as_str().unwrap_or_default() != "PROCEDURE_V2_SCHEMA_INVALID" {
        return Ok(());
    }

    if let Some(codes) = error["details"]
        .get("diagnostic_codes")
        .and_then(Value::as_array)
    {
        let rendered = codes
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        if !rendered.is_empty() {
            write_text_line(stderr, format_args!("diagnostics: {rendered}"))?;
        }
    }
    if let Some(path) = procedure_check_path {
        let hint = render_preview_start_command(&[
            "podway".to_owned(),
            "procedure".to_owned(),
            "check".to_owned(),
            "--".to_owned(),
            path.to_owned(),
        ]);
        write_text_line(
            stderr,
            format_args!("hint: {hint} for field-level diagnostics and remediation guidance"),
        )?;
    }
    Ok(())
}

fn render_human_output_direct(
    output: &podway_protocol::OutputEnvelopeV4,
    stdout: &mut dyn Write,
) -> Result<(), LocalFailure> {
    let value = output.as_value();
    if let Some(workspace) = value.get("workspace") {
        write_text_line(
            stdout,
            format_args!(
                "workspace: {} {} sequence={}",
                workspace["uuid"].as_str().unwrap_or_default(),
                workspace["root"].as_str().unwrap_or_default(),
                workspace["latest_workspace_sequence"]
            ),
        )?;
    }
    if let Some(session) = value.get("session") {
        write_text_line(
            stdout,
            format_args!(
                "session: {} {} {} revision={}",
                session["id"].as_str().unwrap_or_default(),
                session["title"].as_str().unwrap_or_default(),
                session["lifecycle"].as_str().unwrap_or_default(),
                session["revision_after"]
            ),
        )?;
    }
    let result = &value["result"];
    if contains_check_result_projection_v1(result) {
        write_text_line(
            stdout,
            format_args!(
                "note: check_result is a caller-supplied, structurally bound external check result; Podway does not verify or attest it"
            ),
        )?;
    }
    for line in unsatisfied_check_result_lines_v1(result) {
        write_text_line(stdout, format_args!("{line}"))?;
    }
    if result["schema"].as_str() == Some("podway.workspace-removal-result/v2") {
        write_text_line(
            stdout,
            format_args!(
                "Podway workspace state was removed from {}. The Git worktree was preserved; tracked .podway paths may now appear deleted.",
                result["worktree_root"]
                    .as_str()
                    .unwrap_or("the selected worktree")
            ),
        )?;
    }
    let rendered = serde_json::to_string_pretty(result)
        .map_err(|_| LocalFailure::response_invalid("cannot render Procedure v2 result"))?;
    write_text_line(stdout, format_args!("result: {rendered}"))?;
    for warning in value["warnings"].as_array().into_iter().flatten() {
        write_text_line(stdout, format_args!("warning: {warning}"))?;
    }
    Ok(())
}

fn contains_check_result_projection_v1(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.get("type").and_then(Value::as_str) == Some("check_result")
                || ([
                    "operation_id",
                    "operation_digest",
                    "input_basis",
                    "outcome",
                    "output_digest",
                ]
                .into_iter()
                .all(|field| object.contains_key(field)))
                || object.values().any(contains_check_result_projection_v1)
        }
        Value::Array(values) => values.iter().any(contains_check_result_projection_v1),
        _ => false,
    }
}

/// Human-readable lines naming why each stored check result in one result stays unsatisfied.
///
/// The machine `unsatisfied_reason` field stays authoritative; these lines only place the declared
/// and recorded identity side by side so a caller sees the exact mismatch without comparing
/// digests by eye.
fn unsatisfied_check_result_lines_v1(value: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    collect_unsatisfied_check_result_lines_v1(value, &mut lines);
    lines
}

fn collect_unsatisfied_check_result_lines_v1(value: &Value, lines: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("check_result")
                && let Some(reason) = object.get("unsatisfied_reason").and_then(Value::as_str)
            {
                lines.push(unsatisfied_check_result_line_v1(object, reason));
            }
            object
                .values()
                .for_each(|nested| collect_unsatisfied_check_result_lines_v1(nested, lines));
        }
        Value::Array(values) => values
            .iter()
            .for_each(|nested| collect_unsatisfied_check_result_lines_v1(nested, lines)),
        _ => {}
    }
}

fn unsatisfied_check_result_line_v1(item: &Map<String, Value>, reason: &str) -> String {
    let item_id = item
        .get("item_id")
        .and_then(Value::as_str)
        .unwrap_or("<item>");
    let text = |source: &str, field: &str| -> String {
        item.get(source)
            .and_then(|value| value.get(field))
            .map(|value| match value {
                Value::String(text) => text.clone(),
                Value::Array(values) => values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","),
                other => other.to_string(),
            })
            .unwrap_or_else(|| "<unknown>".to_owned())
    };
    let detail = match reason {
        "operation_id_mismatch" => format!(
            "declared operation_id {}; recorded {}",
            text("constraints", "operation_id"),
            text("value", "operation_id")
        ),
        "operation_digest_mismatch" => format!(
            "declared operation_digest {}; recorded {}",
            text("constraints", "operation_digest"),
            text("value", "operation_digest")
        ),
        "outcome_not_accepted" => format!(
            "accepted outcomes {}; recorded {}",
            text("constraints", "accepted_outcomes"),
            text("value", "outcome")
        ),
        _ => "see the machine result".to_owned(),
    };
    format!("unsatisfied check_result {item_id}: {reason}; {detail}")
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
        let generated_at = match local_generated_at(clock) {
            Ok(timestamp) => timestamp,
            Err(clock_failure) => return render_clock_failure_to(clock_failure, stderr),
        };
        let request_id = failure
            .request_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let output = json!({ "schema": "podway.error/v2", "request_id": request_id, "command": failure.command, "generated_at": generated_at.as_str(), "code": failure.code, "message": failure.message, "retryable": failure.retryable, "exit_code": failure.exit_code, "details": failure.details });
        let output = match podway_protocol::ErrorEnvelopeV2::from_value(output) {
            Ok(output) => output,
            Err(_) => {
                failure = LocalFailure::response_invalid(
                    "the local client could not construct a valid error response",
                );
                let fallback = json!({
                    "schema": "podway.error/v2",
                    "request_id": Uuid::new_v4().to_string(),
                    "command": "cli",
                    "generated_at": generated_at.as_str(),
                    "code": failure.code,
                    "message": failure.message,
                    "retryable": failure.retryable,
                    "exit_code": failure.exit_code,
                    "details": {}
                });
                podway_protocol::ErrorEnvelopeV2::from_value(fallback)
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

fn help_text(topic: Option<&str>) -> Result<String, LocalFailure> {
    let topic = topic.unwrap_or("overview");
    if topic == "overview" {
        let preset_ids = podway_presets::list()
            .iter()
            .map(|preset| preset.metadata.id)
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(format!(
            "Podway coordinates durable worktree-local procedures.\n\nTrust boundary:\n  Podway trusts same-user processes reading and writing local workspace state.\n  It provides no authentication or workspace access key.\n  It does not protect against malicious same-user processes.\n  It records caller assertions and does not judge their semantic truth.\n\nDirect execution:\n  Worktree commands execute in this process.\n  Use --worktree <path> to select a Git worktree and --timeout <duration> to bound the operation.\n  Automation mutations require an explicit --idempotency-key; recover lost output with outcome lookup.\n\nUsage:\n  podway help [<route>]\n  podway -h\n  podway --help\n\nExamples:\n  podway start --preset sw-dev-v2 --task 'add retry backoff' --idempotency-key start-example\n  podway begin --idempotency-key begin-example\n  podway status --json\n  podway observe --json\n\nProcedure v2 routes:\n  procedure format|vet|lint|check|graph|preview|scaffold\n  session.begin, session.terminal_disposition, session.decide, session.rework\n  goal.define, goal.revise, goal.assess_criterion\n  evidence read\n  delegation package|accept|receipt\n\nShipped presets: {preset_ids}. Start prepares a session; begin creates its first active attempt. Contributors use disposable Git worktrees for isolated development; see help workflow."
        ));
    }
    if let Some(guidance) = retirement::help_guidance(topic)
        .map_err(|_| LocalFailure::internal("the embedded retirement table is invalid"))?
    {
        return Ok(guidance);
    }
    let text = match topic {
        "workflow" => {
            "Procedure v2 workflow:\n  podway init --idempotency-key init-example\n  podway --json preset show bug-fix-v2\n  podway --json start --preset bug-fix-v2 --task 'inspect the workflow' --idempotency-key start-example\n  podway --json begin --goal 'Verify the requested behavior.' --criterion verified='Relevant checks pass.' --actor developer --idempotency-key begin-example\n  podway --json observe\n\nAdd --dry-run to start for read-only Procedure inspection without creating a session. A successful start creates a prepared session; begin atomically creates the first attempt and optional goal. Follow the allowed item, option, evidence, and rework identifiers returned by observe. Contributors use a disposable Git worktree with its own .podway state. See docs/examples/v2-workflow.md for the complete executable sequence. Podway records caller assertions; it does not establish their semantic truth."
        }
        "rework" => "Rework:\n  podway rework --to implement --reason 'review found a gap'",
        "automation" => {
            "Automation:\n  podway complete --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 12 --if-attempt <uuid> --idempotency-key task-42 --json"
        }
        "procedures" => {
            "Procedures:\n  podway procedure scaffold > .podway/procedures/custom.yaml\n  podway procedure format .podway/procedures/custom.yaml --write\n  podway procedure check .podway/procedures/custom.yaml --warnings-as-errors\n  podway procedure preview .podway/procedures/custom.yaml\n  podway start --procedure .podway/procedures/custom.yaml --expect-procedure-digest sha256:<hex> --task 'perform work'\n\nOther Procedure v2 authoring routes are procedure validate, vet, lint, and graph."
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
        "preset.list" => "Usage:\n  podway preset list\n\nExample:\n  podway preset list",
        "preset.show" => {
            "Usage:\n  podway preset show <name>\n\nExample:\n  podway preset show sw-dev-v2"
        }
        "preset.explain" => {
            "Usage:\n  podway preset explain <name>\n\nExample:\n  podway preset explain sw-dev-v2"
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
        "workspace.remove" => {
            "Usage:\n  podway [--worktree <path>] workspace remove --force [--if-workspace-uuid <uuid>] [--yes]\n\nInteractive use prints the resolved absolute worktree root and requires that exact path as confirmation. JSON and non-interactive use require --yes and --if-workspace-uuid. Success removes all .podway content, including tracked files, while preserving the Git worktree. The direct CLI owns no global daemon logs."
        }
        "session.start" => {
            "Usage:\n  podway start (--preset <name> | --procedure <file> [--expect-procedure-digest <sha256:hex>]) --task <title> [--on-existing <preserve|delete>] [--supersede-reason <text>] [--actor <text>] [--progress-summary <text>] [--dry-run]\n\nExamples:\n  podway start --preset sw-dev-v2 --task 'implement feature'\n  podway start --preset sw-dev-v2 --task 'start successor' --on-existing preserve --supersede-reason 'The successor has priority.'\n  podway start --preset sw-dev-v2 --task 'replace running work' --on-existing delete --progress-summary 'The current progress was reviewed.' --yes\n  podway start --preset bug-fix-v2 --task 'preview deletion' --dry-run --on-existing delete\n\nHuman TTY mode interviews when a nonterminal session already exists; Enter continues it and creates nothing. Noninteractive callers must choose preserve or delete. Disposed terminal sessions archive automatically. A successful start creates a prepared session at revision 0 without a cursor, attempt, or goal."
        }
        "session.begin" => {
            "Usage:\n  podway begin [--goal <text> --criterion <id>=<statement>...] [--actor <text>] [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision 0]\n\nExamples:\n  podway begin\n  podway begin --goal 'Ship safely.' --criterion tested='Tests pass.' --actor developer --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 0\n\nBegin atomically creates attempt 1 at the entry node, optionally creates goal revision 1, and changes the prepared session to running. Omitted fences are filled from a fresh status preflight."
        }
        "session.decide" => {
            "Usage:\n  podway decide --option <id> --reason <text> [--actor <text>] --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision <n> --if-attempt <uuid> [--if-goal-revision <n>]\n\nThe goal revision fence is required for a session-goal assessment decision and, when supplied for a general decision, must match the current goal.\n\nExample:\n  podway decide --option approve --reason 'The evidence supports this route.' --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 7 --if-attempt <uuid>"
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
            "The session.start_replace wire route remains available for compatible automation, but the CLI has no replace command or replace flags. Use `podway start --on-existing preserve|delete`; see `podway help session.start`."
        }
        "session.status" => {
            "Usage:\n  podway status [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--verbose [--history-before <trace-sequence>]] [--compact]\n\nExamples:\n  podway status --verbose\n  podway status --verbose --history-before 42\n  podway status --compact"
        }
        "session.next" => {
            "Usage:\n  podway next [--if-workspace-uuid <uuid>] [--if-session-id <uuid>]\n\nExample:\n  podway next"
        }
        "session.observe" => {
            "Usage:\n  podway observe [--if-workspace-uuid <uuid>] [--if-session-id <uuid>]\n\nReturns one bounded, self-contained status and guidance observation with typed active-item descriptors and fenced mutation templates.\n\nExample:\n  podway --json observe"
        }
        "evidence.read" => {
            "Usage:\n  podway evidence read --source <graph-node-id> --item <item-id> [--page-token <token>] [--if-workspace-uuid <uuid>] [--if-session-id <uuid>]\n\nReturns one bounded page of an item selected by a currently resolved, declared evidence reference of the current attempt. It changes no session revision. Text offsets count Unicode scalars, list offsets count entries, and other item types return one terminal page. The page token is opaque and is not an authorization credential.\n\nExample:\n  podway --json evidence read --source implement --item notes"
        }
        "delegation.package" => {
            "Usage:\n  podway delegation package [--json] [--if-workspace-uuid <uuid>] [--if-session-id <uuid>]\n\nReads the immutable assignment for the current delegated action. Read complete upstream evidence through evidence read before assigning external work. The package digest is stable across action item edits and reads."
        }
        "delegation.accept" => {
            "Usage:\n  podway delegation accept --stdin [--json]\n\nThe closed JSON input supplies workspace UUID, session ID and revision, attempt ID, idempotency key, package digest, execution identity, result digest, rounds, summary, and worker disposition. Record final action evidence and settle the worker before acceptance. Acceptance does not complete the action; run complete while the receipt is current. After an accepted item edit, retry or rework to a fresh attempt. Recover a lost response through outcome lookup with the same key before any replay."
        }
        "delegation.receipt" => {
            "Usage:\n  podway delegation receipt --session <uuid> --attempt <uuid> [--json] [--if-workspace-uuid <uuid>]\n\nReads one exact immutable receipt from the current or retained session. Historical replay preserves the original result; reread current status before completion. Editing an accepted item makes acceptance stale and requires a deliberate fresh attempt."
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
        "session.block" => {
            "Usage:\n  podway block --reason <text> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--if-attempt <uuid>]\n\nExample:\n  podway block --reason 'waiting for API owner'"
        }
        "session.unblock" => {
            "Usage:\n  podway unblock (<blocker-id> | --all) [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--if-attempt <uuid>]\n\nExample:\n  podway unblock --all"
        }
        "session.cancel" => {
            "Usage:\n  podway cancel --reason <text> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--if-attempt <uuid>]\n\nExample:\n  podway cancel --reason 'task no longer needed'"
        }
        "session.terminal_disposition" => {
            "Usage:\n  podway disposition handed-off --summary <text> --reference <text> [--actor <text>] [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>]\n  podway disposition not-required --reason <text> [--actor <text>] [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>]\n\nA disposition is valid only for the exact current completed or cancelled revision. It makes that terminal session eligible for default reset or replacement. Omitted fences are filled from a fresh status preflight."
        }
        "session.archive" => {
            "Usage:\n  podway archive [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>]\n\nArchives the current completed or cancelled session after its terminal disposition is current. The session remains immutable and readable with podway archive show."
        }
        "session.archive_list" => {
            "Usage:\n  podway archive list [--if-workspace-uuid <uuid>]\n\nLists inactive sessions retained for this worktree."
        }
        "session.archive_show" => {
            "Usage:\n  podway archive show --session-id <uuid> [--verbose [--history-before <n>]] [--if-workspace-uuid <uuid>]\n\nShows the preserved read-only state of one inactive session."
        }
        "session.archive_purge" => {
            "Usage:\n  podway archive purge --session-id <uuid> --if-session-revision <n> --yes [--if-workspace-uuid <uuid>]\n\nPermanently deletes exactly one inactive session."
        }
        "session.reset" => {
            "Usage:\n  podway reset [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--dry-run]\n  podway reset --progress-summary <text> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--yes]\n\nExamples:\n  podway reset --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 0\n  podway reset --progress-summary 'Preserved the current diff and verification notes.' --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 7 --yes\n\nDefault reset deletes only a prepared session or a terminal session with a current disposition. A dry run reports eligibility without mutation. Force reset requires a bounded progress summary and explicit confirmation; JSON or non-terminal callers must pass --yes, while an interactive caller may confirm the prompt."
        }
        "workspace.reset_all" => {
            "Usage:\n  podway reset --all --yes --idempotency-key <key> [--if-workspace-uuid <uuid>]\n\nExample:\n  podway reset --all --yes --idempotency-key reset-worktree"
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
        "item.record_many" => {
            "Usage:\n  podway record --stdin [--worktree <path>] [--json]\n\nThe closed JSON stdin document supplies workspace_uuid, session_id, session_revision, attempt_id, idempotency_key, and 1..128 unique typed operations. A check_result is caller supplied and structurally bound; Podway does not execute, verify, or attest the named external operation.\n\nExample:\n  podway record --stdin < record.json"
        }
        "outcome.lookup" => {
            "Usage:\n  podway outcome lookup --idempotency-key <key> [--if-workspace-uuid <uuid>] [--json]\n\nRead the original committed success response without replaying or admitting a mutation."
        }
        _ => return Err(LocalFailure::request_invalid("unknown help topic")),
    };
    Ok(text.to_owned())
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
        local_generated_at, local_result, parse_failure_command_context, parse_timeout_millis,
        procedure_validate_v2_diagnostics, render_authoring_diagnostics,
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
            super::help_text(Some("workspace.remove"))
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
    fn oversized_output_is_an_integrity_failure_without_partial_stdout() {
        let clock = FixedClock(UNIX_EPOCH + Duration::from_millis(12));
        for (length, exit) in [
            (65_537, 0),
            (podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1, 6),
        ] {
            let result = local_result(
                "procedure.show",
                json!({"canonical_json": "x".repeat(length)}),
                String::new(),
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
                exit,
            );
            assert!(stderr.is_empty());
            if exit == 0 {
                let output: OutputEnvelopeV4 = serde_json::from_slice(&stdout).unwrap();
                assert_eq!(
                    output.as_value()["result"]["canonical_json"]
                        .as_str()
                        .unwrap()
                        .len(),
                    length
                );
            } else {
                assert!(stdout.is_empty());
            }
        }
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
