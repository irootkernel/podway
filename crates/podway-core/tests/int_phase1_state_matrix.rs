use std::collections::BTreeSet;

use podway_core::{
    AddItemV1, ArtifactValueV1, AttemptId, AttemptLifecycle, BlockSessionV1, BlockerId,
    BlockerState, CheckItemV1, ClearItemV1, CommandContextV1, CompleteSessionV1, DomainError,
    ItemCommonV1, ItemId, ItemMutationPreconditionsV1, ItemSpecV1, ItemValueV1,
    ProcedureSnapshotAssemblyInputV1, ProcedureSnapshotId, ProcedureSnapshotV1,
    ProcedureSourceLabelV1, ProcedureWarningCodeV1, ReopenSessionV1, ResetAllWorkspaceV1,
    ResetSessionV1, RetrySessionV1, ReturnPolicyV1, ReturnSessionV1, Revision, SessionAggregateV1,
    SessionCommandV1, SessionId, SessionLifecycle, Sha256Digest, SkipPolicyV1, SkipSessionV1,
    StageId, StageProgressState, StageSpecV1, StartReplaceSessionV1, StartSessionV1,
    UnblockSessionV1, UnixMillis, apply_transition_v1, preview_transition_v1,
};

const MATRIX: &str = include_str!("../../../spec/state-transition-matrix.csv");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MatrixClassification {
    ExternalGitStoreBoundary,
    PureDomain(ConformanceCase),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConformanceCase {
    SessionStart,
    SessionStartReplace,
    ItemCheck,
    ItemUncheck,
    ItemSet,
    ItemAdd,
    ItemRemove,
    ItemAttach,
    ItemClear,
    SessionComplete,
    SessionSkip,
    SessionRetry,
    SessionReturn,
    SessionBlock,
    SessionUnblock,
    SessionCancel,
    SessionReopen,
    SessionReset,
    WorkspaceResetAll,
}

#[derive(Debug, Eq, PartialEq)]
struct MatrixRow {
    command: String,
    cli_form: String,
    allowed_session_state: String,
    required_preconditions: String,
    session_revision_change: String,
    attempt_effect: String,
    stage_or_workspace_effect: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MatrixExpectation {
    command: &'static str,
    cli_form: &'static str,
    allowed_session_state: &'static str,
    required_preconditions: &'static str,
    session_revision_change: &'static str,
    attempt_effect: &'static str,
    stage_or_workspace_effect: &'static str,
    classification: MatrixClassification,
}

const MATRIX_EXPECTATIONS: [MatrixExpectation; 20] = [
    MatrixExpectation {
        command: "workspace.init",
        cli_form: "podway init",
        allowed_session_state: "workspace absent or repairable",
        required_preconditions: "valid Git worktree and safe .podway layout",
        session_revision_change: "n/a",
        attempt_effect: "none",
        stage_or_workspace_effect: "initialize workspace database and identity",
        classification: MatrixClassification::ExternalGitStoreBoundary,
    },
    MatrixExpectation {
        command: "session.start",
        cli_form: "podway start",
        allowed_session_state: "no session",
        required_preconditions: "valid procedure and nonempty task title",
        session_revision_change: "create revision 1",
        attempt_effect: "create first active attempt",
        stage_or_workspace_effect: "first stage current and later stages pending",
        classification: MatrixClassification::PureDomain(ConformanceCase::SessionStart),
    },
    MatrixExpectation {
        command: "session.start_replace",
        cli_form: "podway start --replace",
        allowed_session_state: "any existing session",
        required_preconditions: "confirmation plus existing session identity and revision",
        session_revision_change: "replace with revision 1",
        attempt_effect: "delete old session then create first active attempt",
        stage_or_workspace_effect: "exclusive barrier replaces current task",
        classification: MatrixClassification::PureDomain(ConformanceCase::SessionStartReplace),
    },
    MatrixExpectation {
        command: "item.check",
        cli_form: "podway check",
        allowed_session_state: "running",
        required_preconditions: "active attempt and item revision",
        session_revision_change: "+1 when changed",
        attempt_effect: "set confirm true",
        stage_or_workspace_effect: "current stage unchanged",
        classification: MatrixClassification::PureDomain(ConformanceCase::ItemCheck),
    },
    MatrixExpectation {
        command: "item.uncheck",
        cli_form: "podway uncheck",
        allowed_session_state: "running",
        required_preconditions: "active attempt and item revision",
        session_revision_change: "+1 when changed",
        attempt_effect: "clear confirm value",
        stage_or_workspace_effect: "current stage unchanged",
        classification: MatrixClassification::PureDomain(ConformanceCase::ItemUncheck),
    },
    MatrixExpectation {
        command: "item.set",
        cli_form: "podway set",
        allowed_session_state: "running",
        required_preconditions: "active attempt item revision and type-valid value",
        session_revision_change: "+1 when changed",
        attempt_effect: "set scalar item value",
        stage_or_workspace_effect: "current stage unchanged",
        classification: MatrixClassification::PureDomain(ConformanceCase::ItemSet),
    },
    MatrixExpectation {
        command: "item.add",
        cli_form: "podway add",
        allowed_session_state: "running",
        required_preconditions: "active attempt item revision and list constraints",
        session_revision_change: "+1 when changed",
        attempt_effect: "append list value",
        stage_or_workspace_effect: "current stage unchanged",
        classification: MatrixClassification::PureDomain(ConformanceCase::ItemAdd),
    },
    MatrixExpectation {
        command: "item.remove",
        cli_form: "podway remove",
        allowed_session_state: "running",
        required_preconditions: "active attempt item revision and existing value unless ignored",
        session_revision_change: "+1 when changed",
        attempt_effect: "remove list value",
        stage_or_workspace_effect: "current stage unchanged",
        classification: MatrixClassification::PureDomain(ConformanceCase::ItemRemove),
    },
    MatrixExpectation {
        command: "item.attach",
        cli_form: "podway attach",
        allowed_session_state: "running",
        required_preconditions: "active attempt item revision and valid artifact metadata",
        session_revision_change: "+1 when changed",
        attempt_effect: "set artifact metadata",
        stage_or_workspace_effect: "current stage unchanged",
        classification: MatrixClassification::PureDomain(ConformanceCase::ItemAttach),
    },
    MatrixExpectation {
        command: "item.clear",
        cli_form: "podway clear",
        allowed_session_state: "running",
        required_preconditions: "active attempt and item revision",
        session_revision_change: "+1 when changed",
        attempt_effect: "clear item value",
        stage_or_workspace_effect: "current stage unchanged",
        classification: MatrixClassification::PureDomain(ConformanceCase::ItemClear),
    },
    MatrixExpectation {
        command: "session.complete",
        cli_form: "podway complete",
        allowed_session_state: "running",
        required_preconditions: "session revision active attempt all required items and no blockers",
        session_revision_change: "+1",
        attempt_effect: "complete current and create next unless final",
        stage_or_workspace_effect: "current done and next current or session completed",
        classification: MatrixClassification::PureDomain(ConformanceCase::SessionComplete),
    },
    MatrixExpectation {
        command: "session.skip",
        cli_form: "podway skip",
        allowed_session_state: "running",
        required_preconditions: "session revision active attempt skip policy and reason",
        session_revision_change: "+1",
        attempt_effect: "skip current and create next",
        stage_or_workspace_effect: "current skipped and next current or session completed",
        classification: MatrixClassification::PureDomain(ConformanceCase::SessionSkip),
    },
    MatrixExpectation {
        command: "session.retry",
        cli_form: "podway retry",
        allowed_session_state: "running",
        required_preconditions: "session revision active attempt and reason",
        session_revision_change: "+1",
        attempt_effect: "abandon current and create same-stage next attempt",
        stage_or_workspace_effect: "current stage remains current",
        classification: MatrixClassification::PureDomain(ConformanceCase::SessionRetry),
    },
    MatrixExpectation {
        command: "session.return",
        cli_form: "podway return",
        allowed_session_state: "running",
        required_preconditions: "session revision active attempt allowed earlier stage and reason",
        session_revision_change: "+1",
        attempt_effect: "abandon current and create destination attempt",
        stage_or_workspace_effect: "destination current and reached downstream redo",
        classification: MatrixClassification::PureDomain(ConformanceCase::SessionReturn),
    },
    MatrixExpectation {
        command: "session.block",
        cli_form: "podway block",
        allowed_session_state: "running",
        required_preconditions: "session revision active attempt and reason",
        session_revision_change: "+1",
        attempt_effect: "add open blocker",
        stage_or_workspace_effect: "current stage derives blocked",
        classification: MatrixClassification::PureDomain(ConformanceCase::SessionBlock),
    },
    MatrixExpectation {
        command: "session.unblock",
        cli_form: "podway unblock",
        allowed_session_state: "running",
        required_preconditions: "session revision active attempt and current blocker",
        session_revision_change: "+1",
        attempt_effect: "resolve one or all current blockers",
        stage_or_workspace_effect: "current or blocked derived state updates",
        classification: MatrixClassification::PureDomain(ConformanceCase::SessionUnblock),
    },
    MatrixExpectation {
        command: "session.cancel",
        cli_form: "podway cancel",
        allowed_session_state: "running",
        required_preconditions: "session revision active attempt and reason",
        session_revision_change: "+1",
        attempt_effect: "abandon active",
        stage_or_workspace_effect: "current stage abandoned and session cancelled",
        classification: MatrixClassification::PureDomain(ConformanceCase::SessionCancel),
    },
    MatrixExpectation {
        command: "session.reopen",
        cli_form: "podway reopen",
        allowed_session_state: "completed",
        required_preconditions: "session revision allowed destination and reason",
        session_revision_change: "+1",
        attempt_effect: "create destination active attempt",
        stage_or_workspace_effect: "destination current and later reached stages redo",
        classification: MatrixClassification::PureDomain(ConformanceCase::SessionReopen),
    },
    MatrixExpectation {
        command: "session.reset",
        cli_form: "podway reset",
        allowed_session_state: "any existing session",
        required_preconditions: "confirmation session identity and revision",
        session_revision_change: "n/a because session deleted",
        attempt_effect: "delete all session attempts and values",
        stage_or_workspace_effect: "preserve workspace initialization",
        classification: MatrixClassification::PureDomain(ConformanceCase::SessionReset),
    },
    MatrixExpectation {
        command: "workspace.reset_all",
        cli_form: "podway reset --all",
        allowed_session_state: "any or unreadable workspace state",
        required_preconditions: "force confirmation and workspace identity when readable",
        session_revision_change: "n/a because database recreated",
        attempt_effect: "delete all task state",
        stage_or_workspace_effect: "exclusive filesystem-marker protocol recreates workspace runtime",
        classification: MatrixClassification::PureDomain(ConformanceCase::WorkspaceResetAll),
    },
];

fn parse_matrix() -> Vec<MatrixRow> {
    let mut lines = MATRIX.lines();
    assert_eq!(
        lines.next(),
        Some(
            "command,cli_form,allowed_session_state,required_preconditions,session_revision_change,attempt_effect,stage_or_workspace_effect"
        )
    );

    lines
        .enumerate()
        .map(|(index, line)| {
            let columns = line.split(',').collect::<Vec<_>>();
            assert_eq!(
                columns.len(),
                7,
                "matrix row {} has seven columns",
                index + 2
            );
            assert!(
                columns.iter().all(|column| !column.is_empty()),
                "matrix row {} has an empty semantic column",
                index + 2
            );
            MatrixRow {
                command: columns[0].to_owned(),
                cli_form: columns[1].to_owned(),
                allowed_session_state: columns[2].to_owned(),
                required_preconditions: columns[3].to_owned(),
                session_revision_change: columns[4].to_owned(),
                attempt_effect: columns[5].to_owned(),
                stage_or_workspace_effect: columns[6].to_owned(),
            }
        })
        .collect()
}

fn assert_matrix_row(row: &MatrixRow, expected: MatrixExpectation) {
    assert_eq!(row.command, expected.command);
    assert_eq!(row.cli_form, expected.cli_form);
    assert_eq!(row.allowed_session_state, expected.allowed_session_state);
    assert_eq!(row.required_preconditions, expected.required_preconditions);
    assert_eq!(
        row.session_revision_change,
        expected.session_revision_change
    );
    assert_eq!(row.attempt_effect, expected.attempt_effect);
    assert_eq!(
        row.stage_or_workspace_effect,
        expected.stage_or_workspace_effect
    );
}

impl ConformanceCase {
    const fn name(self) -> &'static str {
        match self {
            Self::SessionStart => "session.start",
            Self::SessionStartReplace => "session.start_replace",
            Self::ItemCheck => "item.check",
            Self::ItemUncheck => "item.uncheck",
            Self::ItemSet => "item.set",
            Self::ItemAdd => "item.add",
            Self::ItemRemove => "item.remove",
            Self::ItemAttach => "item.attach",
            Self::ItemClear => "item.clear",
            Self::SessionComplete => "session.complete",
            Self::SessionSkip => "session.skip",
            Self::SessionRetry => "session.retry",
            Self::SessionReturn => "session.return",
            Self::SessionBlock => "session.block",
            Self::SessionUnblock => "session.unblock",
            Self::SessionCancel => "session.cancel",
            Self::SessionReopen => "session.reopen",
            Self::SessionReset => "session.reset",
            Self::WorkspaceResetAll => "workspace.reset_all",
        }
    }

    fn exercise(self) {
        match self {
            Self::SessionStart => case_start(),
            Self::SessionStartReplace => case_start_replace(),
            Self::ItemCheck => case_check(),
            Self::ItemUncheck => case_uncheck(),
            Self::ItemSet => case_set(),
            Self::ItemAdd => case_add(),
            Self::ItemRemove => case_remove(),
            Self::ItemAttach => case_attach(),
            Self::ItemClear => case_clear(),
            Self::SessionComplete => case_complete(),
            Self::SessionSkip => case_skip(),
            Self::SessionRetry => case_retry(),
            Self::SessionReturn => case_return(),
            Self::SessionBlock => case_block(),
            Self::SessionUnblock => case_unblock(),
            Self::SessionCancel => case_cancel(),
            Self::SessionReopen => case_reopen(),
            Self::SessionReset => case_reset(),
            Self::WorkspaceResetAll => case_reset_all(),
        }
    }
}

fn uuid(value: u32) -> String {
    format!("123e4567-e89b-12d3-a456-{value:012x}")
}

fn attempt_id(value: u32) -> AttemptId {
    AttemptId::new(uuid(value)).unwrap()
}

fn blocker_id(value: u32) -> BlockerId {
    BlockerId::new(uuid(value)).unwrap()
}

fn session_id(value: u32) -> SessionId {
    SessionId::new(uuid(value)).unwrap()
}

fn item_id(value: &str) -> ItemId {
    ItemId::new(value).unwrap()
}

fn stage_id(value: &str) -> StageId {
    StageId::new(value).unwrap()
}

fn digest() -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap()
}

