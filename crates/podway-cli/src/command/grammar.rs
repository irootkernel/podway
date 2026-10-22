//! Command grammar, classification and parse diagnostics.

use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
};

use clap::{ArgAction, ArgMatches, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use podway_config::ScaffoldTemplate;
use podway_protocol::ItemRecordManyInputV1;
use serde_json::Value;

use super::completion::Shell;
use super::{LocalFailure, MAX_DIRECT_COMMAND_TIMEOUT_MILLIS_V1, retirement};

#[derive(Debug, Parser)]
#[command(
    name = "podway",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    subcommand_required = true,
    arg_required_else_help = false
)]
pub(super) struct Cli {
    /// Emit exactly one versioned JSON object to stdout.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub(super) json: bool,

    /// Target an explicit Git worktree.
    #[arg(long, global = true, value_name = "PATH")]
    pub(super) worktree: Option<PathBuf>,

    /// Bound lock-acquisition waits; admitted work continues past the deadline.
    #[arg(long, global = true, value_name = "DURATION", value_parser = parse_timeout_millis)]
    pub(super) timeout: Option<u64>,

    /// Disable text color. Podway v1 currently emits uncolored text.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub(super) no_color: bool,

    /// Suppress nonessential text output.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub(super) quiet: bool,

    /// Reuse this idempotency key instead of generating a UUID-v4 key.
    #[arg(long, global = true, value_name = "KEY", allow_hyphen_values = true)]
    pub(super) idempotency_key: Option<String>,

    /// Require an exact workspace identifier.
    #[arg(long, global = true, value_name = "UUID")]
    pub(super) if_workspace_uuid: Option<String>,

    /// Require an exact session identifier.
    #[arg(long, global = true, value_name = "UUID")]
    pub(super) if_session_id: Option<String>,

    /// Require an exact session revision.
    #[arg(long, global = true, value_name = "N")]
    pub(super) if_session_revision: Option<u64>,

    /// Require an exact active attempt identifier.
    #[arg(long, global = true, value_name = "UUID")]
    pub(super) if_attempt: Option<String>,

    /// Require an exact active item revision.
    #[arg(long, global = true, value_name = "N")]
    pub(super) if_item_revision: Option<u64>,

    /// Require an exact current Procedure v2 goal revision.
    #[arg(long, global = true, value_name = "N", value_parser = parse_positive_u64)]
    pub(super) if_goal_revision: Option<u64>,

    /// Approve a destructive operation without prompting.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub(super) yes: bool,

    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
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
    /// Validate or display a local procedure.
    Procedure {
        #[command(subcommand)]
        command: ProcedureCommand,
    },
    /// Inspect built-in presets locally.
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
pub(super) enum OutcomeCommand {
    Lookup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub(super) enum ExistingSessionPolicy {
    Preserve,
    Delete,
}

#[derive(Clone, Debug)]
pub(super) enum StartTerminalDisposition {
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
pub(super) struct StartArgs {
    #[arg(
        long,
        value_name = "PRESET",
        conflicts_with = "procedure",
        required_unless_present = "procedure"
    )]
    pub(super) preset: Option<String>,
    #[arg(
        long,
        value_name = "FILE",
        conflicts_with = "preset",
        required_unless_present = "preset"
    )]
    pub(super) procedure: Option<String>,
    #[arg(
        long,
        value_name = "SHA256",
        requires = "procedure",
        conflicts_with = "preset"
    )]
    pub(super) expect_procedure_digest: Option<String>,
    #[arg(long, value_name = "TITLE")]
    pub(super) task: String,
    #[arg(long, value_enum, value_name = "POLICY")]
    pub(super) on_existing: Option<ExistingSessionPolicy>,
    #[arg(long, value_name = "TEXT")]
    pub(super) supersede_reason: Option<String>,
    #[arg(long, value_name = "TEXT")]
    pub(super) actor: Option<String>,
    #[arg(long, value_name = "TEXT")]
    pub(super) progress_summary: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub(super) dry_run: bool,
    #[arg(skip)]
    pub(super) auto_archive_terminal: bool,
    #[arg(skip)]
    pub(super) terminal_disposition: Option<StartTerminalDisposition>,
    #[arg(skip)]
    pub(super) delete_confirmed: bool,
}

