//! Private, explicit child mode for the GROUP-005 process fixtures.

use crate::int_group002_transactions::{TIME, definition, initial_state};
use podway_config::ProcedureDocumentFormat;
use podway_core::execution_groups::*;
use podway_core::{
    AttemptId, CheckResultInputBasisV2, CriterionId, DomainError, GoalCriterionV2,
    GoalDefinitionV2, GoalOutcome, GoalRevisionNumberV2, GoalStatementV2, GraphMutationErrorV2,
    GraphNodeId, ItemId, OptionId, ReasonV2, RecordedItemValueV2, Revision, SessionId,
    Sha256Digest, UnixMillis, WorkspaceId,
};
use podway_engine::direct_execution::{DirectIdSourceV1, NativeDirectIdSourceV1};
use podway_engine::successor_execution::*;
use podway_protocol::RequestIdV1;
use podway_store::direct_store::{
    DirectMutationResultV1, DirectOutcomeScopeV1, DirectRecordedOutcomeV1, DirectStoreErrorV1,
};
use podway_store::successor_store::{
    CandidateCommitBoundary, CandidateExecutionStore, CandidateFixtureSession, CandidateStoreError,
};
use podway_store::{IdempotencyKeyV1, ValidatedWorkspaceRootV1};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const CHILD_ENV: &str = "PODWAY_GROUP005_FIXTURE_REQUEST";
const CHILD_TEST: &str = "int_group005_processes::driver::engine_child";
const PREFIX: &str = "podway-group005-process-";
const STORE: &str = "candidate.sqlite3";
const WAIT: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(4);
const MAX_MESSAGE: u64 = 8 * 1024 * 1024;
const CONTROLLED_EXIT: i32 = 91;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    pub revision: Revision,
    pub attempt: Option<ScopedAttemptV1>,
    pub goal: Option<u64>,
    pub selected: Option<GroupExecutionIdV1>,
}

impl Cursor {
    fn from_state(state: &ExecutionSessionV3) -> Self {
        let cursor = SuccessorCursor::from_state(state);
        Self {
            revision: cursor.execution.revision,
            attempt: cursor.execution.attempt,
            goal: cursor
                .execution
                .goal_revision
                .map(GoalRevisionNumberV2::get),
            selected: cursor.selected_group,
        }
    }