fn common(id: &str) -> ItemCommonV1 {
    ItemCommonV1::new(item_id(id), format!("Prompt for {id}"), None, false).unwrap()
}

fn stage(id: &str, items: Vec<ItemSpecV1>, skip_policy: SkipPolicyV1) -> StageSpecV1 {
    StageSpecV1::new(stage_id(id), id, Vec::new(), items, skip_policy).unwrap()
}

fn fixture_snapshot_with(
    skip_policies: [SkipPolicyV1; 3],
    return_policy: ReturnPolicyV1,
    accepted_warning_codes: Vec<ProcedureWarningCodeV1>,
) -> Result<ProcedureSnapshotV1, DomainError> {
    ProcedureSnapshotV1::assemble(ProcedureSnapshotAssemblyInputV1 {
        snapshot_id: ProcedureSnapshotId::new(uuid(1)).unwrap(),
        procedure_id: "state-matrix".to_owned(),
        procedure_version: "1".to_owned(),
        name: "State matrix".to_owned(),
        description: None,
        stages: vec![
            stage(
                "one",
                vec![
                    ItemSpecV1::confirm(common("confirm")),
                    ItemSpecV1::text(common("text"), 0, 32, true).unwrap(),
                    ItemSpecV1::list(common("list"), 0, 4, 20, false).unwrap(),
                    ItemSpecV1::artifact(common("artifact"), vec!["text/plain".to_owned()])
                        .unwrap(),
                ],
                skip_policies[0],
            ),
            stage("two", Vec::new(), skip_policies[1]),
            stage("three", Vec::new(), skip_policies[2]),
        ],
        return_policy,
        source_label: ProcedureSourceLabelV1::new("test").unwrap(),
        accepted_warning_codes,
        created_at: UnixMillis::new(1),
    })
}

fn fixture_snapshot() -> ProcedureSnapshotV1 {
    fixture_snapshot_with(
        [SkipPolicyV1::allowed(true); 3],
        ReturnPolicyV1::any_previous(),
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::FinalStageSkippable,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    )
    .unwrap()
}

#[test]
fn fixture_snapshot_rejects_missing_warning_admission_proof() {
    assert!(
        fixture_snapshot_with(
            [SkipPolicyV1::allowed(true); 3],
            ReturnPolicyV1::any_previous(),
            Vec::new(),
        )
        .is_err()
    );
}

fn start_input_with(
    snapshot: ProcedureSnapshotV1,
    session_number: u32,
    attempt_number: u32,
) -> StartSessionV1 {
    StartSessionV1 {
        task_title: "Task".to_owned(),
        snapshot,
        session_id: session_id(session_number),
        first_attempt_id: attempt_id(attempt_number),
    }
}

fn start_input(session_number: u32, attempt_number: u32) -> StartSessionV1 {
    start_input_with(fixture_snapshot(), session_number, attempt_number)
}
fn context(session: &SessionAggregateV1, now: u64) -> CommandContextV1 {
    CommandContextV1 {
        expected_revision: session.revision(),
        now: UnixMillis::new(now),
    }
}

fn apply_equivalent(
    prior: Option<&SessionAggregateV1>,
    command: &SessionCommandV1,
    context: CommandContextV1,
) -> podway_core::TransitionOutcomeV1 {
    let preview = preview_transition_v1(prior, command, context);
    let applied = apply_transition_v1(prior, command, context);
    assert_eq!(preview, applied);
    applied.unwrap()
}

fn started_with(
    snapshot: ProcedureSnapshotV1,
    session_number: u32,
    attempt_number: u32,
) -> SessionAggregateV1 {
    let outcome = apply_equivalent(
        None,
        &SessionCommandV1::Start(start_input_with(snapshot, session_number, attempt_number)),
        CommandContextV1 {
            expected_revision: Revision::ZERO,
            now: UnixMillis::new(10),
        },
    );
    let session = outcome.next_aggregate().unwrap().clone();
    assert_eq!(session.revision(), Revision::new(1));
    assert_eq!(session.latest_recorded_at(), UnixMillis::new(10));
    assert_cursor_and_frontier(&session);
    session
}