#[derive(Debug, Args)]
pub(super) struct BeginArgs {
    #[arg(long, value_name = "TEXT", requires = "criterion")]
    pub(super) goal: Option<String>,
    #[arg(long, value_name = "ID=STATEMENT", action = ArgAction::Append, requires = "goal")]
    pub(super) criterion: Vec<String>,
    #[arg(long, value_name = "TEXT", requires = "goal")]
    pub(super) actor: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(super) enum DispositionCommand {
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
pub(super) enum EvidenceCommand {
    /// Read one bounded page of an item selected by a declared evidence reference.
    Read(EvidenceReadArgs),
}

#[derive(Debug, Args)]
pub(super) struct EvidenceReadArgs {
    #[arg(long, value_name = "GRAPH_NODE_ID")]
    pub(super) source: String,
    #[arg(long, value_name = "ITEM_ID")]
    pub(super) item: String,
    #[arg(long, value_name = "TOKEN")]
    pub(super) page_token: Option<String>,
}

#[derive(Debug, Args, Default)]
pub(super) struct ReadArgs {
    #[arg(long, action = ArgAction::SetTrue)]
    pub(super) verbose: bool,
}

#[derive(Debug, Args, Default)]
pub(super) struct StatusArgs {
    #[command(flatten)]
    pub(super) read: ReadArgs,
    #[arg(
        long,
        action = ArgAction::SetTrue,
        conflicts_with = "verbose"
    )]
    pub(super) compact: bool,
    #[arg(
        long,
        value_name = "TRACE_SEQUENCE",
        value_parser = clap::value_parser!(u64).range(1..),
        requires = "verbose",
        conflicts_with = "compact"
    )]
    pub(super) history_before: Option<u64>,
}