    fn fence(&self, session: &SessionId) -> SuccessorCursor {
        SuccessorCursor {
            execution: ExecutionCursorFenceV1 {
                session: session.clone(),
                revision: self.revision,
                attempt: self.attempt.clone(),
                goal_revision: self.goal.map(GoalRevisionNumberV2::new),
            },
            selected_group: self.selected.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Plan {
    attempt: AttemptId,
    digest: Sha256Digest,
    items: Vec<(ItemId, Revision, Sha256Digest)>,
}

impl Plan {
    fn from_binding(plan: &PlanBindingV1) -> Self {
        Self {
            attempt: plan.design_attempt().clone(),
            digest: plan.item_set_digest().clone(),
            items: plan
                .items()
                .iter()
                .map(|item| {
                    (
                        item.item().clone(),
                        item.revision(),
                        item.value_digest().clone(),
                    )
                })
                .collect(),
        }
    }

    fn into_binding(self) -> PlanBindingV1 {
        PlanBindingV1::new(
            self.attempt,
            self.digest,
            self.items
                .into_iter()
                .map(|(item, revision, digest)| {
                    PlanItemBindingV1::new(item, revision, digest).unwrap()
                })
                .collect(),
        )
        .unwrap()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PriorWork {
    pub execution: GroupExecutionIdV1,
    pub item: Option<HistoricalReferenceV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Edit {
    pub item: String,
    pub expected: u64,
    pub value: ItemValue,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) enum ItemValue {
    Text(String),
    Choice(String),
    Number(i64),
}

impl Edit {
    fn into_write(self) -> GroupItemWriteV1 {
        let value = match self.value {
            ItemValue::Text(value) => RecordedItemValueV2::text(value).unwrap(),
            ItemValue::Choice(value) => RecordedItemValueV2::choice(value).unwrap(),
            ItemValue::Number(value) => RecordedItemValueV2::integer(value),
        };
        GroupItemWriteV1 {
            item: ItemId::new(self.item).unwrap(),
            expected_revision: Revision::new(self.expected),
            value: Some(value),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Action {
    Activate {
        document: Value,
        digest: Sha256Digest,
        plan: Plan,
        predecessor: Option<GroupExecutionIdV1>,
        prior_work: Vec<PriorWork>,
    },
    Record(Edit),
    Batch(Vec<Edit>),
    Complete,
    Return,
    Replan,
    DefineGoal(String),
    ReviseGoal {
        title: String,
        reactivate: bool,
    },
    DecideAchieved,
    Cancel,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Call {
    pub cursor: Cursor,
    pub key: String,
    pub response_id: String,
    pub working_copy_digest: Sha256Digest,
    pub now: UnixMillis,
    pub action: Action,
}

impl Call {
    pub fn new(probe: &Probe, key: &str, action: Action) -> Self {
        Self {
            cursor: probe.cursor.clone(),
            key: key.into(),
            response_id: NativeDirectIdSourceV1.next_session_id().into_inner(),
            working_copy_digest: probe.working_copy_digest.clone(),
            now: probe.now,
            action,
        }
    }

    pub fn retry(&self) -> Self {
        Self {
            response_id: NativeDirectIdSourceV1.next_session_id().into_inner(),
            ..self.clone()
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
enum Operation {
    Read(Vec<String>),
    Inspect(Vec<String>),
    History(Box<HistoryRequest>),
    Lifecycle(Box<Lifecycle>),
    Grow {
        document: Box<Value>,
        digest: Sha256Digest,
        count: u8,
    },
    Mutation(Box<Call>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) enum Location {
    Current,
    Archived,
}

impl From<Location> for CandidateSessionLocation {
    fn from(value: Location) -> Self {
        match value {
            Location::Current => Self::Current,
            Location::Archived => Self::Archived,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Target {
    pub workspace: WorkspaceId,
    pub session: SessionId,
    pub location: Location,
    pub revision: Revision,
}

impl Target {
    fn scope(&self) -> CandidateFixtureSession<'_> {
        CandidateFixtureSession {
            workspace: &self.workspace,
            session: &self.session,
            location: self.location.into(),
            revision: self.revision,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) enum HistoryQuery {
    Groups,
    Group(GroupExecutionIdV1),
    GoalRevisions,
    Dag(Option<GroupExecutionIdV1>, Sha256Digest),
    Attempts(Option<GroupExecutionIdV1>, Sha256Digest),
    Blockers(ScopedAttemptV1),
    Item(ScopedAttemptV1, ItemId),
    PriorWork(GroupExecutionIdV1, Sha256Digest),
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct HistoryRequest {
    pub target: Target,
    pub query: HistoryQuery,
    pub limit: usize,
    pub continuation: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) enum Lifecycle {
    Archive(Target),
    Remove(Target),
    Install(Value, Option<SessionId>),
    InstallWithEntry(Value, SessionId, AttemptId),
    Reset(Target, Value),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(super) enum Mode {
    Normal,
    Contender,
    ParkBeforeCommit,
    ParkAfterCommit,
    ExitBeforeCommit,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Input {
    workspace: WorkspaceId,
    session: SessionId,
    mode: Mode,
    operation: Operation,
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(super) struct Outcome {
    pub response: String,
    pub request_digest: Sha256Digest,
    pub sequence: u64,
    pub session: SessionId,
    pub command: String,
    pub recorded_at: u64,
}

impl Outcome {
    fn new(outcome: &DirectRecordedOutcomeV1) -> Self {
        let DirectOutcomeScopeV1::Session(session) = outcome.scope() else {
            panic!("expected session outcome")
        };
        Self {
            response: outcome.response().as_str().into(),
            request_digest: outcome.digest().clone(),
            sequence: outcome.sequence(),
            session: session.clone(),
            command: outcome.command().into(),
            recorded_at: outcome.recorded_at().get(),
        }
    }

    pub fn document(&self) -> Value {
        serde_json::from_str(&self.response).unwrap()
    }
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(super) enum Refusal {
    Revision {
        expected: Revision,
        actual: Revision,
    },
    ItemRevision {
        expected: Revision,
        actual: Revision,
    },
    Attempt {
        expected: AttemptId,
        actual: Option<AttemptId>,
    },
    Selection {
        expected: Option<GroupExecutionIdV1>,
        actual: Option<GroupExecutionIdV1>,
    },
    ResponseBudget,
    OutcomeLookupBudget,
    Bound {
        field: String,
        actual: u64,
        maximum: u64,
    },
    Workspace,
    MissingSession,
    ArchivedSession,
    ArchivePhase,
    StaleHistoryToken,
    Domain(String),
    Unexpected(String),
}

impl From<SuccessorExecutionError> for Refusal {
    fn from(error: SuccessorExecutionError) -> Self {
        match error {
            SuccessorExecutionError::Store(CandidateStoreError::Domain(error)) => {
                Self::from(SuccessorExecutionError::Domain(error))
            }
            SuccessorExecutionError::Store(CandidateStoreError::Direct(
                DirectStoreErrorV1::ExpectedWorkspaceUuidMismatch { .. },
            )) => Self::Workspace,
            SuccessorExecutionError::Domain(DomainError::SessionIdentityMismatch { .. }) => {
                Self::MissingSession
            }
            SuccessorExecutionError::Domain(DomainError::BoundExceeded {
                field,
                actual,
                maximum,
                ..
            }) => Self::Bound {
                field: field.into(),
                actual,
                maximum,
            },
            SuccessorExecutionError::Store(CandidateStoreError::ArchiveRequiresTerminalSession) => {
                Self::ArchivePhase
            }
            SuccessorExecutionError::Store(CandidateStoreError::ArchiveRevisionMismatch {
                expected,
                actual,
            }) => Self::Revision { expected, actual },
            SuccessorExecutionError::Store(CandidateStoreError::ArchivedSession) => {
                Self::ArchivedSession
            }
            SuccessorExecutionError::StaleHistoryPageToken => Self::StaleHistoryToken,
            SuccessorExecutionError::Domain(DomainError::PreconditionFailed {
                expected,
                actual,
            }) => Self::Revision { expected, actual },
            SuccessorExecutionError::ItemMutation(GraphMutationErrorV2::ItemRevisionConflict {
                expected,
                actual,
            }) => Self::ItemRevision { expected, actual },
            SuccessorExecutionError::Domain(DomainError::AttemptNotCurrent {
                expected,
                actual,
            }) => Self::Attempt { expected, actual },
            SuccessorExecutionError::SelectedActivationMismatch { expected, actual } => {
                Self::Selection { expected, actual }
            }
            SuccessorExecutionError::ResponseBudget => Self::ResponseBudget,
            SuccessorExecutionError::OutcomeLookupBudget => Self::OutcomeLookupBudget,
            SuccessorExecutionError::Domain(DomainError::InvalidState { reason }) => {
                Self::Domain(reason.into())
            }
            error => Self::Unexpected(format!("{error:?}")),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub(super) struct Probe {
    pub cursor: Cursor,
    pub now: UnixMillis,
    pub plan: Option<Plan>,
    pub groups: Vec<Value>,
    pub attempts: Vec<Value>,
    pub goal_history: Vec<Value>,
    pub goal_outcome: Option<String>,
    pub observation: Result<Value, Refusal>,
    pub ledger: (u64, u64, u64),
    pub fingerprint: Sha256Digest,
    pub working_copy_digest: Sha256Digest,
    pub outcomes: BTreeMap<String, Option<Outcome>>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub(super) struct Inspection {
    pub ledger: (u64, u64, u64),
    pub fingerprint: Sha256Digest,
    /// Definitions, sessions, activations, attempts, and retained goal revisions.
    pub retained: (u64, u64, u64, u64, u64),
    pub first_journal_sequence: Option<u64>,
    pub outcomes: BTreeMap<String, Option<Outcome>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) enum Reply {
    Success {
        replayed: bool,
        outcome: Outcome,
    },
    Rejected(Refusal),
    Read(Box<Probe>),
    Inspection(Box<Inspection>),
    History(String),
    Lifecycle,
    Installed {
        workspace: WorkspaceId,
        session: SessionId,
    },
}

#[derive(Debug, Deserialize, Serialize)]
struct Delivery {
    pid: u32,
    reply: Reply,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Passage {
    pub pid: u32,
    pub phase: String,
    pub sequence: u64,
    pub response_digest: Sha256Digest,
}

fn digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(bytes))).unwrap()
}

fn key(value: &str) -> IdempotencyKeyV1 {
    IdempotencyKeyV1::new(value).unwrap()
}

fn goal(statement: String) -> ExecutionGoalV3 {
    ExecutionGoalV3 {
        statement: GoalStatementV2::new(statement).unwrap(),
        criteria: GoalDefinitionV2::new(vec![
            GoalCriterionV2::new(
                CriterionId::new("known-work").unwrap(),
                "Retain applicable group work.",
            )
            .unwrap(),
        ])
        .unwrap(),
    }
}

fn execute(
    store: &mut CandidateExecutionStore,
    workspace: WorkspaceId,
    session: &SessionId,
    call: Call,
) -> Result<DirectMutationResultV1, SuccessorExecutionError> {
    let Call {
        cursor,
        key: id,
        response_id,
        working_copy_digest,
        now,
        action,
    } = call;
    let request = SuccessorMutationRequest {
        workspace,
        key: key(&id),
        request_id: RequestIdV1::new(response_id).unwrap(),
        cursor: cursor.fence(session),
    };
    let ids = &NativeDirectIdSourceV1;
    let basis = || {
        CheckResultInputBasisV2::new("Isolated Git fixture work", working_copy_digest.clone())
            .unwrap()
    };
    let reason = || ReasonV2::new("Exercise the exact admitted process transition.").unwrap();
    match action {
        Action::Activate {
            document,
            digest,
            plan,
            predecessor,
            prior_work,
        } => activate_group(
            store,
            GroupActivationRequest {
                mutation: request,
                document: document.to_string(),
                format: ProcedureDocumentFormat::Json,
                expected_definition: digest,
                plan: plan.into_binding(),
                predecessor,
                working_copy_basis: basis(),
                prior_work: prior_work
                    .into_iter()
                    .map(|work| {
                        PriorWorkDispositionV1::new(
                            work.execution,
                            work.item,
                            PriorWorkChoiceV1::Retain,
                            "Keep the exact earlier work.".into(),
                        )
                        .unwrap()
                    })
                    .collect(),
            },
            || now,
            ids,
        ),
        Action::Record(edit) => record_successor_item(
            store,
            SuccessorItemRequest {
                workspace: request.workspace,
                key: request.key,
                request_id: request.request_id,
                target: cursor.attempt.unwrap(),
                selected_group: cursor.selected,
            },
            edit.into_write(),
            || now,
        ),
        Action::Batch(edits) => record_successor_many(
            store,
            request,
            edits.into_iter().map(Edit::into_write).collect(),
            || now,
        ),
        Action::Return => return_from_group(store, request, || now, ids),
        Action::Replan => {
            let feedback = ReplanFeedbackV1::new(
                cursor.attempt.as_ref().unwrap().clone(),
                OptionId::new("redesign").unwrap(),
                reason().into_inner(),
                basis(),
                "Preserve the previous group work.".into(),
            )
            .unwrap();
            replan_group(store, request, feedback, || now, ids)
        }
        action => {
            let operation = match action {
                Action::Complete => SuccessorCursorMutation::Complete,
                Action::DefineGoal(title) => SuccessorCursorMutation::DefineGoal(goal(title)),
                Action::ReviseGoal { title, reactivate } => {
                    let basis = basis();
                    SuccessorCursorMutation::ReviseGoal(ExecutionGoalChangeV3 {
                        goal: goal(title),
                        target: GraphNodeId::new("design").unwrap(),
                        reactivate,
                        feedback: GroupClosureFeedbackV1::new(
                            reason().into_inner(),
                            basis,
                            "Start a fresh anchor.".into(),
                        )
                        .unwrap(),
                    })
                }
                Action::DecideAchieved => SuccessorCursorMutation::Decide {
                    option: OptionId::new("accept").unwrap(),
                    reason: reason(),
                    goal_outcome: Some(GoalOutcome::Achieved),
                },
                Action::Cancel => SuccessorCursorMutation::Cancel(reason()),
                _ => unreachable!("other child actions have a dedicated typed issuer"),
            };
            issue_cursor_mutation(store, request, operation, || now, ids)
        }
    }
}

fn history(store: &mut CandidateExecutionStore, request: HistoryRequest) -> Reply {
    let target = SuccessorHistorySession {
        workspace: &request.target.workspace,
        session: &request.target.session,
        location: request.target.location.into(),
    };
    let page = SuccessorHistoryPage {
        limit: request.limit,
        continuation: request.continuation.as_deref(),
    };
    let definition = |execution, digest| match execution {
        None => SuccessorHistoryDefinition::Base(digest),
        Some(execution) => SuccessorHistoryDefinition::Group { execution, digest },
    };
    let result = match &request.query {
        HistoryQuery::Groups => list_successor_groups(store, target, page),
        HistoryQuery::Group(execution) => show_successor_group(store, target, execution, page),
        HistoryQuery::GoalRevisions => list_successor_goal_revisions(store, target, page),
        HistoryQuery::Dag(execution, digest) => read_successor_history_dag(
            store,
            target,
            definition(execution.as_ref(), digest),
            page.continuation,
        ),
        HistoryQuery::Attempts(execution, digest) => list_successor_history_attempts(
            store,
            target,
            definition(execution.as_ref(), digest),
            page,
        ),
        HistoryQuery::Blockers(attempt) => {
            list_successor_history_blockers(store, target, attempt, page)
        }
        HistoryQuery::Item(attempt, item) => {
            read_successor_history_item(store, target, attempt, item, page.continuation)
        }
        HistoryQuery::PriorWork(execution, digest) => {
            list_successor_prior_work(store, target, execution, digest, page)
        }
    };
    match result {
        Ok(response) => Reply::History(response.as_str().into()),
        Err(error) => Reply::Rejected(error.into()),
    }
}

fn lifecycle(
    store: &mut CandidateExecutionStore,
    workspace: &WorkspaceId,
    operation: Lifecycle,
) -> Reply {
    let result = match operation {
        Lifecycle::Archive(target) if target.location == Location::Current => store
            .archive_fixture_session(
                &target.workspace,
                &target.session,
                target.revision,
                definition,
            ),
        Lifecycle::Archive(_) => return Reply::Rejected(Refusal::ArchivePhase),
        Lifecycle::Remove(target) => store.remove_fixture_session(target.scope(), definition),
        Lifecycle::Install(base, session) => {
            let requested =
                session.map(|session| (session, NativeDirectIdSourceV1.next_attempt_id()));
            return install(store, workspace, &base, requested);
        }
        Lifecycle::InstallWithEntry(base, session, entry) => {
            return install(store, workspace, &base, Some((session, entry)));
        }
        Lifecycle::Reset(target, base) => {
            let (initial, sources) = initial_state(&base, true);
            let replacement = NativeDirectIdSourceV1.next_workspace_id();
            return match store.reset_fixture_workspace(
                target.scope(),
                replacement.clone(),
                &initial,
                &sources,
                definition,
            ) {
                Ok(()) => Reply::Installed {
                    workspace: replacement,
                    session: initial.fence().session,
                },
                Err(error) => Reply::Rejected(SuccessorExecutionError::from(error).into()),
            };
        }
    };
    match result {
        Ok(()) => Reply::Lifecycle,
        Err(error) => Reply::Rejected(SuccessorExecutionError::from(error).into()),
    }
}

fn install(
    store: &mut CandidateExecutionStore,
    workspace: &WorkspaceId,
    base: &Value,
    requested: Option<(SessionId, AttemptId)>,
) -> Reply {
    let (mut initial, sources) = initial_state(base, true);
    if let Some((session, entry)) = requested {
        initial = ExecutionSessionV3::begin(
            session,
            initial.base_definition().clone(),
            initial.base_digest().clone(),
            None,
            entry,
            TIME,
        )
        .unwrap();
    }
    match store.install_fixture_session(workspace, &initial, &sources) {
        Ok(()) => Reply::Installed {
            workspace: workspace.clone(),
            session: initial.fence().session,
        },
        Err(error) => Reply::Rejected(SuccessorExecutionError::from(error).into()),
    }
}

fn grow(
    store: &mut CandidateExecutionStore,
    workspace: &WorkspaceId,
    session: &SessionId,
    db: &Path,
    document: &Value,
    definition_digest: Sha256Digest,
    count: u8,
) -> Result<(), SuccessorExecutionError> {
    assert!((1..=8).contains(&count));
    let working_copy_digest = digest(&fs::read(db.parent().unwrap().join("work.txt")).unwrap());
    let issue = |store: &mut CandidateExecutionStore, key: String, action| {
        let state = store.current_snapshot(workspace, session, definition)?.0;
        execute(
            store,
            workspace.clone(),
            session,
            Call {
                cursor: Cursor::from_state(&state),
                key,
                action,
                now: state.updated_at(),
                working_copy_digest: working_copy_digest.clone(),
                response_id: NativeDirectIdSourceV1.next_session_id().into_inner(),
            },
        )
        .map(|_| ())
    };
    for _ in 0..count {
        let state = store.current_snapshot(workspace, session, definition)?.0;
        let ordinal = state.group_history().len() + 1;
        let first = state
            .group_history()
            .first()
            .unwrap()
            .identity()
            .execution()
            .clone();
        let predecessor = state
            .group_history()
            .last()
            .unwrap()
            .identity()
            .execution()
            .clone();
        let plan = Plan::from_binding(&state.plan_binding()?);
        issue(
            store,
            format!("grow-{ordinal}-activate"),
            Action::Activate {
                document: document.clone(),
                digest: definition_digest.clone(),
                plan,
                predecessor: Some(predecessor),
                prior_work: vec![PriorWork {
                    execution: first,
                    item: None,
                }],
            },
        )?;
        let edits = [
            ("verdict", ItemValue::Choice("failure".into())),
            ("stage", ItemValue::Choice("scored".into())),
            ("score", ItemValue::Number(80)),
        ]
        .into_iter()
        .map(|(item, value)| Edit {
            item: item.into(),
            expected: 0,
            value,
        })
        .collect();
        issue(
            store,
            format!("grow-{ordinal}-assess"),
            Action::Batch(edits),
        )?;
        issue(store, format!("grow-{ordinal}-complete"), Action::Complete)?;
        issue(store, format!("grow-{ordinal}-replan"), Action::Replan)?;
        issue(
            store,
            format!("grow-{ordinal}-plan"),
            Action::Record(Edit {
                item: "plan".into(),
                expected: 0,
                value: ItemValue::Text(format!("Plan after group {ordinal}")),
            }),
        )?;
    }
    Ok(())
}

fn outcomes(
    store: &CandidateExecutionStore,
    workspace: &WorkspaceId,
    keys: Vec<String>,
) -> BTreeMap<String, Option<Outcome>> {
    keys.into_iter()
        .map(|value| {
            let result = lookup_successor_outcome(store, workspace, &key(&value))
                .unwrap()
                .as_ref()
                .map(Outcome::new);
            (value, result)
        })
        .collect()
}

fn inspect(
    store: &CandidateExecutionStore,
    workspace: &WorkspaceId,
    db: &Path,
    keys: Vec<String>,
) -> Inspection {
    let (ledger, fingerprint) = audit(db);
    let connection = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let retained = connection.query_row(
        "SELECT (SELECT count(*) FROM successor_definitions), (SELECT count(*) FROM successor_sessions),
                (SELECT count(*) FROM successor_activations), (SELECT count(*) FROM successor_attempts),
                (SELECT count(*) FROM successor_goal_revisions)",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    ).unwrap();
    let first_journal_sequence = connection
        .query_row(
            "SELECT min(workspace_sequence) FROM operational_journal",
            [],
            |row| row.get(0),
        )
        .unwrap();
    Inspection {
        ledger,
        fingerprint,
        retained,
        first_journal_sequence,
        outcomes: outcomes(store, workspace, keys),
    }
}

fn read_probe(
    store: &mut CandidateExecutionStore,
    workspace: &WorkspaceId,
    session: &SessionId,
    db: &Path,
    keys: Vec<String>,
) -> Probe {
    let (state, _, sequence) = store
        .current_snapshot(workspace, session, definition)
        .unwrap();
    let cursor = Cursor::from_state(&state);
    let observation = observe_successor(store, workspace, session)
        .map(|view| {
            assert_eq!(view.workspace_sequence, sequence);
            serde_json::from_str(view.projection.as_str()).unwrap()
        })
        .map_err(Refusal::from);
    let groups = state.group_history().iter().map(|group| json!({
        "id": group.identity().execution(), "definition": group.identity().definition_digest(),
        "predecessor": group.identity().predecessor(), "terminal": group.terminal_attempt(),
        "outcome": group.outcome(), "closure": group.closure().map(|closure| json!({
            "cause": closure.cause, "source": closure.source, "feedback": closure.feedback.as_ref().map(|feedback| json!({
                "message":feedback.feedback(), "basis":feedback.working_copy_basis().digest(),
            })),
        })),
        "prior_work": group.identity().prior_work().iter().map(|prior| json!({"group":prior.group(), "item":prior.item()})).collect::<Vec<_>>(),
        "working_copy_basis": group.identity().working_copy_basis().digest(),
    })).collect();
    let attempts = state.attempt_history().iter().map(|attempt| json!({
        "target": attempt.target(), "lifecycle": format!("{:?}", attempt.lifecycle()),
        "ordinal": attempt.decision_entry(), "decision": attempt.decision().map(|decision| json!({
            "option": decision.option, "guards": decision.guard_values.iter().map(|guard| json!({
                "source": guard.source, "item": guard.item.item(), "revision": guard.item.revision(), "digest": guard.item.value_digest(),
            })).collect::<Vec<_>>(),
        })),
        "items": attempt.slots().iter().map(|slot| json!({
            "id":slot.item_id(), "revision":slot.revision(),
            "value":slot.value().map(|value| podway_core::workflow_v2::item_value_json_v2(value).unwrap()),
        })).collect::<Vec<_>>(),
    })).collect();
    let goal_history = state.goal_revision_history().iter().map(|revision| json!({
        "revision": revision.revision.get(), "source": revision.source, "successor": revision.successor,
        "predecessor": revision.predecessor, "cause": revision.closure.cause,
    })).collect();
    let outcomes = outcomes(store, workspace, keys);
    let (ledger, fingerprint) = audit(db);
    assert_eq!(ledger.0, sequence);
    Probe {
        cursor,
        now: state.updated_at(),
        plan: state.plan_binding().ok().as_ref().map(Plan::from_binding),
        groups,
        attempts,
        goal_history,
        goal_outcome: state
            .current_goal_outcome()
            .map(|outcome| outcome.as_str().into()),
        observation,
        ledger,
        fingerprint,
        working_copy_digest: digest(&fs::read(db.parent().unwrap().join("work.txt")).unwrap()),
        outcomes,
    }
}

fn audit(db: &Path) -> ((u64, u64, u64), Sha256Digest) {
    let mut connection = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let transaction = connection.transaction().unwrap();
    let ledger = transaction.query_row(
        "SELECT next_workspace_sequence, (SELECT count(*) FROM mutation_outcomes), (SELECT count(*) FROM operational_journal) FROM workspace_state WHERE singleton=1",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    let mut hash = Sha256::new();
    for sql in [
        "SELECT json_array(workspace_uuid,git_common_fingerprint,git_worktree_fingerprint,last_validated_root,next_workspace_sequence,created_at_ms,updated_at_ms) FROM workspace_state",
        "SELECT json_array(contract_manifest_digest,generation_id,recorded_at_ms) FROM writer_contract",
        "SELECT json_array(digest,kind,document) FROM successor_definitions ORDER BY digest",
        "SELECT json_array(session_id,base_digest,record_json,record_digest,archived) FROM successor_sessions ORDER BY session_id",
        "SELECT json_array(session_id,ordinal,execution_id,definition_digest,design_attempt_id,predecessor_id,record_json,record_digest) FROM successor_activations ORDER BY session_id,ordinal",
        "SELECT json_array(session_id,ordinal,attempt_id,execution_id,definition_digest,record_json,record_digest) FROM successor_attempts ORDER BY session_id,ordinal",
        "SELECT json_array(session_id,ordinal,source_attempt_id,successor_attempt_id,record_json,record_digest) FROM successor_goal_revisions ORDER BY session_id,ordinal",
        "SELECT json_array(idempotency_key,request_digest,command_name,workspace_sequence,scope_kind,scope_session_id,response_json,recorded_at_ms) FROM mutation_outcomes ORDER BY workspace_sequence",
        "SELECT json_array(journal_id,recorded_at_ms,level,event_name,workspace_sequence,summary,details_json) FROM operational_journal ORDER BY journal_id",
    ] {
        hash.update(sql.as_bytes());
        let mut statement = transaction.prepare(sql).unwrap();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let value: String = row.get(0).unwrap();
            hash.update(value.len().to_le_bytes());
            hash.update(value.as_bytes());
        }
    }
    (
        ledger,
        Sha256Digest::new(format!("sha256:{:x}", hash.finalize())).unwrap(),
    )
}

fn write_json(path: &Path, value: &impl Serialize) {
    let stage = path.with_extension("pending");
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(&stage)
        .unwrap();
    serde_json::to_writer(&mut file, value).unwrap();
    drop(file);
    fs::rename(stage, path).unwrap();
}

fn read_json<T: DeserializeOwned>(path: &Path) -> T {
    let mut bytes = Vec::new();
    File::open(path)
        .unwrap()
        .take(MAX_MESSAGE + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(
        bytes.len() as u64 <= MAX_MESSAGE,
        "child fixture message exceeded its bound"
    );
    serde_json::from_slice(&bytes).unwrap()
}

fn wait_gate(path: &Path) {
    let end = Instant::now() + WAIT;
    while !path.is_file() {
        assert!(
            Instant::now() < end,
            "parent gate expired: {}",
            path.display()
        );
        std::thread::sleep(TICK);
    }
}

#[test]
fn engine_child() {
    let Some(input) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let input = PathBuf::from(input);
    assert_eq!(input.file_name().unwrap(), "request.json");
    assert_eq!(input.canonicalize().unwrap(), input);
    let child_dir = input.parent().unwrap();
    let fixture = child_dir.parent().unwrap();
    assert_eq!(
        fixture.parent().unwrap(),
        Path::new("/tmp").canonicalize().unwrap()
    );
    assert!(
        fixture
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with(PREFIX)
    );
    assert!(
        child_dir
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("child-")
    );
    assert!(fixture.join(".git").is_dir());
    let Input {
        workspace,
        session,
        mode,
        operation,
    } = read_json(&input);
    let root = ValidatedWorkspaceRootV1::from_path(fixture).unwrap();
    let db = fixture.join(STORE);
    let mut store = CandidateExecutionStore::reopen_fixture(&db, root, workspace.clone()).unwrap();
    if mode == Mode::Contender {
        let busy = child_dir.join("busy.json");
        let mut reported = false;
        let expires = Instant::now() + WAIT;
        store.observe_writer_busy(move |beginning, attempts| {
            if beginning && !reported {
                write_json(&busy, &json!({"pid":std::process::id(),"immediate_begin":true,"attempts":attempts}));
                reported = true;
            }
            std::thread::sleep(TICK);
            Instant::now() < expires
        }).unwrap();
    }
    let expected = match mode {
        Mode::ParkBeforeCommit | Mode::ExitBeforeCommit => {
            Some(CandidateCommitBoundary::AfterLedgerBeforeCommit)
        }
        Mode::ParkAfterCommit => Some(CandidateCommitBoundary::AfterCommitBeforeReturn),
        _ => None,
    };
    if let Some(expected) = expected {
        let child_dir = child_dir.to_owned();
        store.observe_commit(move |boundary, outcome| {
            if boundary != expected {
                return;
            }
            write_json(
                &child_dir.join("boundary.json"),
                &Passage {
                    pid: std::process::id(),
                    phase: format!("{boundary:?}"),
                    sequence: outcome.sequence(),
                    response_digest: digest(outcome.response().as_str().as_bytes()),
                },
            );
            wait_gate(&child_dir.join("release"));
            if mode == Mode::ExitBeforeCommit {
                std::process::exit(CONTROLLED_EXIT);
            }
        });
    }
    write_json(&child_dir.join("ready.json"), &std::process::id());
    wait_gate(&child_dir.join("start"));
    let reply = match operation {
        Operation::Read(keys) => Reply::Read(Box::new(read_probe(
            &mut store, &workspace, &session, &db, keys,
        ))),
        Operation::Inspect(keys) => {
            Reply::Inspection(Box::new(inspect(&store, &workspace, &db, keys)))
        }
        Operation::History(request) => history(&mut store, *request),
        Operation::Lifecycle(request) => lifecycle(&mut store, &workspace, *request),
        Operation::Grow {
            document,
            digest,
            count,
        } => match grow(
            &mut store, &workspace, &session, &db, &document, digest, count,
        ) {
            Ok(()) => Reply::Lifecycle,
            Err(error) => Reply::Rejected(error.into()),
        },
        Operation::Mutation(call) => match execute(&mut store, workspace, &session, *call) {
            Ok(result) => Reply::Success {
                replayed: result.replayed(),
                outcome: Outcome::new(result.outcome()),
            },
            Err(error) => Reply::Rejected(error.into()),
        },
    };
    drop(store);
    write_json(
        &child_dir.join("reply.json"),
        &Delivery {
            pid: std::process::id(),
            reply,
        },
    );
}

pub(super) struct Fixture {
    owner: tempfile::TempDir,
    workspace: WorkspaceId,
    pub session: SessionId,
    next: usize,
    work: Vec<u8>,
}

impl Fixture {
    pub fn create(base: &Value, plan_ready: bool) -> Self {
        let owner = tempfile::Builder::new()
            .prefix(PREFIX)
            .tempdir_in(Path::new("/tmp").canonicalize().unwrap())
            .unwrap();
        let root = owner.path();
        let git = Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main", "--template="])
            .arg(root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            git.status.success(),
            "isolated Git fixture: {}",
            String::from_utf8_lossy(&git.stderr)
        );
        let work = b"Work retained outside Podway\n".to_vec();
        fs::write(root.join("work.txt"), &work).unwrap();
        let workspace = NativeDirectIdSourceV1.next_workspace_id();
        let (state, sources) = initial_state(base, plan_ready);
        let db = root.join(STORE);
        drop(
            CandidateExecutionStore::create_fixture(
                &db,
                ValidatedWorkspaceRootV1::from_path(root).unwrap(),
                workspace.clone(),
                &state,
                &sources,
            )
            .unwrap(),
        );
        let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let schema: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let writer: (String, String) = conn
            .query_row(
                "SELECT generation_id,contract_manifest_digest FROM writer_contract",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(schema, 16);
        assert_eq!(writer.0, "execution-groups-candidate");
        assert_eq!(
            writer.1,
            digest(include_bytes!(
                "../../../assets/specifications/sqlite-v16.sql"
            ))
            .as_str()
        );
        let (executable, binary) = executable();
        println!(
            "GROUP-005 fixture {} executable={} binary={} sqlite={} writer={} ddl={}",
            root.display(),
            executable.display(),
            binary,
            schema,
            writer.0,
            writer.1
        );
        Self {
            owner,
            workspace,
            session: state.fence().session,
            next: 0,
            work,
        }
    }

    pub fn root(&self) -> &Path {
        self.owner.path()
    }

    pub fn change_work(&mut self, work: &[u8]) {
        fs::write(self.root().join("work.txt"), work).unwrap();
        self.work = work.to_vec();
    }

    fn child(&mut self, operation: Operation, mode: Mode) -> OwnedChild {
        self.next += 1;
        let dir = self.root().join(format!("child-{:03}", self.next));
        fs::create_dir(&dir).unwrap();
        let input = dir.join("request.json");
        let key = match &operation {
            Operation::Read(_) => "read".into(),
            Operation::Inspect(_) => "inspect".into(),
            Operation::History(_) => "history".into(),
            Operation::Lifecycle(_) => "fixture-lifecycle".into(),
            Operation::Grow { .. } => "grow-activations".into(),
            Operation::Mutation(call) => call.key.clone(),
        };
        write_json(
            &input,
            &Input {
                workspace: self.workspace.clone(),
                session: self.session.clone(),
                mode,
                operation,
            },
        );
        let (executable, _) = executable();
        let child_log = File::create(dir.join("child.log")).unwrap();
        let child = Command::new(executable)
            .args(["--exact", CHILD_TEST, "--nocapture"])
            .env_clear()
            .env(CHILD_ENV, input)
            .current_dir(self.root())
            .stdin(Stdio::null())
            .stdout(child_log.try_clone().unwrap())
            .stderr(child_log)
            .spawn()
            .unwrap();
        let mut child = OwnedChild {
            child: Some(child),
            dir,
            key,
            pid: 0,
        };
        child.pid = child.child.as_ref().unwrap().id();
        assert_ne!(child.pid, std::process::id());
        let ready: u32 = child.message("ready.json");
        assert_eq!(ready, child.pid);
        child
    }

    pub fn launch(&mut self, call: &Call, mode: Mode) -> OwnedChild {
        self.child(Operation::Mutation(Box::new(call.clone())), mode)
    }

    pub fn run(&mut self, call: &Call) -> Reply {
        let child = self.launch(call, Mode::Normal);
        child.start();
        child.finish()
    }

    pub fn read(&mut self, keys: &[&str]) -> Probe {
        let Reply::Read(probe) = self.request(Operation::Read(
            keys.iter().map(|key| (*key).into()).collect(),
        )) else {
            panic!("child did not read fixture")
        };
        *probe
    }

    fn request(&mut self, operation: Operation) -> Reply {
        let child = self.child(operation, Mode::Normal);
        child.start();
        child.finish()
    }

    pub fn target(&self, location: Location, revision: Revision) -> Target {
        Target {
            workspace: self.workspace.clone(),
            session: self.session.clone(),
            location,
            revision,
        }
    }

    pub fn history(&mut self, request: HistoryRequest) -> Reply {
        self.request(Operation::History(Box::new(request)))
    }

    pub fn inspect(&mut self, keys: &[&str]) -> Inspection {
        let Reply::Inspection(inspection) = self.request(Operation::Inspect(
            keys.iter().map(|key| (*key).into()).collect(),
        )) else {
            panic!("child did not inspect fixture")
        };
        *inspection
    }

    pub fn lifecycle(&mut self, request: Lifecycle) -> Reply {
        let reply = self.request(Operation::Lifecycle(Box::new(request)));
        if let Reply::Installed { workspace, session } = &reply {
            self.workspace = workspace.clone();
            self.session = session.clone();
        }
        reply
    }

    pub fn grow(&mut self, document: &Value, digest: &Sha256Digest, count: u8) {
        let result = self.request(Operation::Grow {
            document: Box::new(document.clone()),
            digest: digest.clone(),
            count,
        });
        assert!(
            matches!(result, Reply::Lifecycle),
            "activation growth refused: {result:?}"
        );
    }

    pub fn persistent_bytes(&self) -> (Vec<u8>, Option<Vec<u8>>) {
        let db = fs::read(self.root().join(STORE)).unwrap();
        let wal = match fs::read(self.root().join(format!("{STORE}-wal"))) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => panic!("could not read fixture WAL: {error}"),
        };
        (db, wal)
    }

    pub fn finish(self) {
        let path = self.root().to_owned();
        assert_eq!(fs::read(path.join("work.txt")).unwrap(), self.work);
        let children = self.next;
        self.owner.close().unwrap();
        assert!(!path.exists());
        println!(
            "GROUP-005 cleanup {} children={} fixture_exists=false",
            path.display(),
            children
        );
    }
}

fn executable() -> &'static (PathBuf, Sha256Digest) {
    static BINARY: OnceLock<(PathBuf, Sha256Digest)> = OnceLock::new();
    BINARY.get_or_init(|| {
        let path = std::env::current_exe().unwrap().canonicalize().unwrap();
        let mut hash = Sha256::new();
        io::copy(&mut File::open(&path).unwrap(), &mut hash).unwrap();
        (
            path,
            Sha256Digest::new(format!("sha256:{:x}", hash.finalize())).unwrap(),
        )
    })
}

pub(super) struct OwnedChild {
    child: Option<Child>,
    dir: PathBuf,
    key: String,
    pid: u32,
}

impl OwnedChild {
    fn log(&self) -> String {
        let mut bytes = Vec::new();
        File::open(self.dir.join("child.log"))
            .unwrap()
            .take(4096)
            .read_to_end(&mut bytes)
            .unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn message<T: DeserializeOwned>(&mut self, name: &str) -> T {
        let path = self.dir.join(name);
        let end = Instant::now() + WAIT;
        while !path.is_file() {
            if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                assert!(
                    path.is_file(),
                    "child {} exited {status} before {name}: {}",
                    self.pid,
                    self.log()
                );
                break;
            }
            assert!(
                Instant::now() < end,
                "child {} timed out waiting for {name}: {}",
                self.pid,
                self.log()
            );
            std::thread::sleep(TICK);
        }
        read_json(&path)
    }

    pub fn start(&self) {
        File::options()
            .write(true)
            .create_new(true)
            .open(self.dir.join("start"))
            .unwrap();
    }
    pub fn release(&self) {
        File::options()
            .write(true)
            .create_new(true)
            .open(self.dir.join("release"))
            .unwrap();
    }

    pub fn busy(&mut self) {
        let mark: Value = self.message("busy.json");
        assert_eq!(
            mark,
            json!({"pid":self.pid,"immediate_begin":true,"attempts":0})
        );
        assert!(!self.dir.join("reply.json").exists());
        println!(
            "GROUP-005 busy key={} pid={} boundary=immediate_begin",
            self.key, self.pid
        );
    }

    pub fn boundary(&mut self, expected: &str) -> Passage {
        let mark: Passage = self.message("boundary.json");
        assert_eq!(mark.pid, self.pid);
        assert_eq!(mark.phase, expected);
        assert!(!self.dir.join("reply.json").exists());
        println!(
            "GROUP-005 parked key={} pid={} phase={} sequence={} reply_digest={}",
            self.key, self.pid, mark.phase, mark.sequence, mark.response_digest
        );
        mark
    }

    fn wait(&mut self) -> ExitStatus {
        let end = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                self.child.take();
                return status;
            }
            assert!(
                Instant::now() < end,
                "child {} failed to exit: {}",
                self.pid,
                self.log()
            );
            std::thread::sleep(TICK);
        }
    }

    pub fn kill(mut self) {
        assert!(self.child.as_mut().unwrap().try_wait().unwrap().is_none());
        self.child.as_mut().unwrap().kill().unwrap();
        assert_eq!(self.wait().signal(), Some(nix::libc::SIGKILL));
        assert!(!self.dir.join("reply.json").exists());
        println!(
            "GROUP-005 reaped key={} pid={} exit=SIGKILL reply=false",
            self.key, self.pid
        );
    }

    pub fn controlled_exit(mut self) {
        self.release();
        assert_eq!(self.wait().code(), Some(CONTROLLED_EXIT));
        assert!(!self.dir.join("reply.json").exists());
        println!(
            "GROUP-005 reaped key={} pid={} exit={} reply=false",
            self.key, self.pid, CONTROLLED_EXIT
        );
    }

    pub fn finish(mut self) -> Reply {
        let status = self.wait();
        assert!(
            status.success(),
            "child {} failed: {}",
            self.pid,
            self.log()
        );
        let delivery: Delivery = read_json(&self.dir.join("reply.json"));
        assert_eq!(delivery.pid, self.pid);
        match &delivery.reply {
            Reply::Success { replayed, outcome } => println!(
                "GROUP-005 reaped key={} pid={} sequence={} replayed={} reply_digest={}",
                self.key,
                self.pid,
                outcome.sequence,
                replayed,
                digest(outcome.response.as_bytes())
            ),
            Reply::Rejected(error) => println!(
                "GROUP-005 reaped key={} pid={} rejected={error:?}",
                self.key, self.pid
            ),
            Reply::Read(probe) => println!(
                "GROUP-005 reaped read pid={} ledger={:?} fingerprint={}",
                self.pid, probe.ledger, probe.fingerprint
            ),
            Reply::Inspection(inspection) => println!(
                "TRACE-004 reaped inspect pid={} ledger={:?} rows={:?} fingerprint={}",
                self.pid, inspection.ledger, inspection.retained, inspection.fingerprint
            ),
            Reply::History(response) => println!(
                "TRACE-004 reaped history pid={} response_digest={}",
                self.pid,
                digest(response.as_bytes())
            ),
            Reply::Lifecycle | Reply::Installed { .. } => println!(
                "TRACE-004 reaped key={} pid={} fixture_write=true",
                self.key, self.pid
            ),
        }
        delivery.reply
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(super) fn response_digest(response: &str) -> Sha256Digest {
    digest(response.as_bytes())
}