fn started() -> SessionAggregateV1 {
    started_with(fixture_snapshot(), 2, 3)
}

fn apply_next(
    session: &SessionAggregateV1,
    command: SessionCommandV1,
    now: u64,
) -> (podway_core::TransitionOutcomeV1, SessionAggregateV1) {
    let outcome = apply_equivalent(Some(session), &command, context(session, now));
    let next = outcome.next_aggregate().unwrap().clone();
    assert!(outcome.changed());
    assert_eq!(outcome.revision_before(), Some(session.revision()));
    assert_eq!(outcome.revision_after(), Some(next.revision()));
    let expected_revision = if matches!(&command, SessionCommandV1::StartReplace(_)) {
        Revision::new(1)
    } else {
        session.revision().checked_next().unwrap()
    };
    assert_eq!(next.revision(), expected_revision);
    assert_eq!(next.latest_recorded_at(), UnixMillis::new(now));
    assert_cursor_and_frontier(&next);
    if let Some((attempt_id, item_id)) = item_mutation_target(&command) {
        let prior_slot = slot_by_attempt_and_item(session, attempt_id, item_id);
        let next_slot = slot_by_attempt_and_item(&next, attempt_id, item_id);
        assert_eq!(
            next_slot.revision(),
            prior_slot.revision().checked_next().unwrap()
        );
    }
    (outcome, next)
}

fn attempt_by_id<'a>(
    session: &'a SessionAggregateV1,
    expected_attempt_id: &AttemptId,
) -> &'a podway_core::AttemptV1 {
    session
        .attempts()
        .iter()
        .find(|attempt| attempt.attempt_id() == expected_attempt_id)
        .unwrap()
}

fn attempt_by_stage_and_number<'a>(
    session: &'a SessionAggregateV1,
    expected_stage_id: &str,
    expected_number: u32,
) -> &'a podway_core::AttemptV1 {
    session
        .attempts()
        .iter()
        .find(|attempt| {
            attempt.stage_id() == &stage_id(expected_stage_id)
                && attempt.number() == expected_number
        })
        .unwrap()
}

fn blocker_by_id<'a>(
    attempt: &'a podway_core::AttemptV1,
    expected_blocker_id: &BlockerId,
) -> &'a podway_core::BlockerV1 {
    attempt
        .blockers()
        .iter()
        .find(|blocker| blocker.blocker_id() == expected_blocker_id)
        .unwrap()
}

fn slot_by_attempt_and_item<'a>(
    session: &'a SessionAggregateV1,
    expected_attempt_id: &AttemptId,
    expected_item_id: &ItemId,
) -> &'a podway_core::ItemSlotV1 {
    attempt_by_id(session, expected_attempt_id)
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == expected_item_id)
        .unwrap()
}

fn active_attempt(session: &SessionAggregateV1) -> &podway_core::AttemptV1 {
    attempt_by_id(session, session.active_attempt_id().unwrap())
}

fn item_preconditions(session: &SessionAggregateV1, item: &str) -> ItemMutationPreconditionsV1 {
    let attempt = active_attempt(session);
    let slot = attempt
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id(item))
        .unwrap();
    ItemMutationPreconditionsV1 {
        expected_attempt_id: attempt.attempt_id().clone(),
        expected_item_revision: slot.revision(),
    }
}

fn active_slot<'a>(session: &'a SessionAggregateV1, item: &str) -> &'a podway_core::ItemSlotV1 {
    active_attempt(session)
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id(item))
        .unwrap()
}
fn assert_affected_stages(outcome: &podway_core::TransitionOutcomeV1, expected: &[&str]) {
    assert_eq!(
        outcome
            .affected_stages()
            .iter()
            .map(StageId::as_str)
            .collect::<Vec<_>>(),
        expected
    );
}

fn assert_stage_states(session: &SessionAggregateV1, expected: &[StageProgressState]) {
    assert_eq!(
        session
            .stage_progress()
            .iter()
            .map(|progress| progress.state())
            .collect::<Vec<_>>(),
        expected
    );
    assert_cursor_and_frontier(session);
}

fn assert_cursor_and_frontier(session: &SessionAggregateV1) {
    let mut attempt_order = Vec::new();
    for attempt in session.attempts() {
        let stage_index = session
            .snapshot()
            .stages()
            .iter()
            .position(|stage| stage.id() == attempt.stage_id())
            .unwrap();
        attempt_order.push((stage_index, attempt.number()));
    }
    assert!(
        attempt_order.windows(2).all(|pair| pair[0] < pair[1]),
        "attempts must be ordered by stage index and attempt number"
    );

    for (stage_index, (stage, progress)) in session
        .snapshot()
        .stages()
        .iter()
        .zip(session.stage_progress())
        .enumerate()
    {
        assert_eq!(progress.stage_index(), stage_index);
        assert_eq!(progress.stage_id(), stage.id());
        let latest_attempt = session
            .attempts()
            .iter()
            .filter(|attempt| attempt.stage_id() == stage.id())
            .max_by_key(|attempt| attempt.number());
        match latest_attempt {
            Some(attempt) => {
                assert_eq!(progress.latest_attempt_number(), attempt.number());
                assert_eq!(progress.latest_attempt_id(), Some(attempt.attempt_id()));
            }
            None => {
                assert_eq!(progress.latest_attempt_number(), 0);
                assert_eq!(progress.latest_attempt_id(), None);
            }
        }
    }

    match session.lifecycle() {
        SessionLifecycle::Running => {
            let current = session
                .stage_progress()
                .iter()
                .filter(|progress| progress.state() == StageProgressState::Current)
                .collect::<Vec<_>>();
            assert_eq!(current.len(), 1);
            let active_attempt = active_attempt(session);
            assert_eq!(active_attempt.lifecycle(), AttemptLifecycle::Active);
            assert_eq!(session.active_stage_id(), Some(current[0].stage_id()));
            assert_eq!(
                session.active_attempt_id(),
                Some(active_attempt.attempt_id())
            );
            assert_eq!(active_attempt.stage_id(), current[0].stage_id());
            assert_eq!(
                current[0].latest_attempt_id(),
                Some(active_attempt.attempt_id())
            );
        }
        SessionLifecycle::Completed | SessionLifecycle::Cancelled => {
            assert_eq!(session.active_stage_id(), None);
            assert_eq!(session.active_attempt_id(), None);
        }
    }
}

struct ExpectedAttempt<'a> {
    attempt_id: &'a AttemptId,
    stage_id: &'a str,
    number: u32,
    lifecycle: AttemptLifecycle,
    started_at: u64,
    ended_at: Option<u64>,
    reason: Option<&'a str>,
}

fn assert_attempt(session: &SessionAggregateV1, expected: ExpectedAttempt<'_>) {
    let attempt = attempt_by_id(session, expected.attempt_id);
    assert_eq!(
        attempt,
        attempt_by_stage_and_number(session, expected.stage_id, expected.number)
    );
    assert_eq!(attempt.stage_id(), &stage_id(expected.stage_id));
    assert_eq!(attempt.number(), expected.number);
    assert_eq!(attempt.lifecycle(), expected.lifecycle);
    assert_eq!(attempt.started_at(), UnixMillis::new(expected.started_at));
    assert_eq!(attempt.ended_at(), expected.ended_at.map(UnixMillis::new));
    assert_eq!(attempt.reason(), expected.reason);
}

fn assert_stable_noop(prior: &SessionAggregateV1, outcome: &podway_core::TransitionOutcomeV1) {
    assert!(!outcome.changed());
    assert_eq!(outcome.revision_before(), Some(prior.revision()));
    assert_eq!(outcome.revision_after(), Some(prior.revision()));
    assert_eq!(outcome.next_aggregate(), Some(prior));
}

fn assert_exactly_one_active_attempt(session: &SessionAggregateV1) {
    assert_eq!(
        session
            .attempts()
            .iter()
            .filter(|attempt| attempt.lifecycle() == AttemptLifecycle::Active)
            .count(),
        1,
        "a running session must retain exactly one active attempt"
    );
}

fn assert_pristine_active_attempt(session: &SessionAggregateV1, expected_reason: Option<&str>) {
    assert_exactly_one_active_attempt(session);
    let attempt = active_attempt(session);
    assert_eq!(attempt.lifecycle(), AttemptLifecycle::Active);
    assert_eq!(attempt.reason(), expected_reason);
    assert!(attempt.blockers().is_empty());
    assert!(
        attempt
            .item_slots()
            .iter()
            .all(|slot| slot.value().is_none())
    );
    assert!(
        attempt
            .item_slots()
            .iter()
            .all(|slot| slot.revision() == Revision::ZERO)
    );
}

fn assert_fresh_active_attempt(session: &SessionAggregateV1) {
    assert_pristine_active_attempt(session, None);
}