#[derive(Debug, Args)]
pub(super) struct DecideArgs {
    #[arg(long, value_name = "ID")]
    pub(super) option: String,
    #[arg(long, value_name = "TEXT")]
    pub(super) reason: String,
    #[arg(long, value_name = "TEXT")]
    pub(super) actor: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct ReworkArgs {
    #[arg(long, value_name = "GRAPH_NODE_ID")]
    pub(super) to: String,
    #[arg(long, value_name = "TEXT")]
    pub(super) reason: String,
    #[arg(long, value_name = "TEXT")]
    pub(super) actor: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(super) enum GoalCommand {
    Define(GoalDefineArgs),
    Revise(GoalReviseArgs),
    AssessCriterion(GoalAssessCriterionArgs),
}

#[derive(Debug, Args)]
pub(super) struct GoalDefineArgs {
    #[arg(long, value_name = "TEXT")]
    pub(super) goal: String,
    #[arg(long, value_name = "ID=STATEMENT", action = ArgAction::Append, required = true)]
    pub(super) criterion: Vec<String>,
    #[arg(long, value_name = "TEXT")]
    pub(super) actor: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct GoalReviseArgs {
    #[arg(long, value_name = "TEXT")]
    pub(super) goal: String,
    #[arg(long, value_name = "ID=STATEMENT", action = ArgAction::Append, required = true)]
    pub(super) criterion: Vec<String>,
    #[arg(long, value_name = "GRAPH_NODE_ID")]
    pub(super) rework_to: String,
    #[arg(long, value_name = "TEXT")]
    pub(super) reason: String,
    #[arg(long, value_name = "TEXT")]
    pub(super) actor: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub(super) reactivate: bool,
}

#[derive(Debug, Args)]
pub(super) struct GoalAssessCriterionArgs {
    #[arg(value_name = "CRITERION_ID")]
    pub(super) criterion_id: String,
    #[arg(long, value_name = "STATUS", value_parser = ["satisfied", "unsatisfied", "not_applicable"])]
    pub(super) status: String,
    #[arg(long, value_name = "TEXT")]
    pub(super) reason: String,
    #[arg(long, value_name = "GRAPH_NODE_ID", action = ArgAction::Append)]
    pub(super) evidence: Vec<String>,
    #[arg(long, value_name = "ITEM_ID", action = ArgAction::Append)]
    pub(super) item: Vec<String>,
    #[arg(long, value_name = "TEXT")]
    pub(super) actor: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct ResetArgs {
    #[arg(long, action = ArgAction::SetTrue)]
    pub(super) all: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    pub(super) force: bool,
    #[arg(long, value_name = "TEXT")]
    pub(super) progress_summary: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub(super) dry_run: bool,
}

#[derive(Debug, Args)]
pub(super) struct SetArgs {
    #[arg(value_name = "ITEM_ID")]
    pub(super) item_id: String,
    #[arg(
        value_name = "VALUE",
        required_unless_present = "stdin",
        conflicts_with = "stdin"
    )]
    pub(super) value: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub(super) stdin: bool,
}

#[derive(Debug, Args)]
pub(super) struct AttachArgs {
    #[arg(value_name = "ITEM_ID")]
    pub(super) item_id: String,
    #[arg(
        value_name = "PATH",
        required_unless_present = "reference",
        conflicts_with = "reference"
    )]
    pub(super) path: Option<String>,
    #[arg(long, value_name = "REFERENCE", conflicts_with = "path", requires_all = ["digest", "size", "media_type"])]
    pub(super) reference: Option<String>,
    #[arg(long, value_name = "SHA256", requires = "reference")]
    pub(super) digest: Option<String>,
    #[arg(long, value_name = "BYTES", requires = "reference")]
    pub(super) size: Option<u64>,
    #[arg(long, value_name = "TYPE")]
    pub(super) media_type: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct RecordArgs {
    #[arg(long, required = true, action = ArgAction::SetTrue)]
    pub(super) stdin: bool,
    #[arg(skip)]
    pub(super) input: Option<ItemRecordManyInputV1>,
}

#[derive(Debug, Subcommand)]
pub(super) enum ProcedureCommand {
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
pub(super) enum PresetCommand {
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
pub(super) enum WorkspaceCommand {
    Show,
    Repair,
    Remove {
        #[arg(long, action = ArgAction::SetTrue)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum ArchiveCommand {
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
pub(super) enum DelegationCommand {
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
pub(super) struct DelegationAcceptArgs {
    #[arg(long, required = true, action = ArgAction::SetTrue)]
    pub(super) stdin: bool,
    #[arg(skip)]
    pub(super) input: Option<Value>,
}

impl Command {
    pub(super) const fn direct_route(&self) -> Option<&'static str> {
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

    pub(super) fn canonical_route(&self) -> &'static str {
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

    pub(super) const fn is_mutation(&self) -> bool {
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

    pub(super) const fn needs_preflight(&self) -> bool {
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

    pub(super) const fn is_item_mutation(&self) -> bool {
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

    pub(super) const fn accepts_workspace_identity(&self) -> bool {
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

    pub(super) const fn accepts_session_identity(&self) -> bool {
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

    pub(super) const fn is_destructive(&self) -> bool {
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

    pub(super) const fn is_dry_run(&self) -> bool {
        matches!(
            self,
            Self::Start(StartArgs { dry_run: true, .. })
                | Self::Reset(ResetArgs { dry_run: true, .. })
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ParseFailureCommandContext {
    pub(super) route: &'static str,
    pub(super) mutation: bool,
}

impl ParseFailureCommandContext {
    pub(super) const fn new(route: &'static str, mutation: bool) -> Self {
        Self { route, mutation }
    }
}

pub(super) fn parse_failure_command_context(
    arguments: &[OsString],
) -> Option<ParseFailureCommandContext> {
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

pub(super) fn normalize_top_level_help_alias(arguments: &mut [OsString]) {
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

pub(super) fn parse_timeout_millis(value: &str) -> Result<u64, String> {
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

pub(super) fn help_text(topic: Option<&str>) -> Result<String, LocalFailure> {
    let topic = topic.unwrap_or("overview");
    if topic == "overview" {
        let preset_ids = podway_presets::list()
            .iter()
            .map(|preset| preset.metadata.id)
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(format!(
            "Podway coordinates durable worktree-local procedures.\n\nTrust boundary:\n  Podway trusts same-user processes reading and writing local workspace state.\n  It provides no authentication or workspace access key.\n  It does not protect against malicious same-user processes.\n  It records caller assertions and does not judge their semantic truth.\n\nDirect execution:\n  Worktree commands execute in this process.\n  Use --worktree <path> to select a Git worktree and --timeout <duration> to bound lock waiting.\n  Worktree request options apply to workspace routes only; help, version, completions, preset and procedure run locally, open no workspace and accept none of them.\n  Automation mutations require an explicit --idempotency-key; recover lost output with outcome lookup.\n\nUsage:\n  podway help [<route>]\n  podway -h\n  podway --help\n\nExamples:\n  podway start --preset sw-dev-v2 --task 'add retry backoff' --idempotency-key start-example\n  podway begin --idempotency-key begin-example\n  podway status --json\n  podway observe --json\n\nProcedure v2 routes:\n  procedure format|vet|lint|check|graph|preview|scaffold\n  session.begin, session.terminal_disposition, session.decide, session.rework\n  goal.define, goal.revise, goal.assess_criterion\n  evidence read\n  delegation package|accept|receipt\n\nShipped presets: {preset_ids}. Start prepares a session; begin creates its first active attempt. Contributors use disposable Git worktrees for isolated development; see help workflow."
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
            "Usage:\n  podway procedure preview <worktree-relative-file>\n\nRuns validation, graph vetting, and advisory lint in memory, then prints the\nnormalized graph, Mermaid review projection, and an exact session start suggestion\nwhen the Procedure v2 document is admissible. The file must use the same UTF-8,\nworktree-relative, no-parent spelling accepted by start. Reads only and never\nopens workspace runtime state.\n\nExample:\n  podway procedure preview .podway/procedures/custom.yaml"
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
            "Usage:\n  podway [--worktree <path>] workspace remove [--force] [--if-workspace-uuid <uuid>] [--yes]\n\nUnfenced interactive use requires the exact resolved absolute worktree root as confirmation, including with --yes. JSON use also requires --force. JSON and non-interactive use require --yes and --if-workspace-uuid. Success removes all .podway content, including tracked files, while preserving the Git worktree."
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
            "Usage:\n  podway add <item-id> <value> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-attempt <uuid>] [--if-item-revision <n>]\n\nExample:\n  podway add affected-components engine"
        }
        "item.remove" => {
            "Usage:\n  podway remove <item-id> <value> [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-attempt <uuid>] [--if-item-revision <n>] [--ignore-missing]\n\nExample:\n  podway remove affected-components engine"
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
