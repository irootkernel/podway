//! Command grammar, classification and parse diagnostics.

use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
};

use clap::{ArgAction, ArgMatches, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use podway_config::ScaffoldTemplate;
use podway_protocol::{CriterionAssessmentManyInputV1, ItemRecordManyInputV1};

use super::completion::Shell;
use super::{LocalFailure, MAX_DIRECT_COMMAND_TIMEOUT_MILLIS_V1, retirement};

pub(super) const OBSERVATION_READ_GUIDANCE: &str = "observe returns the current scoped cursor, active item revisions, guidance and available history summaries. Generation 2 adds executable templates. Use group and history queries for retained work and evidence read for current evidence values.";
pub(super) const RECORD_CHECK_GUIDANCE: &str = "Record typed items through record --stdin using the current scoped cursor and each expected item revision. A check_result records the caller-supplied operation, input basis, executor, outcome and output digest. Podway does not execute or attest that operation or store its output bytes.";
pub(super) const REVISION_GUIDANCE: &str = "Supply --if-workspace-uuid and the complete result.cursor JSON as --if-cursor when preserving an exact scoped mutation request. Use result.items[].revision as the separate --if-item-revision for a single-item edit. workspace.latest_workspace_sequence orders newly retained successful mutations. Reads and exact replay do not advance it.";

const LIFECYCLE_FENCE_GUIDANCE: &str = "The CLI can fill omitted fences from a fresh status preflight. Automation should retain the observed template fences to preserve its intended workspace, session and revision; do not silently substitute a newer target. Begin and terminal disposition accept no attempt fence. Fence flags remain optional CLI syntax; a caller-owned automation key is still required.";
const DISPOSITION_GUIDANCE: &str = "handed-off records an actual external result with its summary and reference. The reference identifies that result; it is not an instruction for Podway to run. not-required records why no external handoff is owed. Podway records these caller assertions and does not execute or attest the external result.";
const REPLACEMENT_GUIDANCE: &str = "Replacement templates named session.start_replace have two variants: --preset selects an embedded preset, while --procedure with --expect-procedure-digest selects a pinned file. Both are variants of the same session.start_replace action, not two different actions. The digest flag remains optional in CLI grammar; the file template supplies it. Keep the observed command identity, policy, authorization requirements and fences; disposed-terminal auto-archive templates named session.start remain that distinct action.";
const CITATION_GUIDANCE: &str = "--evidence and --item are optional and repeatable, with at most four citations in total across both flags and no duplicate within either kind. Cite only resolved, valid sources declared in the current decision attempt's guidance.evidence with --evidence, or a recorded decision-local item with --item; unrecorded items and undeclared, skipped or stale sources are ineligible. not_applicable carries no citations. The current session, attempt and goal fences still apply; citations are caller assertions, not attestation.";
const OUTCOME_RECOVERY_GUIDANCE: &str = "After lost output, retain the original key and complete canonical request, including original resolved identities, revision/attempt/item/goal fences, values and options. Use outcome lookup with that same --idempotency-key in the owning worktree before exact replay where supported, after classifying the current workspace and writer. Lookup is read-only and never submits a mutation. A retained success contains the original response; exact replay preserves its request ID, timestamp and fences without a second effect. Changed input under that key fails IDEMPOTENCY_KEY_REUSED; a fresh key or refreshed fence is a new mutation. A found=false miss is retention-bounded and does not prove that an external effect never happened or authorize repeating external work. Session outcomes survive archival and archive purge until session/runtime reset or workspace removal; workspace outcomes retain at most 100 entries or 30 days, pruned on successful writes. Completed workspace removal destroys its Store and converges on absence rather than replaying the original response.";

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

    /// Require an exact current goal revision.
    #[arg(long, global = true, value_name = "N", value_parser = parse_positive_u64)]
    pub(super) if_goal_revision: Option<u64>,

    /// Require the exact scoped cursor JSON obtained from observe.
    #[arg(long, global = true, value_name = "JSON")]
    pub(super) if_cursor: Option<String>,

    /// Approve a destructive operation without prompting.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub(super) yes: bool,

    /// Select the closed contract generation for this invocation.
    #[arg(long, global = true, value_name = "1|2")]
    pub(super) contract_generation: Option<String>,

    /// Return the committed mutation result and successor v8 observation in one atomic envelope.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub(super) with_observation: bool,

    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StdinFencedRoute {
    RecordMany,
    DelegationAccept,
    GoalAssessCriteria,
    GroupActivate,
    GroupReplan,
}

impl StdinFencedRoute {
    pub(super) fn from_route(route: &str) -> Option<Self> {
        match route {
            "item.record_many" => Some(Self::RecordMany),
            "delegation.accept" => Some(Self::DelegationAccept),
            "goal.assess_criteria" => Some(Self::GoalAssessCriteria),
            "group.activate" => Some(Self::GroupActivate),
            "group.replan" => Some(Self::GroupReplan),
            _ => None,
        }
    }

    pub(super) const fn conflict_message(self) -> &'static str {
        match self {
            Self::RecordMany => {
                "record identity, revision, and idempotency fences must come only from stdin"
            }
            Self::DelegationAccept => {
                "delegation acceptance identity and idempotency must come only from stdin"
            }
            Self::GoalAssessCriteria => {
                "goal assessment batch identity, revision, and idempotency fences must come only from stdin"
            }
            Self::GroupActivate | Self::GroupReplan => {
                "group identity, revision, and idempotency fences must come only from stdin"
            }
        }
    }
}

pub(super) fn is_stdin_fenced_mutation_route(route: &str) -> bool {
    StdinFencedRoute::from_route(route).is_some()
}