fn assert_rejected_without_mutation(
    session: &SessionAggregateV1,
    command: SessionCommandV1,
    context: CommandContextV1,
) {
    let original_session = session.clone();
    let original_command = command.clone();
    assert_eq!(
        preview_transition_v1(Some(session), &command, context),
        apply_transition_v1(Some(session), &command, context)
    );
    assert!(apply_transition_v1(Some(session), &command, context).is_err());
    assert_eq!(command, original_command);
    assert_eq!(*session, original_session);
}
fn assert_rejected_without_mutation_with_error(
    session: &SessionAggregateV1,
    command: SessionCommandV1,
    context: CommandContextV1,
    expected: DomainError,
) {
    let original_session = session.clone();
    let original_command = command.clone();
    let preview = preview_transition_v1(Some(session), &command, context);
    let applied = apply_transition_v1(Some(session), &command, context);
    assert_eq!(preview, applied);
    assert_eq!(applied.unwrap_err(), expected);
    assert_eq!(command, original_command);
    assert_eq!(*session, original_session);
}

fn assert_start_rejected_without_mutation_with_error(
    command: SessionCommandV1,
    expected: DomainError,
) {
    let original_command = command.clone();
    let context = CommandContextV1 {
        expected_revision: Revision::ZERO,
        now: UnixMillis::new(10),
    };
    let preview = preview_transition_v1(None, &command, context);
    let applied = apply_transition_v1(None, &command, context);
    assert_eq!(preview, applied);
    assert_eq!(applied.unwrap_err(), expected);
    assert_eq!(command, original_command);
}

fn assert_blank_reason_rejected(session: &SessionAggregateV1, command: SessionCommandV1, now: u64) {
    assert_rejected_without_mutation_with_error(
        session,
        command,
        context(session, now),
        DomainError::InvalidState {
            reason: "reason must contain at most 4000 non-blank scalars",
        },
    );
}
fn item_mutation_target(command: &SessionCommandV1) -> Option<(&AttemptId, &ItemId)> {
    match command {
        SessionCommandV1::Check(input) => {
            Some((&input.preconditions.expected_attempt_id, &input.item_id))
        }
        SessionCommandV1::Uncheck(input) => {
            Some((&input.preconditions.expected_attempt_id, &input.item_id))
        }
        SessionCommandV1::Set(input) => {
            Some((&input.preconditions.expected_attempt_id, &input.item_id))
        }
        SessionCommandV1::Add(input) => {
            Some((&input.preconditions.expected_attempt_id, &input.item_id))
        }
        SessionCommandV1::Remove(input) => {
            Some((&input.preconditions.expected_attempt_id, &input.item_id))
        }
        SessionCommandV1::Attach(input) => {
            Some((&input.preconditions.expected_attempt_id, &input.item_id))
        }
        SessionCommandV1::Clear(input) => {
            Some((&input.preconditions.expected_attempt_id, &input.item_id))
        }
        _ => None,
    }
}
fn running_only_commands(expected_attempt_id: AttemptId) -> Vec<SessionCommandV1> {
    let item_preconditions = ItemMutationPreconditionsV1 {
        expected_attempt_id: expected_attempt_id.clone(),
        expected_item_revision: Revision::ZERO,
    };
    vec![
        SessionCommandV1::Check(CheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: item_preconditions.clone(),
        }),
        SessionCommandV1::Uncheck(podway_core::UncheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: item_preconditions.clone(),
        }),
        SessionCommandV1::Set(podway_core::SetItemV1 {
            item_id: item_id("text"),
            value: ItemValueV1::text("value"),
            preconditions: item_preconditions.clone(),
        }),
        SessionCommandV1::Add(AddItemV1 {
            item_id: item_id("list"),
            value: "entry".to_owned(),
            preconditions: item_preconditions.clone(),
        }),
        SessionCommandV1::Remove(podway_core::RemoveItemV1 {
            item_id: item_id("list"),
            value: "entry".to_owned(),
            ignore_missing: true,
            preconditions: item_preconditions.clone(),
        }),
        SessionCommandV1::Attach(podway_core::AttachItemV1 {
            item_id: item_id("artifact"),
            value: ArtifactValueV1::external_reference("artifact:1", digest(), 1, "text/plain")
                .unwrap(),
            preconditions: item_preconditions.clone(),
        }),
        SessionCommandV1::Clear(ClearItemV1 {
            item_id: item_id("text"),
            preconditions: item_preconditions,
        }),
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: expected_attempt_id.clone(),
            next_attempt_id: Some(attempt_id(4)),
            local_artifact_verifications: Vec::new(),
        }),
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: expected_attempt_id.clone(),
            reason: Some("not needed".to_owned()),
            next_attempt_id: Some(attempt_id(4)),
        }),
        SessionCommandV1::Retry(RetrySessionV1 {
            expected_attempt_id: expected_attempt_id.clone(),
            reason: "retry".to_owned(),
            next_attempt_id: attempt_id(4),
        }),
        SessionCommandV1::Return(ReturnSessionV1 {
            expected_attempt_id: expected_attempt_id.clone(),
            destination_stage_id: stage_id("one"),
            reason: "redo".to_owned(),
            destination_attempt_id: attempt_id(4),
        }),
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: expected_attempt_id.clone(),
            blocker_id: blocker_id(4),
            reason: "waiting".to_owned(),
        }),
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: expected_attempt_id.clone(),
            blocker_id: None,
            unblock_all: true,
        }),
        SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
            expected_attempt_id,
            reason: "stop again".to_owned(),
        }),
    ]
}

fn complete_to_second(session: &SessionAggregateV1, now: u64) -> SessionAggregateV1 {
    apply_next(
        session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(session).attempt_id().clone(),
            next_attempt_id: Some(attempt_id(4)),
            local_artifact_verifications: Vec::new(),
        }),
        now,
    )
    .1
}

fn completed_with(session_number: u32, first_attempt_number: u32) -> SessionAggregateV1 {
    let first = started_with(fixture_snapshot(), session_number, first_attempt_number);
    let second = apply_next(
        &first,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&first).attempt_id().clone(),
            next_attempt_id: Some(attempt_id(first_attempt_number + 1)),
            local_artifact_verifications: Vec::new(),
        }),
        11,
    )
    .1;
    let third = apply_next(
        &second,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&second).attempt_id().clone(),
            next_attempt_id: Some(attempt_id(first_attempt_number + 2)),
            local_artifact_verifications: Vec::new(),
        }),
        12,
    )
    .1;
    apply_next(
        &third,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&third).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        13,
    )
    .1
}

fn completed() -> SessionAggregateV1 {
    completed_with(2, 3)
}

fn case_start() {
    let command = SessionCommandV1::Start(start_input(2, 3));
    for task_title in ["", " \t"] {
        let mut input = start_input(2, 3);
        input.task_title = task_title.to_owned();
        assert_start_rejected_without_mutation_with_error(
            SessionCommandV1::Start(input),
            DomainError::InvalidState {
                reason: "task title must contain between one and 500 non-blank scalars",
            },
        );
    }

    let outcome = apply_equivalent(
        None,
        &command,
        CommandContextV1 {
            expected_revision: Revision::ZERO,
            now: UnixMillis::new(10),
        },
    );
    let session = outcome.next_aggregate().unwrap();
    assert!(outcome.changed());
    assert_eq!(outcome.revision_before(), None);
    assert_eq!(outcome.revision_after(), Some(Revision::new(1)));
    assert_eq!(session.lifecycle(), SessionLifecycle::Running);
    assert_eq!(session.revision(), Revision::new(1));
    assert_eq!(session.latest_recorded_at(), UnixMillis::new(10));
    assert_stage_states(
        session,
        &[
            StageProgressState::Current,
            StageProgressState::Pending,
            StageProgressState::Pending,
        ],
    );
    assert_affected_stages(&outcome, &["one"]);
    assert_attempt(
        session,
        ExpectedAttempt {
            attempt_id: &attempt_id(3),
            stage_id: "one",
            number: 1,
            lifecycle: AttemptLifecycle::Active,
            started_at: 10,
            ended_at: None,
            reason: None,
        },
    );
    assert_fresh_active_attempt(session);
    assert_rejected_without_mutation(session, command.clone(), context(session, 11));
}

fn case_start_replace() {
    let session = started();
    for task_title in ["", " \t"] {
        let mut start = start_input(4, 5);
        start.task_title = task_title.to_owned();
        assert_rejected_without_mutation_with_error(
            &session,
            SessionCommandV1::StartReplace(StartReplaceSessionV1 {
                expected_session_id: session.session_id().clone(),
                confirmed: true,
                start,
            }),
            context(&session, 11),
            DomainError::InvalidState {
                reason: "task title must contain between one and 500 non-blank scalars",
            },
        );
    }
    let rejected = SessionCommandV1::StartReplace(StartReplaceSessionV1 {
        expected_session_id: session.session_id().clone(),
        confirmed: false,
        start: start_input(4, 5),
    });
    assert_rejected_without_mutation(&session, rejected, context(&session, 11));
    let stale_identity = SessionCommandV1::StartReplace(StartReplaceSessionV1 {
        expected_session_id: session_id(99),
        confirmed: true,
        start: start_input(4, 5),
    });
    assert_rejected_without_mutation(&session, stale_identity.clone(), context(&session, 11));
    assert_eq!(
        apply_transition_v1(Some(&session), &stale_identity, context(&session, 11)).unwrap_err(),
        DomainError::SessionIdentityMismatch {
            expected: session_id(99),
            actual: Some(session.session_id().clone()),
        }
    );
    let (_, cancelled) = apply_next(
        &session,
        SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            reason: "reset required".to_owned(),
        }),
        11,
    );
    let cancelled_start_replace = SessionCommandV1::StartReplace(StartReplaceSessionV1 {
        expected_session_id: cancelled.session_id().clone(),
        confirmed: true,
        start: start_input(4, 5),
    });
    let (cancelled_outcome, cancelled_replacement) =
        apply_next(&cancelled, cancelled_start_replace, 12);
    assert_eq!(cancelled_replacement.session_id(), &session_id(4));
    assert_eq!(cancelled_replacement.revision(), Revision::new(1));
    assert_affected_stages(&cancelled_outcome, &["one"]);
    assert_fresh_active_attempt(&cancelled_replacement);
    let (outcome, replaced) = apply_next(
        &session,
        SessionCommandV1::StartReplace(StartReplaceSessionV1 {
            expected_session_id: session.session_id().clone(),
            confirmed: true,
            start: start_input(4, 5),
        }),
        12,
    );
    assert_eq!(replaced.session_id(), &session_id(4));
    assert_eq!(replaced.revision(), Revision::new(1));
    assert_affected_stages(&outcome, &["one"]);
    assert_attempt(
        &replaced,
        ExpectedAttempt {
            attempt_id: &attempt_id(5),
            stage_id: "one",
            number: 1,
            lifecycle: AttemptLifecycle::Active,
            started_at: 12,
            ended_at: None,
            reason: None,
        },
    );
    assert_fresh_active_attempt(&replaced);
}

