use crate::aggregate::{AttemptV1, BlockerV1, SessionAggregateV1, item_satisfied};
use crate::procedure::{ItemSpecV1, StageSpecV1};
use crate::{
    AttemptId, BlockerId, BlockerState, ItemId, ItemTypeV1, Revision, SessionId, SessionLifecycle,
    StageId, StageProgressState,
};

/// The lifecycle represented by a derived session view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStatusV1 {
    Running,
    Completed,
    Cancelled,
}

impl From<SessionLifecycle> for SessionStatusV1 {
    fn from(lifecycle: SessionLifecycle) -> Self {
        match lifecycle {
            SessionLifecycle::Running => Self::Running,
            SessionLifecycle::Completed => Self::Completed,
            SessionLifecycle::Cancelled => Self::Cancelled,
        }
    }
}

/// A stage state rendered for a derived view. `Blocked` is never persisted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DerivedStageStatusV1 {
    Pending,
    Current,
    Blocked,
    Done,
    Skipped,
    Redo,
    Abandoned,
}

/// The immutable, ordered status of one procedure stage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageStatusViewV1 {
    stage_id: StageId,
    stage_index: usize,
    title: String,
    status: DerivedStageStatusV1,
    latest_attempt_number: u32,
}

impl StageStatusViewV1 {
    pub fn stage_id(&self) -> &StageId {
        &self.stage_id
    }

    pub const fn stage_index(&self) -> usize {
        self.stage_index
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub const fn status(&self) -> DerivedStageStatusV1 {
        self.status
    }

    pub const fn latest_attempt_number(&self) -> u32 {
        self.latest_attempt_number
    }
}

/// Satisfaction progress for one item on the active attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemProgressViewV1 {
    item_id: ItemId,
    item_type: ItemTypeV1,
    prompt: String,
    required: bool,
    satisfied: bool,
    revision: Revision,
}

impl ItemProgressViewV1 {
    pub fn item_id(&self) -> &ItemId {
        &self.item_id
    }

    pub const fn item_type(&self) -> ItemTypeV1 {
        self.item_type
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub const fn required(&self) -> bool {
        self.required
    }

    pub const fn satisfied(&self) -> bool {
        self.satisfied
    }

    pub const fn revision(&self) -> Revision {
        self.revision
    }
}

/// An open blocker belonging to the active attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenBlockerViewV1 {
    blocker_id: BlockerId,
    attempt_id: AttemptId,
    reason: String,
}

impl OpenBlockerViewV1 {
    pub fn blocker_id(&self) -> &BlockerId {
        &self.blocker_id
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The active stage and attempt, including bounded item and blocker progress.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentStageAttemptViewV1 {
    stage_id: StageId,
    stage_index: usize,
    title: String,
    attempt_id: AttemptId,
    attempt_number: u32,
    blocked: bool,
    ready_to_complete: bool,
    open_blockers: Vec<OpenBlockerViewV1>,
    required_items: Vec<ItemProgressViewV1>,
    optional_items: Vec<ItemProgressViewV1>,
}

impl CurrentStageAttemptViewV1 {
    pub fn stage_id(&self) -> &StageId {
        &self.stage_id
    }

    pub const fn stage_index(&self) -> usize {
        self.stage_index
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub const fn attempt_number(&self) -> u32 {
        self.attempt_number
    }

    pub const fn blocked(&self) -> bool {
        self.blocked
    }

    pub const fn ready_to_complete(&self) -> bool {
        self.ready_to_complete
    }

    pub fn open_blockers(&self) -> &[OpenBlockerViewV1] {
        &self.open_blockers
    }

    pub fn required_items(&self) -> &[ItemProgressViewV1] {
        &self.required_items
    }

    pub fn optional_items(&self) -> &[ItemProgressViewV1] {
        &self.optional_items
    }
}

/// A complete pure status projection for one validated session aggregate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionStatusViewV1 {
    session_id: SessionId,
    revision: Revision,
    status: SessionStatusV1,
    current: Option<CurrentStageAttemptViewV1>,
    stages: Vec<StageStatusViewV1>,
}

impl SessionStatusViewV1 {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub const fn revision(&self) -> Revision {
        self.revision
    }

    pub const fn status(&self) -> SessionStatusV1 {
        self.status
    }

    pub fn current(&self) -> Option<&CurrentStageAttemptViewV1> {
        self.current.as_ref()
    }

    pub fn stages(&self) -> &[StageStatusViewV1] {
        &self.stages
    }
}

/// The active stage context reported by `derive_next_work_v1`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextStageViewV1 {
    stage_id: StageId,
    stage_index: usize,
    title: String,
    instructions: Vec<String>,
    attempt_id: AttemptId,
    attempt_number: u32,
}

impl NextStageViewV1 {
    pub fn stage_id(&self) -> &StageId {
        &self.stage_id
    }