impl Cli {
    pub(super) fn reject_stdin_conflicting_fences(&self) -> Result<(), LocalFailure> {
        let Some(route) = self.command.stdin_fenced_route() else {
            return Ok(());
        };
        if self.idempotency_key.is_some()
            || self.if_workspace_uuid.is_some()
            || self.if_session_id.is_some()
            || self.if_session_revision.is_some()
            || self.if_attempt.is_some()
            || self.if_item_revision.is_some()
            || self.if_goal_revision.is_some()
            || self.if_cursor.is_some()
        {
            return Err(LocalFailure::request_invalid(route.conflict_message()));
        }
        Ok(())
    }
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Show offline help and examples.
    Help {
        #[arg(value_name = "TOPIC")]
        topic: Vec<String>,
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
    /// Read retained execution or decision records.
    History {
        #[command(subcommand)]
        command: HistoryCommand,
    },
    /// Activate, return from, or replan an execution group.
    Group {
        #[command(subcommand)]
        command: GroupCommand,
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
    /// Show effective integration permission without opening runtime state.
    Integration {
        #[command(subcommand)]
        command: IntegrationCommand,
    },
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
pub(super) enum IntegrationCommand {
    Show {
        #[arg(value_name = "INTEGRATION")]
        integration: String,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum OutcomeCommand {
    Lookup,
}

#[derive(Debug, Subcommand)]
pub(super) enum GroupCommand {
    /// Activate one group from the current approved design.
    Activate {
        #[arg(long, required = true, action = ArgAction::SetTrue)]
        stdin: bool,
    },
    /// Return from the current group's terminal placement to its declared base successor.
    Return,
    /// List retained activations.
    List(HistoryPageArgs),
    /// Read one retained activation and its decisions.
    Show {
        #[command(flatten)]
        page: HistoryPageArgs,
        #[arg(long, value_name = "UUID")]
        execution: String,
    },
    /// List the prior work bound to a retained activation.
    PriorWork {
        #[command(flatten)]
        page: HistoryPageArgs,
        #[arg(long, value_name = "UUID")]
        execution: String,
        #[arg(long, value_name = "SHA256")]
        definition_digest: String,
    },
    /// Record a guarded replan and return to the base design anchor.
    Replan {
        #[arg(long, required = true, action = ArgAction::SetTrue)]
        stdin: bool,
    },
}

#[derive(Debug, Args)]
pub(super) struct HistoryPageArgs {
    /// Exact current or archived session to read.
    #[arg(long, value_name = "UUID")]
    pub(super) session: String,
    /// Select the archived session namespace.
    #[arg(long, action = ArgAction::SetTrue)]
    pub(super) archived: bool,
    /// Maximum number of rows to return.
    #[arg(long, default_value_t = 32, value_parser = clap::value_parser!(u8).range(1..=64))]
    pub(super) limit: u8,
    /// Continuation returned by the same history query.
    #[arg(long, value_name = "TOKEN")]
    pub(super) page_token: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct HistoryDefinitionArgs {
    /// Exact definition digest. With no group execution, this selects the base.
    #[arg(long, value_name = "SHA256")]
    pub(super) definition_digest: String,
    /// Exact retained execution UUID when selecting a group definition.
    #[arg(long, value_name = "UUID")]
    pub(super) group_execution: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(super) enum HistoryCommand {
    /// Read a retained graph definition.
    Dag {
        #[command(flatten)]
        page: HistoryPageArgs,
        #[command(flatten)]
        scope: HistoryDefinitionArgs,
    },
    /// List retained attempts in a particular definition.
    Attempts {
        #[command(flatten)]
        page: HistoryPageArgs,
        #[command(flatten)]
        scope: HistoryDefinitionArgs,
    },
    /// List blockers for an exact retained attempt.
    Blockers {
        #[command(flatten)]
        page: HistoryPageArgs,
        #[arg(long, value_name = "JSON")]
        attempt: String,
    },
    /// Read an exact retained item and its bounded value pages.
    Item {
        #[command(flatten)]
        page: HistoryPageArgs,
        #[arg(long, value_name = "JSON")]
        attempt: String,
        #[arg(long, value_name = "ITEM_ID")]
        item: String,
    },
    /// List goal revisions and their selection lineage.
    GoalRevisions(HistoryPageArgs),
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
    AssessCriteria(GoalAssessCriteriaArgs),
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
    pub(super) basis_description: String,
    #[arg(long, value_name = "SHA256")]
    pub(super) basis_digest: String,
    #[arg(long, value_name = "TEXT")]
    pub(super) feedback: String,
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
pub(super) struct GoalAssessCriteriaArgs {
    #[arg(long, required = true, action = ArgAction::SetTrue)]
    pub(super) stdin: bool,
    #[arg(skip)]
    pub(super) input: Option<CriterionAssessmentManyInputV1>,
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
    pub(super) input: Option<podway_protocol::DelegationAcceptanceInputV1>,
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
            Self::Goal {
                command: GoalCommand::AssessCriteria(_),
            } => Some("goal.assess_criteria"),
            Self::Group { command } => Some(match command {
                GroupCommand::Activate { .. } => "group.activate",
                GroupCommand::Return => "group.return",
                GroupCommand::List(_) => "group.list",
                GroupCommand::Show { .. } => "group.show",
                GroupCommand::PriorWork { .. } => "group.prior_work",
                GroupCommand::Replan { .. } => "group.replan",
            }),
            Self::History { command } => Some(match command {
                HistoryCommand::Dag { .. } => "history.dag",
                HistoryCommand::Attempts { .. } => "history.attempts",
                HistoryCommand::Blockers { .. } => "history.blockers",
                HistoryCommand::Item { .. } => "history.item",
                HistoryCommand::GoalRevisions(_) => "history.goal_revisions",
            }),
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
            Self::Integration { .. } => Some("integration.show"),
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

    pub(super) fn stdin_fenced_route(&self) -> Option<StdinFencedRoute> {
        StdinFencedRoute::from_route(self.canonical_route())
    }

    pub(super) fn uses_stdin_fences(&self) -> bool {
        self.stdin_fenced_route().is_some()
    }

    pub(super) const fn is_mutation(&self) -> bool {
        match self {
            Self::Init { .. }
            | Self::Complete
            | Self::Begin(_)
            | Self::Decide(_)
            | Self::Rework(_)
            | Self::Goal { .. }
            | Self::Group {
                command:
                    GroupCommand::Activate { .. } | GroupCommand::Return | GroupCommand::Replan { .. },
            }
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
            | Self::Group {
                command: GroupCommand::Return,
            }
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
            | Self::Clear { .. } => true,
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
                | Self::Group { .. }
                | Self::History { .. }
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
            // Plain start accepts the paired optional session fences; the
            // pairing itself is enforced by `validate_direct_flags`.
            Self::Start(_)
            | Self::Begin(_)
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
            | Self::Group {
                command: GroupCommand::Return,
            }
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

/// Parser context selects candidates; only labels regenerated from our grammar may escape.
pub(super) fn syntax_failure_message(error: &clap::Error, arguments: &[OsString]) -> String {
    use clap::error::{ContextKind, ContextValue, ErrorKind};

    let generic = || "invalid command syntax".to_owned();
    if error.kind() != ErrorKind::MissingRequiredArgument {
        return generic();
    }
    let Some(ContextValue::Strings(missing)) = error.get(ContextKind::InvalidArg) else {
        return generic();
    };
    let mut grammar = Cli::command().ignore_errors(true);
    let Ok(matches) = grammar.try_get_matches_from_mut(arguments) else {
        return generic();
    };
    let mut scope = &grammar;
    let mut selected = &matches;
    while let Some((name, nested)) = selected.subcommand() {
        let Some(child) = scope.find_subcommand(name) else {
            return generic();
        };
        scope = child;
        selected = nested;
    }
    // get_arguments retains declaration order, independently of Clap's diagnostic ordering.
    for argument in scope.get_arguments() {
        let label = argument.clone().required(true).to_string();
        if missing.contains(&label) {
            return format!("missing required argument: {label}");
        }
    }
    generic()
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
                    ("assess-criteria", "goal.assess_criteria"),
                ],
            )?
            .route;
            ParseFailureCommandContext::new(route, true)
        }
        "group" => {
            let route = nested_parse_failure_context(
                matches,
                &[
                    ("activate", "group.activate"),
                    ("return", "group.return"),
                    ("replan", "group.replan"),
                    ("list", "group.list"),
                    ("show", "group.show"),
                    ("prior-work", "group.prior_work"),
                ],
            )?
            .route;
            ParseFailureCommandContext::new(
                route,
                matches!(route, "group.activate" | "group.return" | "group.replan"),
            )
        }
        "history" => nested_parse_failure_context(
            matches,
            &[
                ("dag", "history.dag"),
                ("attempts", "history.attempts"),
                ("blockers", "history.blockers"),
                ("item", "history.item"),
                ("goal-revisions", "history.goal_revisions"),
            ],
        )?,
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
        "integration" => nested_parse_failure_context(matches, &[("show", "integration.show")])?,
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
    // `--contract-generation` is the only valued global flag admitted before
    // the help alias, so its value token is skipped alongside it.
    let mut skip_generation_value = false;
    for (index, argument) in arguments.iter().enumerate().skip(1) {
        if skip_generation_value {
            skip_generation_value = false;
            continue;
        }
        match argument.as_os_str() {
            value if value == OsStr::new("-h") || value == OsStr::new("--help") => {
                if help_index.replace(index).is_some() {
                    return;
                }
            }
            value if value == OsStr::new("--contract-generation") => {
                skip_generation_value = true;
            }
            value
                if value
                    .to_str()
                    .is_some_and(|text| text.starts_with("--contract-generation=")) => {}
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

/// Guide topics shared by topic resolution and the overview index.
const GUIDE_TOPICS: &[&str] = &[
    "workflow",
    "automation",
    "procedures",
    "artifacts",
    "contract-generations",
];

/// Retired family topics the overview index lists; each resolves through the
/// retirement table.
const RETIRED_FAMILY_TOPICS: &[&str] = &["daemon", "runtime reset", "workspace mode", "job"];

/// Flag-selected routes the overview index lists after their base command,
/// resolved through the same parse-failure context as plain word paths. Each
/// entry names the base command word, the displayed spelling and the exact
/// flag words used for the route lookup.
const FLAG_SELECTED_TOPICS: &[(&str, &str, &[&str])] = &[
    (
        "start",
        "start --on-existing",
        &["--on-existing", "preserve"],
    ),
    ("reset", "reset --all", &["--all"]),
];

/// Offline completion uses the same visible command tree and help definitions as resolution.
pub(super) fn help_completion_topics() -> Vec<String> {
    fn collect(scope: &clap::Command, path: &mut Vec<String>, topics: &mut Vec<String>) {
        for child in scope.get_subcommands().filter(|child| !child.is_hide_set()) {
            path.push(child.get_name().to_owned());
            topics.push(path.join(" "));
            if let Some(route) = word_route(path) {
                topics.push(route.to_owned());
            }
            collect(child, path, topics);
            path.pop();
        }
    }
    let mut grammar = Cli::command();
    grammar.build();
    let mut topics = vec!["overview".to_owned()];
    collect(&grammar, &mut Vec::new(), &mut topics);
    topics.extend(GUIDE_TOPICS.iter().map(|topic| (*topic).to_owned()));
    topics.extend(
        RETIRED_FAMILY_TOPICS
            .iter()
            .flat_map(|topic| [(*topic).to_owned(), topic.replace(' ', ".")]),
    );
    topics.extend(FLAG_SELECTED_TOPICS.iter().flat_map(|(_, display, flags)| {
        let mut words = vec![display.split_whitespace().next().unwrap().to_owned()];
        words.extend(flags.iter().map(|word| (*word).to_owned()));
        [
            (*display).to_owned(),
            word_route(&words).unwrap().to_owned(),
        ]
    }));
    topics.sort();
    topics.dedup();
    topics
}

pub(super) fn help_completion_globals() -> Vec<(String, bool)> {
    let mut grammar = Cli::command();
    grammar.build();
    grammar
        .get_arguments()
        .filter_map(|arg| {
            arg.get_long().map(|long| {
                (
                    long.to_owned(),
                    arg.get_num_args().is_some_and(|range| range.takes_values()),
                )
            })
        })
        .collect()
}

/// Maps one word path, including flag-selected spellings, to its route through
/// the parse-failure command context.
fn word_context(words: &[String]) -> Option<ParseFailureCommandContext> {
    let mut argv: Vec<OsString> = Vec::with_capacity(words.len() + 1);
    argv.push(OsString::from("podway"));
    argv.extend(words.iter().cloned().map(OsString::from));
    parse_failure_command_context(&argv)
}

fn word_route(words: &[String]) -> Option<&'static str> {
    word_context(words).map(|context| context.route)
}

/// Resolves executable command words and group words against the clap tree,
/// walking only non-hidden subcommands.
fn command_word_topic(words: &[String]) -> Option<String> {
    for (name, display, flags) in FLAG_SELECTED_TOPICS {
        if words
            .iter()
            .map(String::as_str)
            .eq(display.split_whitespace())
        {
            let mut selected = vec![(*name).to_owned()];
            selected.extend(flags.iter().map(|flag| (*flag).to_owned()));
            return word_route(&selected).and_then(route_help);
        }
    }
    let mut grammar = Cli::command();
    grammar.build();
    let mut scope = &grammar;
    for word in words {
        scope = scope
            .find_subcommand(word.as_str())
            .filter(|child| !child.is_hide_set())?;
    }
    if scope.is_subcommand_required_set() {
        let mut lines = Vec::new();
        for child in scope.get_subcommands().filter(|child| !child.is_hide_set()) {
            let mut child_words = words.to_vec();
            child_words.push(child.get_name().to_owned());
            let route = word_route(&child_words).expect("every listed group child maps to a route");
            lines.push(format!("  {} -> {route}", child_words.join(" ")));
        }
        return Some(format!(
            "{} command group:\n{}",
            words.join(" "),
            lines.join("\n")
        ));
    }
    word_route(words).and_then(route_help)
}

fn collect_executable_lines(
    scope: &clap::Command,
    path: &mut Vec<String>,
    lines: &mut Vec<String>,
) {
    for child in scope.get_subcommands().filter(|child| !child.is_hide_set()) {
        path.push(child.get_name().to_owned());
        if !child.is_subcommand_required_set() {
            let words = path.join(" ");
            let route = word_route(path).expect("every executable word path maps to a route");
            lines.push(format!("  {words} -> {route}"));
            for (_, display, flags) in FLAG_SELECTED_TOPICS
                .iter()
                .filter(|(name, _, _)| path.len() == 1 && path[0] == *name)
            {
                let mut selected = path.clone();
                selected.extend(flags.iter().map(|flag| flag.to_string()));
                let route = word_route(&selected).expect("flag-selected words map to a route");
                lines.push(format!("  {display} -> {route}"));
            }
            collect_executable_lines(child, path, lines);
        } else {
            collect_executable_lines(child, path, lines);
        }
        path.pop();
    }
}

/// Generates the overview topic index: every executable word path in clap
/// declaration order with the flag-selected routes, then the guide, group and
/// retired-family lines.
fn overview_topic_lines(grammar: &clap::Command) -> Vec<String> {
    let mut lines = Vec::new();
    collect_executable_lines(grammar, &mut Vec::new(), &mut lines);
    lines.push(format!("  guides: {}", GUIDE_TOPICS.join(", ")));
    let groups: Vec<&str> = grammar
        .get_subcommands()
        .filter(|child| !child.is_hide_set() && child.is_subcommand_required_set())
        .map(|child| child.get_name())
        .collect();
    lines.push(format!("  command groups: {}", groups.join(", ")));
    lines.push(format!(
        "  retired families: {}",
        RETIRED_FAMILY_TOPICS.join(", ")
    ));
    lines
}

fn overview_text() -> String {
    let preset_ids = podway_presets::list_v3()
        .iter()
        .map(|preset| preset.metadata.id)
        .collect::<Vec<_>>()
        .join(", ");
    let mut grammar = Cli::command();
    grammar.build();
    let topics = overview_topic_lines(&grammar).join("\n");
    format!(
        "Podway coordinates durable worktree-local procedures.\n\nTrust boundary:\n  Podway trusts same-user processes reading and writing local workspace state.\n  It provides no authentication or workspace access key.\n  It does not protect against malicious same-user processes.\n  It records caller assertions and does not judge their semantic truth.\n\nDirect execution:\n  Worktree commands execute in this process.\n  Use --worktree <path> to select a Git worktree and --timeout <duration> to bound lock waiting.\n  Worktree request options apply to workspace routes only; help, version, completions, preset and procedure run locally, open no workspace and accept none of them.\n  Local commands also accept --contract-generation.\n  Automation mutations require an explicit --idempotency-key; recover lost output with outcome lookup.\n\nContract generations:\n  --contract-generation <1|2> selects the contract generation for one invocation.\n  Generation 1 is the default and is deprecated; no runtime warning is emitted.\n  Generation 2 serves observe as podway.observation-result/v8 and the\n  generation-2 error details of CRITERION_CITATION_INVALID and\n  WORKSPACE_CONTRACT_MISMATCH.\n\nUsage:\n  podway help [<topic>...]\n  podway -h\n  podway --help\n\nExamples:\n  podway start --preset sw-dev-v3 --task 'add retry backoff' --idempotency-key start-example\n  podway begin --idempotency-key begin-example\n  podway status --json\n  podway observe --json\n\nTopics:\n{topics}\n\nShipped presets: {preset_ids}. Start prepares a session; begin creates its first active attempt. Contributors use disposable Git worktrees for isolated development; see help workflow."
    )
}

pub(super) fn help_text(words: &[String]) -> Result<String, LocalFailure> {
    if words.is_empty() || (words.len() == 1 && words[0] == "overview") {
        return Ok(overview_text());
    }
    if let Some(guidance) = retirement::help_guidance(&words.join("."))
        .map_err(|_| LocalFailure::internal("the embedded retirement table is invalid"))?
    {
        return Ok(guidance);
    }
    if words.len() == 1
        && let Some(text) = route_help(&words[0])
    {
        return Ok(text);
    }
    if let Some(text) = command_word_topic(words) {
        return Ok(text);
    }
    Err(LocalFailure::request_invalid(
        "unknown help topic; run podway help to list topics",
    ))
}

/// Resolve help applicability through the existing grammar and route classifier.
fn topic_context(
    topic: &str,
    grammar: &clap::Command,
) -> Option<(ParseFailureCommandContext, bool)> {
    fn find(
        scope: &clap::Command,
        path: &mut Vec<String>,
        topic: &str,
    ) -> Option<(ParseFailureCommandContext, bool)> {
        for child in scope.get_subcommands().filter(|child| !child.is_hide_set()) {
            path.push(child.get_name().to_owned());
            if let Some(context) = word_context(path)
                && context.route == topic
            {
                let local = matches!(
                    path[0].as_str(),
                    "help" | "version" | "completions" | "procedure" | "preset"
                );
                return Some((context, local));
            }
            if let Some(found) = find(child, path, topic) {
                return Some(found);
            }
            path.pop();
        }
        None
    }
    for (name, _, flags) in FLAG_SELECTED_TOPICS {
        let mut words = vec![(*name).to_owned()];
        words.extend(flags.iter().map(|flag| (*flag).to_owned()));
        if let Some(context) = word_context(&words)
            && context.route == topic
        {
            return Some((context, false));
        }
    }
    find(grammar, &mut Vec::new(), topic)
}

fn route_help(topic: &str) -> Option<String> {
    let text = route_topic(topic)?;
    if topic == "automation" {
        return Some(format!("{text}\n\n{OUTCOME_RECOVERY_GUIDANCE}"));
    }
    let mut grammar = Cli::command();
    grammar.build();
    let Some((context, local)) = topic_context(topic, &grammar) else {
        return Some(text.to_owned());
    };
    let stdin_owned = is_stdin_fenced_mutation_route(topic);
    let mutation = context.mutation || matches!(topic, "workspace.repair" | "workspace.remove");
    let key = !stdin_owned && (mutation || topic == "outcome.lookup");
    let options: Vec<_> = grammar
        .get_arguments()
        .filter(|arg| {
            matches!(
                arg.get_id().as_str(),
                "json" | "no_color" | "quiet" | "contract_generation"
            ) || (!local && matches!(arg.get_id().as_str(), "worktree" | "timeout"))
                || (key && arg.get_id().as_str() == "idempotency_key")
        })
        .collect();
    let mut usage = true;
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            usage = false;
        }
        let mut line = line.to_owned();
        if usage && line.starts_with("  podway ") {
            for arg in &options {
                let flag = format!(
                    "--{}",
                    arg.get_long().expect("selected globals have long names")
                );
                if !line
                    .split_whitespace()
                    .any(|word| word.trim_matches(['[', ']']) == flag)
                {
                    line.push_str(&format!(" [{}]", (*arg).clone().required(true)));
                }
            }
        }
        lines.push(line);
    }
    let mut result = lines.join("\n");
    if stdin_owned {
        result.push_str("\n\nThe atomic stdin owns the key and fences; do not pass CLI identity, revision or idempotency options.");
    } else if topic == "outcome.lookup" {
        result.push_str("\n\nLookup requires its key even though it is read-only; use the original caller-owned --idempotency-key.");
    } else if mutation {
        result.push_str("\n\nAutomation mutations require a caller-owned --idempotency-key. Interactive mutations may omit it to generate a UUID-v4 key.");
        if matches!(
            topic,
            "session.start" | "session.start_replace" | "session.reset"
        ) {
            result.push_str(
                " With --dry-run the invocation is read-only and rejects --idempotency-key.",
            );
        }
    }
    let guidance: &[&str] = match topic {
        "session.observe" => &[
            OBSERVATION_READ_GUIDANCE,
            RECORD_CHECK_GUIDANCE,
            REVISION_GUIDANCE,
        ],
        "session.status" | "session.next" => &[OBSERVATION_READ_GUIDANCE],
        "session.begin" => &[LIFECYCLE_FENCE_GUIDANCE],
        "session.terminal_disposition" => &[LIFECYCLE_FENCE_GUIDANCE, DISPOSITION_GUIDANCE],
        "session.start" | "session.start_replace" => &[REPLACEMENT_GUIDANCE],
        "goal.assess_criterion" | "goal.assess_criteria" => &[CITATION_GUIDANCE],
        "outcome.lookup" => &[OUTCOME_RECOVERY_GUIDANCE],
        "item.record_many" => &[RECORD_CHECK_GUIDANCE, REVISION_GUIDANCE],
        "item.set"
        | "workspace.init"
        | "workspace.show"
        | "session.archive"
        | "session.archive_list"
        | "session.archive_show" => &[REVISION_GUIDANCE],
        _ => &[],
    };
    for guidance in guidance {
        result.push_str("\n\n");
        result.push_str(guidance);
    }
    if matches!(
        topic,
        "session.start"
            | "session.start_replace"
            | "session.begin"
            | "session.decide"
            | "session.rework"
            | "goal.define"
            | "goal.revise"
            | "goal.assess_criterion"
            | "session.terminal_disposition"
    ) {
        result.push_str("\n\n--actor is optional attribution of 1..256 Unicode scalars for correlation, not authentication or authorization.");
        if topic == "session.begin" {
            result.push_str(" The initial --goal and --criterion group remains optional; --actor requires --goal and --goal requires at least one --criterion.");
        } else if matches!(topic, "session.start" | "session.start_replace") {
            result.push_str(" On start, --actor requires --on-existing preserve; it does not apply to plain start or deletion.");
        }
    }
    if mutation {
        result.push_str("\n\nFor lost output, retain the original key and complete canonical request; consult podway help outcome.lookup before replay.");
    }
    Some(result)
}

/// The exact-topic table shared by single-word resolution and the command-word
/// walk: guides, local words and dotted route identifiers.
fn route_topic(topic: &str) -> Option<&'static str> {
    let text = match topic {
        "workflow" => {
            "Procedure v3 workflow:\n  podway init --idempotency-key init-example\n  podway --json preset show bug-fix-v3\n  podway --json start --preset bug-fix-v3 --task 'inspect the workflow' --idempotency-key start-example\n  podway --json begin --goal 'Verify the requested behavior.' --criterion verified='Relevant checks pass.' --actor developer --idempotency-key begin-example\n  podway --json observe\n\nAdd --dry-run to start for read-only Procedure inspection without creating a session. A successful start creates a prepared session; begin atomically creates the first attempt and optional goal. Follow the allowed item, option, evidence, and rework identifiers returned by observe. Contributors use a disposable Git worktree with its own .podway state. Use generation 2 observe to read the current plan binding and group activation template. Podway records caller assertions; it does not establish their semantic truth."
        }
        "automation" => {
            "Automation:\n  podway complete --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 12 --if-attempt <uuid> --idempotency-key task-42 --json"
        }
        "procedures" => {
            "Procedures:\n  podway procedure scaffold > .podway/procedures/custom.yaml\n  podway procedure format .podway/procedures/custom.yaml --write\n  podway procedure check .podway/procedures/custom.yaml --warnings-as-errors\n  podway procedure preview .podway/procedures/custom.yaml\n  podway start --procedure .podway/procedures/custom.yaml --expect-procedure-digest sha256:<hex> --task 'perform work'\n\nOther Procedure v3 authoring routes are procedure validate, vet, lint, and graph."
        }
        "artifacts" => {
            "Artifacts:\n  podway attach verification-reference report.md --media-type text/markdown\n  podway attach verification-reference --reference build:42 --digest sha256:<hex> --size 42 --media-type text/plain"
        }
        "contract-generations" => {
            "Contract generations:\n  podway --contract-generation <1|2> <command>\n  Generation 1 is the default and is deprecated; no runtime warning is emitted.\n  Generation 2 serves session.observe as podway.observation-result/v8 with\n  executable scoped mutation templates, and serves the\n  generation-2 error details of CRITERION_CITATION_INVALID and\n  WORKSPACE_CONTRACT_MISMATCH.\n  Selection applies to one invocation only and never changes request identity,\n  idempotency, stored outcomes, replay, outcome lookup, Store state or writer\n  identity. No environment or configuration channel selects a generation."
        }
        "help" => "Usage:\n  podway help [<topic>...]\n\nExample:\n  podway help session.start",
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
            "Usage:\n  podway procedure format <file> [--check] [--write]\n\nRenders a Procedure v3 document in canonical authoring form on stdout. --check\nreports drift and writes nothing; --write replaces the named file atomically.\n\nExample:\n  podway procedure format .podway/procedures/custom.yaml"
        }
        "procedure.vet" => {
            "Usage:\n  podway procedure vet <file>\n\nRuns mandatory graph-wide semantic and resource-budget checks on Procedure v3\nand Execution Group v1 documents without writing anything.\n\nExample:\n  podway procedure vet .podway/procedures/custom.yaml"
        }
        "procedure.graph" => {
            "Usage:\n  podway procedure graph <file> --format <json|mermaid|puml|dot>\n\nEmits a deterministic JSON, Mermaid, PlantUML, or DOT projection of a validated\nand vetted Procedure v3 graph without writing anything.\n\nExample:\n  podway procedure graph .podway/procedures/custom.yaml --format dot"
        }
        "procedure.preview" => {
            "Usage:\n  podway procedure preview <worktree-relative-file>\n\nRuns validation, graph vetting, and advisory lint in memory, then prints the\nnormalized graph, Mermaid review projection, and an exact session start suggestion\nwhen the Procedure v3 document is admissible. The file must use the same UTF-8,\nworktree-relative, no-parent spelling accepted by start. Reads only and never\nopens workspace runtime state.\n\nExample:\n  podway procedure preview .podway/procedures/custom.yaml"
        }
        "procedure.lint" => {
            "Usage:\n  podway procedure lint <file> [--warnings-as-errors]\n\nReports advisory authoring findings for a Procedure v3 document. Every finding is\na warning, so the file stays valid and the exit code stays 0 unless\n--warnings-as-errors makes any finding fatal.\n\nExample:\n  podway procedure lint .podway/procedures/custom.yaml --warnings-as-errors"
        }
        "procedure.check" => {
            "Usage:\n  podway procedure check <file> [--warnings-as-errors]\n\nRuns every authoring stage over a Procedure v3 document — canonical formatting,\nvalidation, graph vetting, and lint — and reports one merged result. Exits 1 on\nany error, including formatting drift; --warnings-as-errors also fails on\nadvisory findings.\n\nExample:\n  podway procedure check .podway/procedures/custom.yaml --warnings-as-errors"
        }
        "procedure.scaffold" => {
            "Usage:\n  podway procedure scaffold [--template minimal]\n\nWrites a minimal Procedure v3 authoring starting point to stdout and reads\nnothing. The emitted document is already in canonical authoring form and already\npasses every authoring stage, so redirecting it to a file and running\nprocedure check on that file reports nothing.\n\nExample:\n  podway procedure scaffold --template minimal > .podway/procedures/custom.yaml"
        }
        "preset.list" => "Usage:\n  podway preset list\n\nExample:\n  podway preset list",
        "preset.show" => {
            "Usage:\n  podway preset show <name>\n\nExample:\n  podway preset show sw-dev-v3"
        }
        "preset.explain" => {
            "Usage:\n  podway preset explain <name>\n\nExample:\n  podway preset explain sw-dev-v3"
        }
        "workspace.init" => {
            "Usage:\n  podway init [--repair]\n\nCreates the worktree-local Podway workspace. Tracked files:\n  .podway/config.yaml, .podway/procedures/ and .podway/.gitignore, which\n  ignores runtime/ so the worktree-local runtime stays untracked. Authoritative\n  task state and maintenance locks live under .podway/runtime/ and are never\n  tracked.\n\nWhen initialization is interrupted, the initialization marker retains the operation. Use the recovery recipe to resume the original workspace and idempotency key. A successful initialization reports latest_workspace_sequence.\n\nExamples:\n  podway init\n  podway init --repair"
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
        "group.activate" => {
            "Usage:\n  podway group activate --stdin\n\nSubmit a closed group activation input from the current anchor. Include the exact base/attempt/goal/selection fences, plan binding, group digest, working-copy basis and required prior-work dispositions. Read generation 2 observe for a template."
        }
        "group.replan" => {
            "Usage:\n  podway group replan --stdin\n\nSubmit the declared guarded replan option and closure feedback for the current scoped decision. Keep the original key and input after response loss."
        }
        "group.return" => {
            "Usage:\n  podway group return --if-workspace-uuid <uuid> --if-cursor <json> --idempotency-key <key>\n\nComplete the selected group's terminal action and open the declared base successor atomically."
        }
        "group.list" => {
            "Usage:\n  podway group list --session <uuid> [--archived] [--limit <rows>] [--page-token <token>]\n\nList retained group executions in activation order."
        }
        "group.show" => {
            "Usage:\n  podway group show --session <uuid> --execution <uuid> [--archived] [--limit <rows>] [--page-token <token>]\n\nRead a retained execution, its design binding, closure and decision records."
        }
        "group.prior_work" => {
            "Usage:\n  podway group prior-work --session <uuid> --execution <uuid> --definition-digest <sha256> [--archived] [--limit <rows>] [--page-token <token>]\n\nRead the prior-work dispositions bound to the selected execution."
        }
        "history.goal_revisions" => {
            "Usage:\n  podway history goal-revisions --session <uuid> [--archived] [--limit <rows>] [--page-token <token>]\n\nRead retained goal revisions and their selection closures."
        }
        "history.dag" => {
            "Usage:\n  podway history dag --session <uuid> --definition-digest <sha256> [--group-execution <uuid>] [--archived] [--page-token <token>]\n\nRead a retained base or group definition page."
        }
        "history.attempts" => {
            "Usage:\n  podway history attempts --session <uuid> --definition-digest <sha256> [--group-execution <uuid>] [--archived] [--limit <rows>] [--page-token <token>]\n\nList attempts in the selected retained definition."
        }
        "history.blockers" => {
            "Usage:\n  podway history blockers --session <uuid> --attempt <scoped-attempt-json> [--archived] [--limit <rows>] [--page-token <token>]\n\nRead blockers retained for the exact scoped attempt."
        }
        "history.item" => {
            "Usage:\n  podway history item --session <uuid> --attempt <scoped-attempt-json> --item <item-id> [--archived] [--page-token <token>]\n\nRead a bounded retained value page. Use evidence read to inspect evidence for the current attempt."
        }
        "session.start" => {
            "Usage:\n  podway start (--preset <name> | --procedure <file> [--expect-procedure-digest <sha256:hex>]) --task <title> [--on-existing <preserve|delete>] [--supersede-reason <text>] [--actor <text>] [--progress-summary <text>] [--dry-run] [--with-observation] [--if-session-id <uuid> --if-session-revision <n>]\n\nExamples:\n  podway start --preset sw-dev-v3 --task 'implement feature'\n  podway start --preset sw-dev-v3 --task 'start successor' --on-existing preserve --supersede-reason 'The successor has priority.'\n  podway start --preset sw-dev-v3 --task 'replace running work' --on-existing delete --progress-summary 'The current progress was reviewed.' --yes\n  podway start --preset bug-fix-v3 --task 'preview deletion' --dry-run --on-existing delete\n\nThe paired optional --if-session-id and --if-session-revision fences are verified against the observed current session inside the start transaction and never prompt; supply both or neither, and --if-attempt does not apply. Plain start does not auto-fill omitted fences; the replacement path keeps its preflight-derived fences. Unfenced human TTY mode interviews when a nonterminal session already exists; Enter continues it and creates nothing. Noninteractive callers must choose preserve or delete. Disposed terminal sessions archive automatically. A successful start creates a prepared session at revision 0 without a cursor, attempt, or goal. Plain non-dry-run start also accepts --with-observation to return podway.mutation-observation-result/v2 with an embedded v8 observation."
        }
        "session.begin" => {
            "Usage:\n  podway begin [--goal <text> --criterion <id>=<statement>...] [--actor <text>] [--with-observation] [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision 0]\n\nExamples:\n  podway begin\n  podway begin --goal 'Ship safely.' --criterion tested='Tests pass.' --actor developer --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 0\n\nBegin atomically creates attempt 1 at the entry node, optionally creates goal revision 1, and changes the prepared session to running. Omitted fences are filled from a fresh status preflight. Add --with-observation to return podway.mutation-observation-result/v2 with an embedded v8 observation."
        }
        "session.decide" => {
            "Usage:\n  podway decide --option <id> --reason <text> [--actor <text>] [--with-observation] --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision <n> --if-attempt <uuid> [--if-goal-revision <n>]\n\nThe goal revision fence is required for a session-goal assessment decision and, when supplied for a general decision, must match the current goal.\n\nExample:\n  podway decide --option approve --reason 'The evidence supports this route.' --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 7 --if-attempt <uuid>"
        }
        "session.rework" => {
            "Usage:\n  podway rework --to <graph-node-id> --reason <text> [--actor <text>] --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision <n> [--if-attempt <uuid>]\n\nExample:\n  podway rework --to implement --reason 'Review found a gap.' --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 7"
        }
        "goal.define" => {
            "Usage:\n  podway goal define --goal <text> --criterion <id>=<statement>... [--actor <text>] --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision <n>\n\nExample:\n  podway goal define --goal 'Ship safely.' --criterion tested='Tests pass.' --if-workspace-uuid <uuid> --if-session-id <uuid> --if-session-revision 7"
        }
        "goal.revise" => {
            "Usage:\n  podway goal revise --goal <text> --criterion <id>=<statement>... --rework-to <base-anchor> --reason <text> --basis-description <text> --basis-digest <sha256> --feedback <text> [--actor <text>] [--reactivate] --if-workspace-uuid <uuid> --if-cursor <json> --idempotency-key <key>\n\nUse the base's design anchor as the rework target after group activation. Copy the scoped cursor from generation-2 observe. Record the working-copy basis, why the goal changed, and how to treat prior work. Use --reactivate only for a completed goal requiring a new attempt."
        }
        "goal.assess_criterion" => {
            "Usage:\n  podway goal assess-criterion <criterion-id> --status <satisfied|unsatisfied|not_applicable> --reason <text> [--evidence <selector>]... [--item <item-id>]... [--actor <text>] --if-workspace-uuid <uuid> --if-cursor <json> --idempotency-key <key>\n\nUse a currently resolved local node ID, @plan or @execution-result as an evidence selector. Copy the whole current result.cursor from observe when supplying a fence."
        }
        "goal.assess_criteria" => {
            r#"Usage:
  podway goal assess-criteria --stdin [--with-observation] [--worktree <path>] [--json]

Submit one podway.scoped-criterion-assessment-input/v1 JSON document. Use the
generation-2 observe stdin_template, supply the caller-owned key and assessments,
and keep the current workspace and cursor fences. The goal revision comes from
result.cursor.goal_revision. Each of 1..16 assessments supplies criterion,
status, reason, and up to four scoped citations. The batch commits one revision
or leaves the session unchanged.

Example stdin document:
  {"schema":"podway.scoped-criterion-assessment-input/v1","context":{"workspace_uuid":"00000000-0000-4000-8000-000000000001","idempotency_key":"assess-example","cursor":{"attempt":{"session":"00000000-0000-4000-8000-000000000002","scope":{"kind":"base"},"base_digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","node":"assessment","attempt":"00000000-0000-4000-8000-000000000003"},"session_revision":7,"goal_revision":1,"selected_group":null}},"assessments":[{"criterion":"tested","status":"satisfied","reason":"The test passed.","citations":[{"kind":"execution_result"}]}]}"#
        }
        "session.start_replace" => {
            "Use `podway start --on-existing preserve|delete` for session.start_replace. See `podway help session.start` for source and concurrency fences."
        }
        "session.status" => {
            "Usage:\n  podway status [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--verbose [--history-before <trace-sequence>]] [--compact]\n\nUse --compact for bounded cursor and item metadata. Use --verbose for the current observation with retained attempts in descending trace order. Pass its history_before to request the next page."
        }
        "session.next" => {
            "Usage:\n  podway next [--if-workspace-uuid <uuid>] [--if-session-id <uuid>]\n\nExample:\n  podway next"
        }
        "session.observe" => {
            "Usage:\n  podway --json observe [--if-workspace-uuid <uuid>] [--if-session-id <uuid>]\n\nRead the current scoped cursor, active guidance, item revisions and group summary. Generation 1 serves observation-result/v7. Generation 2 serves observation-result/v8 with executable templates. Copy the exact cursor and workspace UUID for a fenced mutation. Retained decisions and former attempts are available through group and history queries. A template carries concurrency fences and placeholders for caller-owned data."
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
            "Usage:\n  podway complete [--with-observation] [--if-workspace-uuid <uuid>] [--if-session-id <uuid>] [--if-session-revision <n>] [--if-attempt <uuid>]\n\nExample:\n  podway complete --if-session-revision 12 --if-attempt <uuid>"
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
            r#"Usage:
  podway record --stdin [--with-observation] [--worktree <path>] [--json]

Submit one podway.scoped-item-record-many-input/v1 document. Use the
generation-2 observe stdin_template. Preserve context.workspace_uuid and the
exact scoped attempt, session revision, goal revision, and selected group in
context.cursor. Supply a caller-owned context.idempotency_key and 1..128
distinct operations. Copy each expected_item_revision from result.items.
Supply either a typed record or clear: true for each operation.

The artifact record accepts path with optional media_type, or reference with
digest, size_bytes and media_type. A check_result carries the declared
operation_id and operation_digest, input_basis: {descriptor, digest},
executor: {name, version}, outcome, summary, and a digest of the exact check-output bytes.
The batch commits atomically. Podway records caller assertions.

Example stdin document:
  {"schema":"podway.scoped-item-record-many-input/v1","context":{"workspace_uuid":"00000000-0000-4000-8000-000000000001","idempotency_key":"record-example","cursor":{"attempt":{"session":"00000000-0000-4000-8000-000000000002","scope":{"kind":"base"},"base_digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","node":"plan","attempt":"00000000-0000-4000-8000-000000000003"},"session_revision":3,"goal_revision":null,"selected_group":null}},"operations":[{"item_id":"summary","expected_item_revision":1,"record":{"type":"text","value":"Recorded."}}]}

Example:
  podway record --stdin < record.json"#
        }
        "integration.show" => {
            "Usage:\n  podway integration show dolgorae [--json]\n\nRead effective integration permission and its source (project > user > false). Works before init and never opens SQLite or a maintenance lease. Only this query reads $HOME/.config/podway/config.yaml. Enabled permission does not establish connection readiness, capability or execution. An external adapter must adopt this query before new assignments. Disabled permission does not cancel accepted work or disable result lookup, reconciliation, receipts, acceptance or explicit completion. No stdin document is read. Idempotency keys, workspace/session identities, revision/attempt/item/goal fences and destructive flags are rejected."
        }
        "outcome.lookup" => {
            "Usage:\n  podway outcome lookup --idempotency-key <key> [--if-workspace-uuid <uuid>] [--json]\n\nRead the original committed success response without replaying or admitting a mutation."
        }
        _ => return None,
    };
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls002_missing_required_context_never_supplies_untrusted_labels() {
        use clap::error::{ContextKind, ContextValue, ErrorKind};

        let arguments = ["podway", "start", "--preset", "private-value"].map(OsString::from);
        let mut error = Cli::try_parse_from(&arguments).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
        assert_eq!(
            syntax_failure_message(&error, &arguments),
            "missing required argument: --task <TITLE>"
        );
        error.insert(
            ContextKind::InvalidArg,
            ContextValue::Strings(vec!["private-value".to_owned()]),
        );
        assert_eq!(
            syntax_failure_message(&error, &arguments),
            "invalid command syntax"
        );
        error.remove(ContextKind::InvalidArg);
        assert_eq!(
            syntax_failure_message(&error, &arguments),
            "invalid command syntax"
        );
    }

    #[test]
    fn calls001_help_completion_topics_resolve_through_the_shared_grammar() {
        let topics = help_completion_topics();
        for topic in &topics {
            let words = topic
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            assert!(
                help_text(&words).is_ok(),
                "completion topic does not resolve: {topic}"
            );
        }
        for topic in [
            "overview",
            "goal",
            "goal assess-criterion",
            "goal.assess_criterion",
            "reset --all",
            "workspace.reset_all",
            "workspace mode",
            "workspace.mode",
        ] {
            assert!(
                topics.iter().any(|candidate| candidate == topic),
                "missing {topic}"
            );
        }
        assert!(!topics.iter().any(|topic| topic.contains("__complete")));
    }

    const EXECUTABLE_WORD_ROUTES: &[(&[&str], &str)] = &[
        (&["help"], "help"),
        (&["version"], "version"),
        (&["completions"], "completions"),
        (&["procedure", "validate"], "procedure.validate"),
        (&["procedure", "show"], "procedure.show"),
        (&["procedure", "format"], "procedure.format"),
        (&["procedure", "vet"], "procedure.vet"),
        (&["procedure", "graph"], "procedure.graph"),
        (&["procedure", "preview"], "procedure.preview"),
        (&["procedure", "lint"], "procedure.lint"),
        (&["procedure", "check"], "procedure.check"),
        (&["procedure", "scaffold"], "procedure.scaffold"),
        (&["preset", "list"], "preset.list"),
        (&["preset", "show"], "preset.show"),
        (&["preset", "explain"], "preset.explain"),
        (&["init"], "workspace.init"),
        (&["doctor"], "workspace.doctor"),
        (&["workspace", "show"], "workspace.show"),
        (&["workspace", "repair"], "workspace.repair"),
        (&["workspace", "remove"], "workspace.remove"),
        (&["start"], "session.start"),
        (&["begin"], "session.begin"),
        (&["status"], "session.status"),
        (&["next"], "session.next"),
        (&["observe"], "session.observe"),
        (&["evidence", "read"], "evidence.read"),
        (&["delegation", "package"], "delegation.package"),
        (&["delegation", "accept"], "delegation.accept"),
        (&["delegation", "receipt"], "delegation.receipt"),
        (&["complete"], "session.complete"),
        (&["decide"], "session.decide"),
        (&["rework"], "session.rework"),
        (&["goal", "define"], "goal.define"),
        (&["goal", "revise"], "goal.revise"),
        (&["goal", "assess-criterion"], "goal.assess_criterion"),
        (&["goal", "assess-criteria"], "goal.assess_criteria"),
        (&["group", "activate"], "group.activate"),
        (&["group", "return"], "group.return"),
        (&["group", "replan"], "group.replan"),
        (&["group", "list"], "group.list"),
        (&["group", "show"], "group.show"),
        (&["group", "prior-work"], "group.prior_work"),
        (&["history", "dag"], "history.dag"),
        (&["history", "attempts"], "history.attempts"),
        (&["history", "blockers"], "history.blockers"),
        (&["history", "item"], "history.item"),
        (&["history", "goal-revisions"], "history.goal_revisions"),
        (&["skip"], "session.skip"),
        (&["retry"], "session.retry"),
        (&["block"], "session.block"),
        (&["unblock"], "session.unblock"),
        (&["cancel"], "session.cancel"),
        (
            &["disposition", "handed-off"],
            "session.terminal_disposition",
        ),
        (
            &["disposition", "not-required"],
            "session.terminal_disposition",
        ),
        (&["archive"], "session.archive"),
        (&["archive", "list"], "session.archive_list"),
        (&["archive", "show"], "session.archive_show"),
        (&["archive", "purge"], "session.archive_purge"),
        (&["reset"], "session.reset"),
        (&["check"], "item.check"),
        (&["uncheck"], "item.uncheck"),
        (&["set"], "item.set"),
        (&["add"], "item.add"),
        (&["remove"], "item.remove"),
        (&["attach"], "item.attach"),
        (&["clear"], "item.clear"),
        (&["record"], "item.record_many"),
        (&["outcome", "lookup"], "outcome.lookup"),
        (&["integration", "show"], "integration.show"),
    ];

    const GROUP_WORDS: &[&str] = &[
        "procedure",
        "preset",
        "workspace",
        "evidence",
        "delegation",
        "goal",
        "history",
        "group",
        "disposition",
        "integration",
        "outcome",
    ];

    const FLAG_SELECTED_ROUTES: &[(&[&str], &str)] = &[
        (
            &["start", "--on-existing", "preserve"],
            "session.start_replace",
        ),
        (&["reset", "--all"], "workspace.reset_all"),
    ];

    fn owned(words: &[&str]) -> Vec<String> {
        words.iter().copied().map(|word| word.to_owned()).collect()
    }

    fn tree_executable_paths(
        scope: &clap::Command,
        path: &mut Vec<String>,
        paths: &mut Vec<Vec<String>>,
    ) {
        for child in scope.get_subcommands().filter(|child| !child.is_hide_set()) {
            path.push(child.get_name().to_owned());
            if child.is_subcommand_required_set() {
                tree_executable_paths(child, path, paths);
            } else {
                paths.push(path.clone());
                tree_executable_paths(child, path, paths);
            }
            path.pop();
        }
    }

    #[test]
    fn guide003_word_routes_match_the_clap_tree() {
        let mut grammar = Cli::command();
        grammar.build();
        let mut tree_paths = Vec::new();
        tree_executable_paths(&grammar, &mut Vec::new(), &mut tree_paths);
        let mut expected: Vec<Vec<String>> = EXECUTABLE_WORD_ROUTES
            .iter()
            .map(|(words, _)| owned(words))
            .collect();
        tree_paths.sort();
        expected.sort();
        assert_eq!(tree_paths, expected);

        let groups: Vec<&str> = grammar
            .get_subcommands()
            .filter(|child| !child.is_hide_set() && child.is_subcommand_required_set())
            .map(|child| child.get_name())
            .collect();
        assert_eq!(groups, GROUP_WORDS);

        for (words, route) in EXECUTABLE_WORD_ROUTES {
            let via_words = help_text(&owned(words)).unwrap();
            let via_route = help_text(&[route.to_string()]).unwrap();
            assert_eq!(via_words, via_route, "{words:?}");
        }
        for (words, route) in FLAG_SELECTED_ROUTES {
            let argv: Vec<OsString> = std::iter::once("podway")
                .chain(words.iter().copied())
                .map(OsString::from)
                .collect();
            assert_eq!(
                parse_failure_command_context(&argv)
                    .expect("flag-selected words parse")
                    .route,
                *route,
                "{words:?}"
            );
            assert!(help_text(&[route.to_string()]).is_ok(), "{route}");
        }
        for guide in GUIDE_TOPICS {
            assert!(help_text(&[guide.to_string()]).is_ok(), "{guide}");
        }
    }

    #[test]
    fn guide003_help_topics_resolve_words_groups_and_unknown_topics() {
        for (words, route) in [
            (&["goal", "define"][..], "goal.define"),
            (&["record"][..], "item.record_many"),
            (&["init"][..], "workspace.init"),
            (&["rework"][..], "session.rework"),
            (
                &["disposition", "not-required"][..],
                "session.terminal_disposition",
            ),
        ] {
            assert_eq!(
                help_text(&owned(words)).unwrap(),
                help_text(&[route.to_string()]).unwrap(),
                "{words:?}"
            );
        }
        let rework = help_text(&["rework".to_owned()]).unwrap();
        assert!(rework.contains("Usage:") && rework.contains("--to <graph-node-id>"));

        let goal = help_text(&["goal".to_owned()]).unwrap();
        assert_eq!(
            goal,
            "goal command group:\n  goal define -> goal.define\n  goal revise -> goal.revise\n  goal assess-criterion -> goal.assess_criterion\n  goal assess-criteria -> goal.assess_criteria"
        );
        for group in GROUP_WORDS {
            let listing = help_text(&[group.to_string()]).unwrap();
            assert!(
                listing.starts_with(&format!("{group} command group:")),
                "{group}"
            );
        }
        let disposition = help_text(&["disposition".to_owned()]).unwrap();
        assert!(disposition.contains("disposition not-required -> session.terminal_disposition"));

        assert!(
            help_text(&["daemon".to_owned()])
                .unwrap()
                .contains("retired")
        );
        assert!(
            help_text(&owned(&["runtime", "reset"]))
                .unwrap()
                .contains("retired")
        );
        assert!(
            help_text(&owned(&["job", "lookup"]))
                .unwrap()
                .contains("outcome.lookup")
        );

        for words in [&["bogus"][..], &["goal", "bogus"][..], &["__complete"][..]] {
            let failure = help_text(&owned(words)).unwrap_err();
            assert_eq!(failure.code, "REQUEST_INVALID", "{words:?}");
            assert!(failure.message.contains("podway help"), "{words:?}");
        }
    }

    #[test]
    fn guide003_overview_lists_the_complete_topic_index() {
        let overview = help_text(&[]).unwrap();
        assert_eq!(overview, help_text(&["overview".to_owned()]).unwrap());
        assert!(overview.contains("Direct execution:"));
        assert!(overview.contains("podway --help"));
        for (words, route) in EXECUTABLE_WORD_ROUTES {
            assert!(
                overview.contains(&format!("  {} -> {route}", words.join(" "))),
                "{words:?}"
            );
        }
        assert!(overview.contains("  archive -> session.archive"));
        assert!(overview.contains("  archive list -> session.archive_list"));
        assert!(overview.contains("  start --on-existing -> session.start_replace"));
        assert!(overview.contains("  reset --all -> workspace.reset_all"));
        assert!(overview.contains(&format!("  guides: {}", GUIDE_TOPICS.join(", "))));
        assert!(overview.contains(&format!("  command groups: {}", GROUP_WORDS.join(", "))));
        assert!(overview.contains(&format!(
            "  retired families: {}",
            RETIRED_FAMILY_TOPICS.join(", ")
        )));
    }

    #[test]
    fn guide009_contract_generation_topic_alias_and_overview() {
        let topic = help_text(&["contract-generations".to_owned()]).unwrap();
        assert!(topic.contains("Contract generations:"));
        assert!(topic.contains("--contract-generation <1|2>"));
        assert!(topic.contains("podway.observation-result/v8"));
        // The generation-2 error details are emitted (GUIDE-010); pin the
        // wrap-safe tokens and the absence of the retired interim sentence.
        assert!(topic.contains("CRITERION_CITATION_INVALID"));
        assert!(topic.contains("WORKSPACE_CONTRACT_MISMATCH"));
        assert!(!topic.contains("currently keep their"));
        assert!(topic.contains("No environment or configuration channel"));

        let overview = help_text(&[]).unwrap();
        assert!(overview.contains("Contract generations:"));
        assert!(overview.contains("Local commands also accept --contract-generation."));
        assert!(overview.contains("observation-result/v8"));
        assert!(overview.contains("CRITERION_CITATION_INVALID"));
        assert!(overview.contains("WORKSPACE_CONTRACT_MISMATCH"));
        assert!(!overview.contains("currently keep their"));
        assert!(overview.contains(&format!("  guides: {}", GUIDE_TOPICS.join(", "))));

        // The alias rewrites only the help token; the valued generation pair
        // survives, so an invalid value before -h still reaches validation.
        let mut arguments = vec![
            OsString::from("podway"),
            OsString::from("--contract-generation"),
            OsString::from("3"),
            OsString::from("-h"),
        ];
        normalize_top_level_help_alias(&mut arguments);
        assert_eq!(
            arguments,
            vec![
                OsString::from("podway"),
                OsString::from("--contract-generation"),
                OsString::from("3"),
                OsString::from("help"),
            ]
        );
        let mut equals_form = vec![
            OsString::from("podway"),
            OsString::from("--contract-generation=2"),
            OsString::from("--json"),
            OsString::from("--help"),
        ];
        normalize_top_level_help_alias(&mut equals_form);
        assert_eq!(equals_form[3], OsString::from("help"));

        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let offered = match shell {
                Shell::Bash | Shell::Zsh => shell.script().contains("--contract-generation"),
                Shell::Fish => shell.script().contains("-l contract-generation"),
            };
            assert!(offered, "{shell:?} omits the contract generation flag");
        }
    }

    #[test]
    fn guide011_contract_generation_deprecation_topic_and_overview() {
        let topic = help_text(&["contract-generations".to_owned()]).unwrap();
        assert!(topic.contains("Generation 1 is the default and is deprecated"));
        assert!(topic.contains("no runtime warning is emitted"));
        assert!(!topic.contains("while it stays supported"));

        let overview = help_text(&[]).unwrap();
        assert!(overview.contains("Generation 1 is the default and is deprecated"));
        assert!(overview.contains("no runtime warning is emitted"));
        assert!(!overview.contains("the default is 1."));

        let observation = help_text(&["session.observe".to_owned()]).unwrap();
        assert!(observation.contains("Generation 1 serves observation-result/v7"));
        assert!(
            observation
                .contains("Generation 2 serves observation-result/v8 with executable templates")
        );
    }

    #[test]
    fn guide004_record_help_documents_sources_and_decodes_example() {
        let text = help_text(&owned(&["record"])).unwrap();
        assert_eq!(text, help_text(&["item.record_many".to_owned()]).unwrap());
        for phrase in [
            "context.workspace_uuid",
            "context.cursor",
            "context.idempotency_key",
            "expected_item_revision from result.items",
            "typed record or clear: true",
            "exact check-output bytes",
        ] {
            assert!(text.contains(phrase), "missing {phrase}");
        }
        let example = text
            .lines()
            .map(str::trim_start)
            .find(|line| line.starts_with(r#"{"schema":"#))
            .unwrap();
        let decoded =
            podway_protocol::decode_scoped_item_record_many_input_v1(example.as_bytes()).unwrap();
        assert_eq!(
            decoded.context.workspace_uuid.as_str(),
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(decoded.context.idempotency_key.as_str(), "record-example");
        assert_eq!(decoded.context.cursor.session_revision.get(), 3);
        assert_eq!(
            decoded.context.cursor.attempt.attempt().as_str(),
            "00000000-0000-4000-8000-000000000003"
        );
        assert_eq!(decoded.operations[0].item_id.as_str(), "summary");
        assert_eq!(decoded.operations[0].expected_item_revision.get(), 1);
    }

    #[test]
    fn guide004_observe_help_documents_fences_guidance_templates_and_errors() {
        let text = help_text(&owned(&["observe"])).unwrap();
        assert_eq!(text, help_text(&["session.observe".to_owned()]).unwrap());
        assert!(text.contains("Usage:"));
        for phrase in [
            "current scoped cursor",
            "--if-workspace-uuid",
            "--if-cursor",
            "--if-item-revision",
            "Generation 1 serves observation-result/v7",
            "Generation 2 serves observation-result/v8 with executable templates",
            "Retained decisions and former attempts are available through group and history queries",
            "concurrency fences and placeholders for caller-owned data",
        ] {
            assert!(text.contains(phrase), "missing: {phrase}");
        }
    }

    #[test]
    fn guide004_init_help_separates_tracked_files_from_ignored_runtime() {
        let text = help_text(&owned(&["init"])).unwrap();
        assert_eq!(text, help_text(&["workspace.init".to_owned()]).unwrap());
        assert!(text.contains("Usage:"));
        for phrase in [
            ".podway/config.yaml",
            ".podway/procedures/",
            ".podway/.gitignore",
            "ignores runtime/",
            ".podway/runtime/",
        ] {
            assert!(text.contains(phrase), "missing: {phrase}");
        }
    }

    #[test]
    fn agent003_goal_assess_criteria_help_example_decodes_and_names_status_goal_revision() {
        let text = help_text(&owned(&["goal", "assess-criteria"])).unwrap();
        assert_eq!(
            text,
            help_text(&["goal.assess_criteria".to_owned()]).unwrap()
        );
        assert!(text.contains("result.cursor.goal_revision"));
        let example = text
            .lines()
            .map(str::trim_start)
            .find(|line| line.starts_with(r#"{"schema":"#))
            .expect("example stdin document line");
        let decoded = podway_protocol::decode_scoped_criterion_input_v1(example.as_bytes())
            .expect("the help example decodes with the current stdin decoder");
        assert_eq!(
            decoded.context.workspace_uuid.as_str(),
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(decoded.context.idempotency_key.as_str(), "assess-example");
        assert_eq!(decoded.context.cursor.session_revision.get(), 7);
        assert_eq!(decoded.context.cursor.goal_revision, Some(1));
        assert_eq!(
            decoded.context.cursor.attempt.attempt().as_str(),
            "00000000-0000-4000-8000-000000000003"
        );
        assert_eq!(decoded.entries[0].criterion.as_str(), "tested");
        assert_eq!(decoded.entries[0].status.as_str(), "satisfied");
        assert_eq!(decoded.entries[0].citations.len(), 1);
    }
}