fn case_check() {
    let session = started();
    let (outcome, checked) = apply_next(
        &session,
        SessionCommandV1::Check(CheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: item_preconditions(&session, "confirm"),
        }),
        11,
    );
    assert_eq!(outcome.revision_after(), Some(Revision::new(2)));
    assert!(active_slot(&checked, "confirm").value().is_some());
    assert_stage_states(
        &checked,
        &[
            StageProgressState::Current,
            StageProgressState::Pending,
            StageProgressState::Pending,
        ],
    );
    assert_affected_stages(&outcome, &["one"]);
    let noop = apply_equivalent(
        Some(&checked),
        &SessionCommandV1::Check(CheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: item_preconditions(&checked, "confirm"),
        }),
        context(&checked, 12),
    );
    assert_stable_noop(&checked, &noop);
}

fn case_uncheck() {
    let session = started();
    let (_, checked) = apply_next(
        &session,
        SessionCommandV1::Check(CheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: item_preconditions(&session, "confirm"),
        }),
        11,
    );
    let (outcome, unchecked) = apply_next(
        &checked,
        SessionCommandV1::Uncheck(podway_core::UncheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: item_preconditions(&checked, "confirm"),
        }),
        12,
    );
    assert!(active_slot(&unchecked, "confirm").value().is_none());
    assert_eq!(unchecked.revision(), Revision::new(3));
    assert_stage_states(
        &unchecked,
        &[
            StageProgressState::Current,
            StageProgressState::Pending,
            StageProgressState::Pending,
        ],
    );
    assert_affected_stages(&outcome, &["one"]);
}

fn case_set() {
    let session = started();
    let (outcome, updated) = apply_next(
        &session,
        SessionCommandV1::Set(podway_core::SetItemV1 {
            item_id: item_id("text"),
            value: ItemValueV1::text("value"),
            preconditions: item_preconditions(&session, "text"),
        }),
        11,
    );
    assert_eq!(
        active_slot(&updated, "text")
            .value()
            .and_then(ItemValueV1::as_text),
        Some("value")
    );
    assert_eq!(updated.revision(), Revision::new(2));
    assert_stage_states(
        &updated,
        &[
            StageProgressState::Current,
            StageProgressState::Pending,
            StageProgressState::Pending,
        ],
    );
    assert_affected_stages(&outcome, &["one"]);
}

fn case_add() {
    let session = started();
    let (outcome, updated) = apply_next(
        &session,
        SessionCommandV1::Add(AddItemV1 {
            item_id: item_id("list"),
            value: "entry".to_owned(),
            preconditions: item_preconditions(&session, "list"),
        }),
        11,
    );
    assert_eq!(
        active_slot(&updated, "list")
            .value()
            .and_then(ItemValueV1::as_list),
        Some(["entry".to_owned()].as_slice())
    );
    assert_eq!(updated.revision(), Revision::new(2));
    assert_stage_states(
        &updated,
        &[
            StageProgressState::Current,
            StageProgressState::Pending,
            StageProgressState::Pending,
        ],
    );
    assert_affected_stages(&outcome, &["one"]);
    let mut at_capacity = updated;
    for (now, value) in [(12, "second"), (13, "third"), (14, "fourth")] {
        at_capacity = apply_next(
            &at_capacity,
            SessionCommandV1::Add(AddItemV1 {
                item_id: item_id("list"),
                value: value.to_owned(),
                preconditions: item_preconditions(&at_capacity, "list"),
            }),
            now,
        )
        .1;
    }
    assert_rejected_without_mutation_with_error(
        &at_capacity,
        SessionCommandV1::Add(AddItemV1 {
            item_id: item_id("list"),
            value: "overflow".to_owned(),
            preconditions: item_preconditions(&at_capacity, "list"),
        }),
        context(&at_capacity, 15),
        DomainError::InvalidState {
            reason: "item value does not meet its storage rules",
        },
    );
}

fn case_remove() {
    let session = started();
    let (added_outcome, added) = apply_next(
        &session,
        SessionCommandV1::Add(AddItemV1 {
            item_id: item_id("list"),
            value: "entry".to_owned(),
            preconditions: item_preconditions(&session, "list"),
        }),
        11,
    );
    let (outcome, removed) = apply_next(
        &added,
        SessionCommandV1::Remove(podway_core::RemoveItemV1 {
            item_id: item_id("list"),
            value: "entry".to_owned(),
            ignore_missing: false,
            preconditions: item_preconditions(&added, "list"),
        }),
        12,
    );
    assert_eq!(
        active_slot(&removed, "list")
            .value()
            .and_then(ItemValueV1::as_list),
        Some([].as_slice())
    );
    assert_eq!(removed.revision(), Revision::new(3));
    assert_affected_stages(&added_outcome, &["one"]);
    assert_eq!(outcome.revision_after(), Some(Revision::new(3)));
    assert_affected_stages(&outcome, &["one"]);
    assert_rejected_without_mutation_with_error(
        &removed,
        SessionCommandV1::Remove(podway_core::RemoveItemV1 {
            item_id: item_id("list"),
            value: "missing".to_owned(),
            ignore_missing: false,
            preconditions: item_preconditions(&removed, "list"),
        }),
        context(&removed, 13),
        DomainError::InvalidState {
            reason: "list item value is not present",
        },
    );
    let noop = apply_equivalent(
        Some(&removed),
        &SessionCommandV1::Remove(podway_core::RemoveItemV1 {
            item_id: item_id("list"),
            value: "missing".to_owned(),
            ignore_missing: true,
            preconditions: item_preconditions(&removed, "list"),
        }),
        context(&removed, 13),
    );
    assert_stable_noop(&removed, &noop);
}

fn case_attach() {
    let session = started();
    let artifact =
        ArtifactValueV1::external_reference("artifact:1", digest(), 1, "text/plain").unwrap();
    let (outcome, attached) = apply_next(
        &session,
        SessionCommandV1::Attach(podway_core::AttachItemV1 {
            item_id: item_id("artifact"),
            value: artifact.clone(),
            preconditions: item_preconditions(&session, "artifact"),
        }),
        11,
    );
    assert_eq!(
        active_slot(&attached, "artifact")
            .value()
            .and_then(ItemValueV1::as_artifact),
        Some(&artifact)
    );
    assert_eq!(attached.revision(), Revision::new(2));
    assert_affected_stages(&outcome, &["one"]);
    assert_eq!(outcome.revision_after(), Some(Revision::new(2)));
}

fn case_clear() {
    let session = started();
    let (_, set) = apply_next(
        &session,
        SessionCommandV1::Set(podway_core::SetItemV1 {
            item_id: item_id("text"),
            value: ItemValueV1::text("value"),
            preconditions: item_preconditions(&session, "text"),
        }),
        11,
    );
    let (outcome, cleared) = apply_next(
        &set,
        SessionCommandV1::Clear(ClearItemV1 {
            item_id: item_id("text"),
            preconditions: item_preconditions(&set, "text"),
        }),
        12,
    );
    assert!(active_slot(&cleared, "text").value().is_none());
    assert_eq!(active_slot(&cleared, "text").revision(), Revision::new(2));
    assert_affected_stages(&outcome, &["one"]);
    assert_eq!(outcome.revision_after(), Some(Revision::new(3)));
    let noop = apply_equivalent(
        Some(&cleared),
        &SessionCommandV1::Clear(ClearItemV1 {
            item_id: item_id("text"),
            preconditions: item_preconditions(&cleared, "text"),
        }),
        context(&cleared, 13),
    );
    assert_stable_noop(&cleared, &noop);
}