    pub const fn stage_index(&self) -> usize {
        self.stage_index
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn instructions(&self) -> &[String] {
        &self.instructions
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub const fn attempt_number(&self) -> u32 {
        self.attempt_number
    }
}

/// The next stage in immutable procedure order after the current stage completes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextStageAfterCompletionViewV1 {
    stage_id: StageId,
    stage_index: usize,
    title: String,
}

impl NextStageAfterCompletionViewV1 {
    pub fn stage_id(&self) -> &StageId {
        &self.stage_id
    }

    pub const fn stage_index(&self) -> usize {
        self.stage_index
    }

    pub fn title(&self) -> &str {
        &self.title
    }
}

/// A required active-stage item that remains unsatisfied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextItemViewV1 {
    item_id: ItemId,
    item_type: ItemTypeV1,
    prompt: String,
    help: Option<String>,
}

impl NextItemViewV1 {
    pub fn item_id(&self) -> &ItemId {
        &self.item_id
    }

    pub const fn item_type(&self) -> ItemTypeV1 {
        self.item_type
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn help(&self) -> Option<&str> {
        self.help.as_deref()
    }
}

/// The first action that can advance active-stage work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NextActionV1 {
    Item(NextItemViewV1),
    Stage(NextStageViewV1),
}

impl NextActionV1 {
    pub fn item(&self) -> Option<&NextItemViewV1> {
        match self {
            Self::Item(item) => Some(item),
            Self::Stage(_) => None,
        }
    }

    pub fn stage(&self) -> Option<&NextStageViewV1> {
        match self {
            Self::Item(_) => None,
            Self::Stage(stage) => Some(stage),
        }
    }
}

/// A pure next-work projection for one validated session aggregate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextWorkViewV1 {
    status: SessionStatusV1,
    stage: Option<NextStageViewV1>,
    missing_required_items: Vec<NextItemViewV1>,
    open_blockers: Vec<OpenBlockerViewV1>,
    next_action: Option<NextActionV1>,
    next_stage_after_completion: Option<NextStageAfterCompletionViewV1>,
}

impl NextWorkViewV1 {
    pub const fn status(&self) -> SessionStatusV1 {
        self.status
    }

    pub fn stage(&self) -> Option<&NextStageViewV1> {
        self.stage.as_ref()
    }

    pub fn missing_required_items(&self) -> &[NextItemViewV1] {
        &self.missing_required_items
    }

    pub fn open_blockers(&self) -> &[OpenBlockerViewV1] {
        &self.open_blockers
    }

    pub fn next_action(&self) -> Option<&NextActionV1> {
        self.next_action.as_ref()
    }

    pub fn next_stage_after_completion(&self) -> Option<&NextStageAfterCompletionViewV1> {
        self.next_stage_after_completion.as_ref()
    }
}