fn case_complete() {
    let session = started();
    assert_rejected_without_mutation(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        context(&session, 11),
    );
    let (outcome, second) = apply_next(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: Some(attempt_id(4)),
            local_artifact_verifications: Vec::new(),
        }),
        11,
    );
    assert_eq!(outcome.revision_after(), Some(Revision::new(2)));
    assert_affected_stages(&outcome, &["one", "two"]);
    assert_stage_states(
        &second,
        &[
            StageProgressState::Done,
            StageProgressState::Current,
            StageProgressState::Pending,
        ],
    );
    assert_attempt(
        &second,
        ExpectedAttempt {
            attempt_id: &attempt_id(3),
            stage_id: "one",
            number: 1,
            lifecycle: AttemptLifecycle::Completed,
            started_at: 10,
            ended_at: Some(11),
            reason: None,
        },
    );
    assert_attempt(
        &second,
        ExpectedAttempt {
            attempt_id: &attempt_id(4),
            stage_id: "two",
            number: 1,
            lifecycle: AttemptLifecycle::Active,
            started_at: 11,
            ended_at: None,
            reason: None,
        },
    );
    assert_fresh_active_attempt(&second);

    let (_, third) = apply_next(
        &second,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&second).attempt_id().clone(),
            next_attempt_id: Some(attempt_id(5)),
            local_artifact_verifications: Vec::new(),
        }),
        12,
    );
    assert_rejected_without_mutation(
        &third,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&third).attempt_id().clone(),
            next_attempt_id: Some(attempt_id(6)),
            local_artifact_verifications: Vec::new(),
        }),
        context(&third, 13),
    );
    let (final_outcome, final_session) = apply_next(
        &third,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&third).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        13,
    );
    assert_eq!(final_session.lifecycle(), SessionLifecycle::Completed);
    assert!(final_session.active_attempt_id().is_none());
    assert_affected_stages(&final_outcome, &["three"]);
    assert_stage_states(
        &final_session,
        &[
            StageProgressState::Done,
            StageProgressState::Done,
            StageProgressState::Done,
        ],
    );
    assert!(
        final_session
            .attempts()
            .iter()
            .all(|attempt| attempt.attempt_id() != &attempt_id(6))
    );
    assert_attempt(
        &final_session,
        ExpectedAttempt {
            attempt_id: &attempt_id(5),
            stage_id: "three",
            number: 1,
            lifecycle: AttemptLifecycle::Completed,
            started_at: 12,
            ended_at: Some(13),
            reason: None,
        },
    );
}

fn case_skip() {
    let session = started();
    for reason in ["", " \t"] {
        assert_blank_reason_rejected(
            &session,
            SessionCommandV1::Skip(SkipSessionV1 {
                expected_attempt_id: active_attempt(&session).attempt_id().clone(),
                reason: Some(reason.to_owned()),
                next_attempt_id: Some(attempt_id(4)),
            }),
            11,
        );
    }
    assert_rejected_without_mutation(
        &session,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            reason: None,
            next_attempt_id: Some(attempt_id(4)),
        }),
        context(&session, 11),
    );
    assert_rejected_without_mutation(
        &session,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            reason: Some("next attempt is required".to_owned()),
            next_attempt_id: None,
        }),
        context(&session, 11),
    );
    let (outcome, skipped) = apply_next(
        &session,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            reason: Some("not needed".to_owned()),
            next_attempt_id: Some(attempt_id(4)),
        }),
        11,
    );
    assert_affected_stages(&outcome, &["one", "two"]);
    assert_eq!(outcome.revision_after(), Some(Revision::new(2)));
    assert_stage_states(
        &skipped,
        &[
            StageProgressState::Skipped,
            StageProgressState::Current,
            StageProgressState::Pending,
        ],
    );
    assert_attempt(
        &skipped,
        ExpectedAttempt {
            attempt_id: &attempt_id(3),
            stage_id: "one",
            number: 1,
            lifecycle: AttemptLifecycle::Skipped,
            started_at: 10,
            ended_at: Some(11),
            reason: Some("not needed"),
        },
    );
    assert_attempt(
        &skipped,
        ExpectedAttempt {
            attempt_id: &attempt_id(4),
            stage_id: "two",
            number: 1,
            lifecycle: AttemptLifecycle::Active,
            started_at: 11,
            ended_at: None,
            reason: None,
        },
    );
    assert_fresh_active_attempt(&skipped);

    let no_reason_required = started_with(
        fixture_snapshot_with(
            [
                SkipPolicyV1::allowed(false),
                SkipPolicyV1::allowed(false),
                SkipPolicyV1::allowed(false),
            ],
            ReturnPolicyV1::any_previous(),
            vec![
                ProcedureWarningCodeV1::StageHasNoRequiredItems,
                ProcedureWarningCodeV1::FinalStageSkippable,
                ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
            ],
        )
        .unwrap(),
        20,
        21,
    );
    let (_, no_reason_skipped) = apply_next(
        &no_reason_required,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: active_attempt(&no_reason_required).attempt_id().clone(),
            reason: None,
            next_attempt_id: Some(attempt_id(22)),
        }),
        11,
    );
    assert_attempt(
        &no_reason_skipped,
        ExpectedAttempt {
            attempt_id: &attempt_id(21),
            stage_id: "one",
            number: 1,
            lifecycle: AttemptLifecycle::Skipped,
            started_at: 10,
            ended_at: Some(11),
            reason: None,
        },
    );

    let not_allowed = started_with(
        fixture_snapshot_with(
            [
                SkipPolicyV1::not_allowed(),
                SkipPolicyV1::allowed(true),
                SkipPolicyV1::allowed(true),
            ],
            ReturnPolicyV1::any_previous(),
            vec![
                ProcedureWarningCodeV1::StageHasNoRequiredItems,
                ProcedureWarningCodeV1::FinalStageSkippable,
                ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
            ],
        )
        .unwrap(),
        30,
        31,
    );
    assert_rejected_without_mutation(
        &not_allowed,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: active_attempt(&not_allowed).attempt_id().clone(),
            reason: Some("policy cannot permit this".to_owned()),
            next_attempt_id: Some(attempt_id(32)),
        }),
        context(&not_allowed, 11),
    );
}

fn case_retry() {
    let session = started();
    for reason in ["", " \t"] {
        assert_blank_reason_rejected(
            &session,
            SessionCommandV1::Retry(RetrySessionV1 {
                expected_attempt_id: active_attempt(&session).attempt_id().clone(),
                reason: reason.to_owned(),
                next_attempt_id: attempt_id(4),
            }),
            11,
        );
    }
    let (_, filled) = apply_next(
        &session,
        SessionCommandV1::Set(podway_core::SetItemV1 {
            item_id: item_id("text"),
            value: ItemValueV1::text("value"),
            preconditions: item_preconditions(&session, "text"),
        }),
        11,
    );
    let (outcome, retried) = apply_next(
        &filled,
        SessionCommandV1::Retry(RetrySessionV1 {
            expected_attempt_id: active_attempt(&filled).attempt_id().clone(),
            reason: "retry".to_owned(),
            next_attempt_id: attempt_id(4),
        }),
        12,
    );
    assert_affected_stages(&outcome, &["one"]);
    assert_eq!(outcome.revision_after(), Some(Revision::new(3)));
    assert_stage_states(
        &retried,
        &[
            StageProgressState::Current,
            StageProgressState::Pending,
            StageProgressState::Pending,
        ],
    );
    assert_eq!(retried.stage_progress()[0].latest_attempt_number(), 2);
    assert_attempt(
        &retried,
        ExpectedAttempt {
            attempt_id: &attempt_id(3),
            stage_id: "one",
            number: 1,
            lifecycle: AttemptLifecycle::Abandoned,
            started_at: 10,
            ended_at: Some(12),
            reason: Some("retry"),
        },
    );
    assert_attempt(
        &retried,
        ExpectedAttempt {
            attempt_id: &attempt_id(4),
            stage_id: "one",
            number: 2,
            lifecycle: AttemptLifecycle::Active,
            started_at: 12,
            ended_at: None,
            reason: None,
        },
    );
    assert_fresh_active_attempt(&retried);
}

fn case_return() {
    let restricted_snapshot = fixture_snapshot_with(
        [SkipPolicyV1::allowed(true); 3],
        ReturnPolicyV1::only(vec![stage_id("one")]).unwrap(),
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::FinalStageSkippable,
        ],
    )
    .unwrap();
    let first = started_with(restricted_snapshot, 2, 3);
    let second = complete_to_second(&first, 11);
    let third = apply_next(
        &second,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&second).attempt_id().clone(),
            next_attempt_id: Some(attempt_id(5)),
            local_artifact_verifications: Vec::new(),
        }),
        12,
    )
    .1;
    for reason in ["", " \t"] {
        assert_blank_reason_rejected(
            &third,
            SessionCommandV1::Return(ReturnSessionV1 {
                expected_attempt_id: active_attempt(&third).attempt_id().clone(),
                destination_stage_id: stage_id("one"),
                reason: reason.to_owned(),
                destination_attempt_id: attempt_id(6),
            }),
            13,
        );
    }
    assert_rejected_without_mutation(
        &third,
        SessionCommandV1::Return(ReturnSessionV1 {
            expected_attempt_id: active_attempt(&third).attempt_id().clone(),
            destination_stage_id: stage_id("two"),
            reason: "policy rejects this destination".to_owned(),
            destination_attempt_id: attempt_id(6),
        }),
        context(&third, 13),
    );
    let (outcome, returned) = apply_next(
        &third,
        SessionCommandV1::Return(ReturnSessionV1 {
            expected_attempt_id: active_attempt(&third).attempt_id().clone(),
            destination_stage_id: stage_id("one"),
            reason: "redo".to_owned(),
            destination_attempt_id: attempt_id(7),
        }),
        13,
    );
    assert_affected_stages(&outcome, &["one", "two", "three"]);
    assert_eq!(outcome.revision_after(), Some(Revision::new(4)));
    assert_stage_states(
        &returned,
        &[
            StageProgressState::Current,
            StageProgressState::Redo,
            StageProgressState::Redo,
        ],
    );
    assert_attempt(
        &returned,
        ExpectedAttempt {
            attempt_id: &attempt_id(5),
            stage_id: "three",
            number: 1,
            lifecycle: AttemptLifecycle::Abandoned,
            started_at: 12,
            ended_at: Some(13),
            reason: Some("redo"),
        },
    );
    assert_attempt(
        &returned,
        ExpectedAttempt {
            attempt_id: &attempt_id(7),
            stage_id: "one",
            number: 2,
            lifecycle: AttemptLifecycle::Active,
            started_at: 13,
            ended_at: None,
            reason: None,
        },
    );
    assert_fresh_active_attempt(&returned);
}

fn case_block() {
    let session = started();
    for reason in ["", " \t"] {
        assert_blank_reason_rejected(
            &session,
            SessionCommandV1::Block(BlockSessionV1 {
                expected_attempt_id: active_attempt(&session).attempt_id().clone(),
                blocker_id: blocker_id(4),
                reason: reason.to_owned(),
            }),
            11,
        );
    }
    let (outcome, blocked) = apply_next(
        &session,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            blocker_id: blocker_id(4),
            reason: "waiting".to_owned(),
        }),
        11,
    );
    assert_affected_stages(&outcome, &["one"]);
    assert_eq!(outcome.revision_after(), Some(Revision::new(2)));
    assert_stage_states(
        &blocked,
        &[
            StageProgressState::Current,
            StageProgressState::Pending,
            StageProgressState::Pending,
        ],
    );
    let blocker = blocker_by_id(active_attempt(&blocked), &blocker_id(4));
    assert_eq!(blocker.blocker_id(), &blocker_id(4));
    assert_eq!(blocker.attempt_id(), &attempt_id(3));
    assert_eq!(blocker.reason(), "waiting");
    assert_eq!(blocker.state(), BlockerState::Open);
    assert_eq!(blocker.created_at(), UnixMillis::new(11));
    assert_eq!(blocker.resolved_at(), None);
    assert_rejected_without_mutation(
        &blocked,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: active_attempt(&blocked).attempt_id().clone(),
            blocker_id: blocker_id(4),
            reason: "duplicate".to_owned(),
        }),
        context(&blocked, 12),
    );
}

fn case_unblock() {
    let session = started();
    let (_, first_blocked) = apply_next(
        &session,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            blocker_id: blocker_id(4),
            reason: "first".to_owned(),
        }),
        11,
    );
    let (_, blocked) = apply_next(
        &first_blocked,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: active_attempt(&first_blocked).attempt_id().clone(),
            blocker_id: blocker_id(5),
            reason: "second".to_owned(),
        }),
        12,
    );
    for command in [
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: active_attempt(&blocked).attempt_id().clone(),
            blocker_id: None,
            unblock_all: false,
        }),
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: active_attempt(&blocked).attempt_id().clone(),
            blocker_id: Some(blocker_id(4)),
            unblock_all: true,
        }),
    ] {
        assert_rejected_without_mutation(&blocked, command, context(&blocked, 13));
    }

    let (targeted_outcome, targeted) = apply_next(
        &blocked,
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: active_attempt(&blocked).attempt_id().clone(),
            blocker_id: Some(blocker_id(4)),
            unblock_all: false,
        }),
        13,
    );
    assert_affected_stages(&targeted_outcome, &["one"]);
    assert_eq!(targeted_outcome.revision_after(), Some(Revision::new(4)));
    let targeted_blocker = blocker_by_id(active_attempt(&targeted), &blocker_id(4));
    assert_eq!(targeted_blocker.state(), BlockerState::Resolved);
    assert_eq!(targeted_blocker.created_at(), UnixMillis::new(11));
    assert_eq!(targeted_blocker.resolved_at(), Some(UnixMillis::new(13)));
    let remaining_blocker = blocker_by_id(active_attempt(&targeted), &blocker_id(5));
    assert_eq!(remaining_blocker.state(), BlockerState::Open);
    assert_eq!(remaining_blocker.created_at(), UnixMillis::new(12));
    assert_eq!(remaining_blocker.resolved_at(), None);

    let (all_outcome, unblocked) = apply_next(
        &targeted,
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: active_attempt(&targeted).attempt_id().clone(),
            blocker_id: None,
            unblock_all: true,
        }),
        14,
    );
    assert_affected_stages(&all_outcome, &["one"]);
    assert_eq!(all_outcome.revision_after(), Some(Revision::new(5)));
    assert!(
        active_attempt(&unblocked)
            .blockers()
            .iter()
            .all(|blocker| blocker.state() == BlockerState::Resolved)
    );
    let resolved_blocker = blocker_by_id(active_attempt(&unblocked), &blocker_id(5));
    assert_eq!(resolved_blocker.created_at(), UnixMillis::new(12));
    assert_eq!(resolved_blocker.resolved_at(), Some(UnixMillis::new(14)));

    let (_, retried) = apply_next(
        &unblocked,
        SessionCommandV1::Retry(RetrySessionV1 {
            expected_attempt_id: active_attempt(&unblocked).attempt_id().clone(),
            reason: "new current attempt".to_owned(),
            next_attempt_id: attempt_id(6),
        }),
        15,
    );
    let (_, current_blocked) = apply_next(
        &retried,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: active_attempt(&retried).attempt_id().clone(),
            blocker_id: blocker_id(7),
            reason: "current".to_owned(),
        }),
        16,
    );
    let not_current = SessionCommandV1::Unblock(UnblockSessionV1 {
        expected_attempt_id: active_attempt(&current_blocked).attempt_id().clone(),
        blocker_id: Some(blocker_id(4)),
        unblock_all: false,
    });
    assert_rejected_without_mutation(
        &current_blocked,
        not_current.clone(),
        context(&current_blocked, 17),
    );
    assert_eq!(
        apply_transition_v1(
            Some(&current_blocked),
            &not_current,
            context(&current_blocked, 17)
        )
        .unwrap_err(),
        DomainError::BlockerNotCurrent
    );
}

fn case_cancel() {
    let session = started();
    for reason in ["", " \t"] {
        assert_blank_reason_rejected(
            &session,
            SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
                expected_attempt_id: active_attempt(&session).attempt_id().clone(),
                reason: reason.to_owned(),
            }),
            11,
        );
    }
    let (_, blocked) = apply_next(
        &session,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            blocker_id: blocker_id(4),
            reason: "waiting".to_owned(),
        }),
        11,
    );
    let blocker = blocker_by_id(active_attempt(&blocked), &blocker_id(4));
    assert_eq!(blocker.state(), BlockerState::Open);
    assert_eq!(blocker.created_at(), UnixMillis::new(11));
    assert_eq!(blocker.resolved_at(), None);

    let (outcome, cancelled) = apply_next(
        &blocked,
        SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
            expected_attempt_id: active_attempt(&blocked).attempt_id().clone(),
            reason: "stop".to_owned(),
        }),
        12,
    );
    assert_eq!(cancelled.lifecycle(), SessionLifecycle::Cancelled);
    assert!(cancelled.active_attempt_id().is_none());
    assert_affected_stages(&outcome, &["one"]);
    assert_eq!(outcome.revision_after(), Some(Revision::new(3)));
    assert_stage_states(
        &cancelled,
        &[
            StageProgressState::Abandoned,
            StageProgressState::Pending,
            StageProgressState::Pending,
        ],
    );
    assert_attempt(
        &cancelled,
        ExpectedAttempt {
            attempt_id: &attempt_id(3),
            stage_id: "one",
            number: 1,
            lifecycle: AttemptLifecycle::Abandoned,
            started_at: 10,
            ended_at: Some(12),
            reason: Some("stop"),
        },
    );
    let blocker = blocker_by_id(attempt_by_id(&cancelled, &attempt_id(3)), &blocker_id(4));
    assert_eq!(blocker.created_at(), UnixMillis::new(11));
    assert_eq!(blocker.state(), BlockerState::Resolved);
    assert_eq!(blocker.resolved_at(), Some(UnixMillis::new(12)));
}

fn case_reopen() {
    let session = completed();
    for reason in ["", " \t"] {
        assert_blank_reason_rejected(
            &session,
            SessionCommandV1::Reopen(ReopenSessionV1 {
                expected_session_id: session.session_id().clone(),
                destination_stage_id: stage_id("one"),
                reason: reason.to_owned(),
                destination_attempt_id: attempt_id(6),
            }),
            14,
        );
    }
    let (outcome, reopened) = apply_next(
        &session,
        SessionCommandV1::Reopen(ReopenSessionV1 {
            expected_session_id: session.session_id().clone(),
            destination_stage_id: stage_id("one"),
            reason: "follow-up".to_owned(),
            destination_attempt_id: attempt_id(6),
        }),
        14,
    );
    assert_eq!(reopened.lifecycle(), SessionLifecycle::Running);
    assert_affected_stages(&outcome, &["one", "two", "three"]);
    assert_eq!(outcome.revision_after(), Some(Revision::new(5)));
    assert_stage_states(
        &reopened,
        &[
            StageProgressState::Current,
            StageProgressState::Redo,
            StageProgressState::Redo,
        ],
    );
    assert_attempt(
        &reopened,
        ExpectedAttempt {
            attempt_id: &attempt_id(6),
            stage_id: "one",
            number: 2,
            lifecycle: AttemptLifecycle::Active,
            started_at: 14,
            ended_at: None,
            reason: Some("follow-up"),
        },
    );
    assert_pristine_active_attempt(&reopened, Some("follow-up"));
}