/// Derives the status projection without I/O, mutation, clocks, or generated identifiers.
pub fn derive_session_status_v1(aggregate: &SessionAggregateV1) -> SessionStatusViewV1 {
    let current = if aggregate.lifecycle() == SessionLifecycle::Running {
        Some(current_context(aggregate))
    } else {
        None
    };
    let blocked_stage_id = current.as_ref().and_then(|context| {
        context
            .attempt
            .blockers()
            .iter()
            .any(|blocker| blocker.state() == BlockerState::Open)
            .then_some(context.stage.id())
    });
    let stages = aggregate
        .snapshot()
        .stages()
        .iter()
        .zip(aggregate.stage_progress())
        .enumerate()
        .map(|(index, (stage, progress))| {
            stage_status_view(
                stage,
                index,
                progress.state(),
                progress.latest_attempt_number(),
                blocked_stage_id,
            )
        })
        .collect();
    let current = current.map(current_stage_attempt_view);

    SessionStatusViewV1 {
        session_id: aggregate.session_id().clone(),
        revision: aggregate.revision(),
        status: aggregate.lifecycle().into(),
        current,
        stages,
    }
}

/// Derives the next active-stage item or completion action in immutable item order.
pub fn derive_next_work_v1(aggregate: &SessionAggregateV1) -> NextWorkViewV1 {
    let status = aggregate.lifecycle().into();
    if aggregate.lifecycle() != SessionLifecycle::Running {
        return NextWorkViewV1 {
            status,
            stage: None,
            missing_required_items: Vec::new(),
            open_blockers: Vec::new(),
            next_action: None,
            next_stage_after_completion: None,
        };
    }
    let context = current_context(aggregate);

    let stage = next_stage_view(&context);
    let open_blockers = open_blocker_views(context.attempt);
    let missing_required_items = context
        .stage
        .items()
        .iter()
        .filter(|item| item.common().required())
        .filter(|item| !item_is_satisfied(item, context.attempt))
        .map(next_item_view)
        .collect::<Vec<_>>();
    let next_action = if !open_blockers.is_empty() {
        None
    } else if let Some(item) = missing_required_items.first() {
        Some(NextActionV1::Item(item.clone()))
    } else {
        Some(NextActionV1::Stage(stage.clone()))
    };
    let next_stage_after_completion = aggregate
        .snapshot()
        .stages()
        .get(context.stage_index + 1)
        .map(|next_stage| NextStageAfterCompletionViewV1 {
            stage_id: next_stage.id().clone(),
            stage_index: context.stage_index + 1,
            title: next_stage.title().to_owned(),
        });

    NextWorkViewV1 {
        status,
        stage: Some(stage),
        missing_required_items,
        open_blockers,
        next_action,
        next_stage_after_completion,
    }
}

struct CurrentContextV1<'a> {
    stage: &'a StageSpecV1,
    stage_index: usize,
    attempt: &'a AttemptV1,
}

fn current_context(aggregate: &SessionAggregateV1) -> CurrentContextV1<'_> {
    let active_stage_id = aggregate
        .active_stage_id()
        .expect("validated running aggregate has an active stage");
    let active_attempt_id = aggregate
        .active_attempt_id()
        .expect("validated running aggregate has an active attempt");
    let stage_index = aggregate
        .snapshot()
        .stages()
        .iter()
        .position(|stage| stage.id() == active_stage_id)
        .expect("validated active stage exists in the snapshot");
    let stage = &aggregate.snapshot().stages()[stage_index];
    let attempt = aggregate
        .attempts()
        .iter()
        .find(|attempt| {
            attempt.attempt_id() == active_attempt_id && attempt.stage_id() == stage.id()
        })
        .expect("validated active attempt matches the active stage");

    CurrentContextV1 {
        stage,
        stage_index,
        attempt,
    }
}