fn case_reset() {
    let session = started();
    assert_rejected_without_mutation(
        &session,
        SessionCommandV1::Reset(ResetSessionV1 {
            expected_session_id: session.session_id().clone(),
            confirmed: false,
        }),
        context(&session, 11),
    );
    let stale_identity = SessionCommandV1::Reset(ResetSessionV1 {
        expected_session_id: session_id(99),
        confirmed: true,
    });
    assert_rejected_without_mutation(&session, stale_identity.clone(), context(&session, 11));
    assert_eq!(
        apply_transition_v1(Some(&session), &stale_identity, context(&session, 11)).unwrap_err(),
        DomainError::SessionIdentityMismatch {
            expected: session_id(99),
            actual: Some(session.session_id().clone()),
        }
    );
    let outcome = apply_equivalent(
        Some(&session),
        &SessionCommandV1::Reset(ResetSessionV1 {
            expected_session_id: session.session_id().clone(),
            confirmed: true,
        }),
        context(&session, 12),
    );
    assert!(outcome.changed());
    assert!(outcome.next_aggregate().is_none());
    assert_affected_stages(&outcome, &["one", "two", "three"]);
}

fn case_reset_all() {
    assert!(
        apply_transition_v1(
            None,
            &SessionCommandV1::ResetAll(ResetAllWorkspaceV1 {
                workspace_id: None,
                confirmed: false,
            }),
            CommandContextV1 {
                expected_revision: Revision::ZERO,
                now: UnixMillis::new(10),
            },
        )
        .is_err()
    );
    let outcome = apply_equivalent(
        None,
        &SessionCommandV1::ResetAll(ResetAllWorkspaceV1 {
            workspace_id: None,
            confirmed: true,
        }),
        CommandContextV1 {
            expected_revision: Revision::ZERO,
            now: UnixMillis::new(10),
        },
    );
    assert!(outcome.changed());
    assert!(outcome.next_aggregate().is_none());
    assert!(outcome.effect().is_some());
    assert_affected_stages(&outcome, &[]);
}

fn assert_cross_cutting_preconditions() {
    let session = started();
    let expected_attempt_id = active_attempt(&session).attempt_id().clone();
    let stale_revision_prior = session.clone();
    let stale_revision = apply_transition_v1(
        Some(&session),
        &SessionCommandV1::Retry(RetrySessionV1 {
            expected_attempt_id: expected_attempt_id.clone(),
            reason: "stale revision".to_owned(),
            next_attempt_id: attempt_id(4),
        }),
        CommandContextV1 {
            expected_revision: Revision::ZERO,
            now: UnixMillis::new(11),
        },
    )
    .unwrap_err();
    assert_eq!(
        stale_revision,
        DomainError::PreconditionFailed {
            expected: Revision::ZERO,
            actual: Revision::new(1),
        }
    );
    assert_eq!(session, stale_revision_prior);

    assert_rejected_without_mutation(
        &session,
        SessionCommandV1::Retry(RetrySessionV1 {
            expected_attempt_id: attempt_id(99),
            reason: "stale attempt".to_owned(),
            next_attempt_id: attempt_id(4),
        }),
        context(&session, 11),
    );

    let stale_item = item_preconditions(&session, "confirm");
    let (_, checked) = apply_next(
        &session,
        SessionCommandV1::Check(CheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: stale_item.clone(),
        }),
        11,
    );
    let stale_item_prior = checked.clone();
    let stale_item_error = apply_transition_v1(
        Some(&checked),
        &SessionCommandV1::Check(CheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: stale_item,
        }),
        context(&checked, 12),
    )
    .unwrap_err();
    assert_eq!(
        stale_item_error,
        DomainError::PreconditionFailed {
            expected: Revision::ZERO,
            actual: Revision::new(1),
        }
    );
    assert_eq!(checked, stale_item_prior);
    assert_rejected_without_mutation(
        &checked,
        SessionCommandV1::Check(CheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: ItemMutationPreconditionsV1 {
                expected_attempt_id: expected_attempt_id.clone(),
                expected_item_revision: Revision::ZERO,
            },
        }),
        context(&checked, 12),
    );

    let (_, cancelled) = apply_next(
        &checked,
        SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
            expected_attempt_id,
            reason: "stop".to_owned(),
        }),
        13,
    );
    let completed = completed();
    assert_eq!(running_only_commands(attempt_id(3)).len(), 14);
    for terminal in [&completed, &cancelled] {
        for command in running_only_commands(attempt_id(3)) {
            assert_rejected_without_mutation(terminal, command, context(terminal, 20));
        }
    }

    assert_rejected_without_mutation(
        &session,
        SessionCommandV1::Reopen(ReopenSessionV1 {
            expected_session_id: session.session_id().clone(),
            destination_stage_id: stage_id("one"),
            reason: "not complete".to_owned(),
            destination_attempt_id: attempt_id(4),
        }),
        context(&session, 11),
    );

    let foreign_completed = completed_with(20, 30);
    let stale_identity_reopen = SessionCommandV1::Reopen(ReopenSessionV1 {
        expected_session_id: foreign_completed.session_id().clone(),
        destination_stage_id: stage_id("one"),
        reason: "cross-session".to_owned(),
        destination_attempt_id: attempt_id(6),
    });
    assert_rejected_without_mutation(
        &completed,
        stale_identity_reopen.clone(),
        context(&completed, 20),
    );
    assert_eq!(
        apply_transition_v1(
            Some(&completed),
            &stale_identity_reopen,
            context(&completed, 20),
        )
        .unwrap_err(),
        DomainError::SessionIdentityMismatch {
            expected: foreign_completed.session_id().clone(),
            actual: Some(completed.session_id().clone()),
        }
    );

    assert_rejected_without_mutation(
        &cancelled,
        SessionCommandV1::Reopen(ReopenSessionV1 {
            expected_session_id: cancelled.session_id().clone(),
            destination_stage_id: stage_id("one"),
            reason: "cancelled sessions cannot reopen".to_owned(),
            destination_attempt_id: attempt_id(4),
        }),
        context(&cancelled, 20),
    );
}

#[test]
fn every_state_matrix_row_has_exactly_one_explicit_conformance_classification() {
    let matrix_rows = parse_matrix();
    let matrix_commands = matrix_rows
        .iter()
        .map(|row| row.command.as_str())
        .collect::<Vec<_>>();
    let expected_commands = MATRIX_EXPECTATIONS
        .iter()
        .map(|expected| expected.command)
        .collect::<Vec<_>>();
    let matrix_set = matrix_commands.iter().copied().collect::<BTreeSet<_>>();
    let expected_set = expected_commands.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(matrix_commands.len(), matrix_set.len());
    assert_eq!(expected_commands.len(), expected_set.len());
    assert_eq!(matrix_rows.len(), MATRIX_EXPECTATIONS.len());
    assert_eq!(matrix_set, expected_set);

    for (row, expected) in matrix_rows.iter().zip(MATRIX_EXPECTATIONS) {
        assert_matrix_row(row, expected);
    }

    let pure_case_names = MATRIX_EXPECTATIONS
        .iter()
        .filter_map(|expected| match expected.classification {
            MatrixClassification::ExternalGitStoreBoundary => None,
            MatrixClassification::PureDomain(case) => Some(case.name()),
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(pure_case_names.len(), MATRIX_EXPECTATIONS.len() - 1);

    let workspace_init = MATRIX_EXPECTATIONS
        .iter()
        .find(|expected| expected.command == "workspace.init")
        .unwrap();
    assert_eq!(
        workspace_init.classification,
        MatrixClassification::ExternalGitStoreBoundary
    );
    assert_matrix_row(
        matrix_rows
            .iter()
            .find(|row| row.command == workspace_init.command)
            .unwrap(),
        *workspace_init,
    );

    for expected in MATRIX_EXPECTATIONS {
        match expected.classification {
            MatrixClassification::ExternalGitStoreBoundary => {
                assert_eq!(expected.command, "workspace.init");
            }
            MatrixClassification::PureDomain(case) => {
                assert_eq!(expected.command, case.name());
                case.exercise();
            }
        }
    }
}

#[test]
fn matrix_cross_cutting_preconditions_preserve_the_prior_aggregate() {
    assert_cross_cutting_preconditions();
}
#[test]
fn pac_066_retry_return_and_reopen_leave_exactly_one_active_attempt_and_reset_deletes_session_history()
 {
    case_retry();
    case_return();
    case_reopen();
    case_reset();

    let session = started();
    let (_, cancelled) = apply_next(
        &session,
        SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            reason: "stop".to_owned(),
        }),
        11,
    );
    assert_rejected_without_mutation_with_error(
        &cancelled,
        SessionCommandV1::Reopen(ReopenSessionV1 {
            expected_session_id: cancelled.session_id().clone(),
            destination_stage_id: stage_id("one"),
            reason: "cannot reopen a cancelled session".to_owned(),
            destination_attempt_id: attempt_id(4),
        }),
        context(&cancelled, 12),
        DomainError::InvalidTransition {
            command: podway_core::DomainCommandKind::SessionReopen,
            state: SessionLifecycle::Cancelled,
        },
    );
}