fn stage_status_view(
    stage: &StageSpecV1,
    stage_index: usize,
    progress_state: StageProgressState,
    latest_attempt_number: u32,
    blocked_stage_id: Option<&StageId>,
) -> StageStatusViewV1 {
    let status = if matches!(progress_state, StageProgressState::Current)
        && blocked_stage_id.is_some_and(|stage_id| stage.id() == stage_id)
    {
        DerivedStageStatusV1::Blocked
    } else {
        derived_stage_status(progress_state)
    };

    StageStatusViewV1 {
        stage_id: stage.id().clone(),
        stage_index,
        title: stage.title().to_owned(),
        status,
        latest_attempt_number,
    }
}

fn derived_stage_status(state: StageProgressState) -> DerivedStageStatusV1 {
    match state {
        StageProgressState::Pending => DerivedStageStatusV1::Pending,
        StageProgressState::Current => DerivedStageStatusV1::Current,
        StageProgressState::Done => DerivedStageStatusV1::Done,
        StageProgressState::Skipped => DerivedStageStatusV1::Skipped,
        StageProgressState::Redo => DerivedStageStatusV1::Redo,
        StageProgressState::Abandoned => DerivedStageStatusV1::Abandoned,
    }
}

fn current_stage_attempt_view(context: CurrentContextV1<'_>) -> CurrentStageAttemptViewV1 {
    let open_blockers = open_blocker_views(context.attempt);
    let mut required_items = Vec::new();
    let mut optional_items = Vec::new();
    for item in context.stage.items() {
        let progress = item_progress_view(item, context.attempt);
        if item.common().required() {
            required_items.push(progress);
        } else {
            optional_items.push(progress);
        }
    }

    CurrentStageAttemptViewV1 {
        stage_id: context.stage.id().clone(),
        stage_index: context.stage_index,
        title: context.stage.title().to_owned(),
        attempt_id: context.attempt.attempt_id().clone(),
        attempt_number: context.attempt.number(),
        blocked: !open_blockers.is_empty(),
        ready_to_complete: context.attempt.is_ready_to_complete(context.stage),
        open_blockers,
        required_items,
        optional_items,
    }
}

fn next_stage_view(context: &CurrentContextV1<'_>) -> NextStageViewV1 {
    NextStageViewV1 {
        stage_id: context.stage.id().clone(),
        stage_index: context.stage_index,
        title: context.stage.title().to_owned(),
        instructions: context.stage.instructions().to_vec(),
        attempt_id: context.attempt.attempt_id().clone(),
        attempt_number: context.attempt.number(),
    }
}

fn item_progress_view(item: &ItemSpecV1, attempt: &AttemptV1) -> ItemProgressViewV1 {
    let slot = attempt
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == item.id());
    ItemProgressViewV1 {
        item_id: item.id().clone(),
        item_type: item.item_type(),
        prompt: item.common().prompt().to_owned(),
        required: item.common().required(),
        satisfied: slot.is_some_and(|slot| item_satisfied(item, slot.value())),
        revision: slot.map_or(Revision::ZERO, |slot| slot.revision()),
    }
}

fn item_is_satisfied(item: &ItemSpecV1, attempt: &AttemptV1) -> bool {
    attempt
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == item.id())
        .is_some_and(|slot| item_satisfied(item, slot.value()))
}

fn next_item_view(item: &ItemSpecV1) -> NextItemViewV1 {
    NextItemViewV1 {
        item_id: item.id().clone(),
        item_type: item.item_type(),
        prompt: item.common().prompt().to_owned(),
        help: item.common().help().map(str::to_owned),
    }
}

fn open_blocker_views(attempt: &AttemptV1) -> Vec<OpenBlockerViewV1> {
    attempt
        .blockers()
        .iter()
        .filter(|blocker| blocker.state() == BlockerState::Open)
        .map(open_blocker_view)
        .collect()
}

fn open_blocker_view(blocker: &BlockerV1) -> OpenBlockerViewV1 {
    OpenBlockerViewV1 {
        blocker_id: blocker.blocker_id().clone(),
        attempt_id: blocker.attempt_id().clone(),
        reason: blocker.reason().to_owned(),
    }
}
