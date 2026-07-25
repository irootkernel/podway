use std::collections::{BTreeMap, BTreeSet};

use podway_core::{
    AddItemV1, ArtifactValueV1, AttachItemV1, AttemptId, AttemptInputV1, AttemptLifecycle,
    AttemptV1, BlockSessionV1, BlockerId, BlockerInputV1, BlockerState, BlockerV1, CheckItemV1,
    ClearItemV1, CommandContextV1, CompleteSessionV1, DomainError, ItemCommonV1, ItemId,
    ItemMutationPreconditionsV1, ItemSlotInputV1, ItemSlotV1, ItemSpecV1, ItemValueV1,
    LocalArtifactVerificationV1, ProcedureSnapshotAssemblyInputV1, ProcedureSnapshotId,
    ProcedureSnapshotV1, ProcedureSourceLabelV1, ProcedureWarningCodeV1, ReopenSessionV1,
    ResetAllWorkspaceV1, ResetSessionV1, RetrySessionV1, ReturnPolicyV1, ReturnSessionV1, Revision,
    SessionAggregateV1, SessionCommandV1, SessionId, SessionLifecycle, SetItemV1, Sha256Digest,
    SkipPolicyV1, SkipSessionV1, StageId, StageProgressState, StageSpecV1, StartReplaceSessionV1,
    StartSessionV1, TransitionEffectV1, UnblockSessionV1, UnixMillis, apply_transition_v1,
    preview_transition_v1,
};

const SEEDS: [u32; 8] = [1, 7, 19, 43, 97, 211, 509, 1_009];
const STAGE_COUNTS: [usize; 4] = [1, 2, 3, 4];
const STEPS_PER_SEED: usize = 48;
const MODEL_ITEM_IDS: [&str; 6] = ["confirm", "text", "choice", "integer", "list", "artifact"];

#[derive(Clone, Debug)]
struct Lcg(u32);

impl Lcg {
    const fn new(seed: u32) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0
    }
}

struct IdSource {
    next: u32,
}

impl IdSource {
    const fn new(seed: u32) -> Self {
        Self {
            next: 10_000 + seed * 100,
        }
    }

    fn next_attempt(&mut self) -> AttemptId {
        let value = self.next;
        self.next += 1;
        AttemptId::new(uuid(value)).unwrap()
    }

    fn next_blocker(&mut self) -> BlockerId {
        let value = self.next;
        self.next += 1;
        BlockerId::new(uuid(value)).unwrap()
    }

    fn next_session(&mut self) -> SessionId {
        let value = self.next;
        self.next += 1;
        SessionId::new(uuid(value)).unwrap()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ModelValue {
    Confirm,
    Text(String),
    Choice(String),
    Integer(i64),
    List(Vec<String>),
    Artifact(ArtifactValueV1),
}

impl ModelValue {
    fn as_item_value(&self) -> ItemValueV1 {
        match self {
            Self::Confirm => ItemValueV1::confirm(),
            Self::Text(value) => ItemValueV1::text(value.clone()),
            Self::Choice(value) => ItemValueV1::choice(value.clone()).unwrap(),
            Self::Integer(value) => ItemValueV1::integer(*value),
            Self::List(values) => ItemValueV1::list(values.clone()).unwrap(),
            Self::Artifact(value) => ItemValueV1::artifact(value.clone()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModelSlot {
    revision: u64,
    value: Option<ModelValue>,
    created_at: Option<u64>,
    updated_at: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModelBlocker {
    blocker_id: String,
    reason: String,
    state: BlockerState,
    created_at: u64,
    resolved_at: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModelAttempt {
    attempt_id: String,
    stage_index: usize,
    number: u32,
    lifecycle: AttemptLifecycle,
    started_at: u64,
    ended_at: Option<u64>,
    reason: Option<String>,
    slots: BTreeMap<String, ModelSlot>,
    blockers: Vec<ModelBlocker>,
}

impl ModelAttempt {
    fn fresh(attempt_id: &AttemptId, stage_index: usize, number: u32, now: u64) -> Self {
        Self {
            attempt_id: attempt_id.as_str().to_owned(),
            stage_index,
            number,
            lifecycle: AttemptLifecycle::Active,
            started_at: now,
            ended_at: None,
            reason: None,
            slots: MODEL_ITEM_IDS
                .iter()
                .map(|item_id| {
                    (
                        (*item_id).to_owned(),
                        ModelSlot {
                            revision: 0,
                            value: None,
                            created_at: None,
                            updated_at: None,
                        },
                    )
                })
                .collect(),
            blockers: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct ReferenceModel {
    session_id: String,
    lifecycle: SessionLifecycle,
    revision: u64,
    created_at: u64,
    completed_at: Option<u64>,
    cancelled_at: Option<u64>,
    cancel_reason: Option<String>,
    active_stage: Option<usize>,
    active_attempt_id: Option<String>,
    stage_states: Vec<StageProgressState>,
    latest_attempt_numbers: Vec<u32>,
    attempts: Vec<ModelAttempt>,
}

impl ReferenceModel {
    fn started(
        stage_count: usize,
        session_id: &SessionId,
        first_attempt_id: &AttemptId,
        now: u64,
    ) -> Self {
        let mut stage_states = vec![StageProgressState::Pending; stage_count];
        stage_states[0] = StageProgressState::Current;
        let mut latest_attempt_numbers = vec![0; stage_count];
        latest_attempt_numbers[0] = 1;
        Self {
            session_id: session_id.as_str().to_owned(),
            lifecycle: SessionLifecycle::Running,
            revision: 1,
            created_at: now,
            completed_at: None,
            cancelled_at: None,
            cancel_reason: None,
            active_stage: Some(0),
            active_attempt_id: Some(first_attempt_id.as_str().to_owned()),
            stage_states,
            latest_attempt_numbers,
            attempts: vec![ModelAttempt::fresh(first_attempt_id, 0, 1, now)],
        }
    }

    fn active_attempt(&self) -> &ModelAttempt {
        let active_attempt_id = self.active_attempt_id.as_ref().unwrap();
        self.attempts
            .iter()
            .find(|attempt| &attempt.attempt_id == active_attempt_id)
            .unwrap()
    }

    fn active_attempt_mut(&mut self) -> &mut ModelAttempt {
        let active_attempt_id = self.active_attempt_id.clone().unwrap();
        self.attempts
            .iter_mut()
            .find(|attempt| attempt.attempt_id == active_attempt_id)
            .unwrap()
    }

    fn attempt_by_stage_number(&self, stage_index: usize, number: u32) -> &ModelAttempt {
        self.attempts
            .iter()
            .find(|attempt| attempt.stage_index == stage_index && attempt.number == number)
            .unwrap()
    }

    fn latest_attempt_id(&self, stage_index: usize) -> Option<&str> {
        let number = self.latest_attempt_numbers[stage_index];
        (number > 0).then(|| {
            self.attempt_by_stage_number(stage_index, number)
                .attempt_id
                .as_str()
        })
    }

    fn latest_recorded_at(&self) -> u64 {
        let mut latest = self.created_at;
        for timestamp in self.completed_at.into_iter().chain(self.cancelled_at) {
            latest = latest.max(timestamp);
        }
        for attempt in &self.attempts {
            latest = latest.max(attempt.started_at);
            if let Some(ended_at) = attempt.ended_at {
                latest = latest.max(ended_at);
            }
            for slot in attempt.slots.values() {
                for timestamp in slot.created_at.into_iter().chain(slot.updated_at) {
                    latest = latest.max(timestamp);
                }
            }
            for blocker in &attempt.blockers {
                latest = latest.max(blocker.created_at);
                if let Some(resolved_at) = blocker.resolved_at {
                    latest = latest.max(resolved_at);
                }
            }
        }
        latest
    }

    fn list_values(&self) -> Vec<String> {
        match &self.active_attempt().slots.get("list").unwrap().value {
            Some(ModelValue::List(values)) => values.clone(),
            None => Vec::new(),
            value => panic!("unexpected list model value: {value:?}"),
        }
    }

    fn write(&mut self, item_id: &str, value: ModelValue, now: u64) {
        let next_revision = self.revision + 1;
        let changed = {
            let slot = self.active_attempt_mut().slots.get_mut(item_id).unwrap();
            if slot.value.as_ref() == Some(&value) {
                false
            } else {
                slot.value = Some(value);
                slot.revision += 1;
                slot.created_at = slot.created_at.or(Some(now));
                slot.updated_at = Some(now);
                true
            }
        };
        if changed {
            self.revision = next_revision;
        }
    }

    fn clear(&mut self, item_id: &str, now: u64) {
        let next_revision = self.revision + 1;
        let changed = {
            let slot = self.active_attempt_mut().slots.get_mut(item_id).unwrap();
            if slot.value.is_none() {
                false
            } else {
                slot.value = None;
                slot.revision += 1;
                slot.updated_at = Some(now);
                true
            }
        };
        if changed {
            self.revision = next_revision;
        }
    }

    fn check(&mut self, now: u64) {
        self.write("confirm", ModelValue::Confirm, now);
    }

    fn uncheck(&mut self, now: u64) {
        self.clear("confirm", now);
    }

    fn add(&mut self, value: String, now: u64) {
        let next_revision = self.revision + 1;
        {
            let slot = self.active_attempt_mut().slots.get_mut("list").unwrap();
            let mut values = match &slot.value {
                Some(ModelValue::List(values)) => values.clone(),
                None => Vec::new(),
                value => panic!("unexpected list model value: {value:?}"),
            };
            values.push(value);
            slot.value = Some(ModelValue::List(values));
            slot.revision += 1;
            slot.created_at = slot.created_at.or(Some(now));
            slot.updated_at = Some(now);
        }
        self.revision = next_revision;
    }

    fn remove(&mut self, value: &str, now: u64) {
        let next_revision = self.revision + 1;
        let changed = {
            let slot = self.active_attempt_mut().slots.get_mut("list").unwrap();
            let mut values = match &slot.value {
                Some(ModelValue::List(values)) => values.clone(),
                None => Vec::new(),
                value => panic!("unexpected list model value: {value:?}"),
            };
            if let Some(index) = values.iter().position(|entry| entry == value) {
                values.remove(index);
                slot.value = Some(ModelValue::List(values));
                slot.revision += 1;
                slot.updated_at = Some(now);
                true
            } else {
                false
            }
        };
        if changed {
            self.revision = next_revision;
        }
    }

    fn block(&mut self, blocker_id: &BlockerId, reason: &str, now: u64) {
        self.active_attempt_mut().blockers.push(ModelBlocker {
            blocker_id: blocker_id.as_str().to_owned(),
            reason: reason.to_owned(),
            state: BlockerState::Open,
            created_at: now,
            resolved_at: None,
        });
        self.revision += 1;
    }

    fn open_blocker_ids(&self) -> BTreeSet<String> {
        self.active_attempt_id
            .as_ref()
            .map(|active_attempt_id| {
                self.attempts
                    .iter()
                    .find(|attempt| &attempt.attempt_id == active_attempt_id)
                    .unwrap()
                    .blockers
                    .iter()
                    .filter(|blocker| blocker.state == BlockerState::Open)
                    .map(|blocker| blocker.blocker_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn unblock(&mut self, blocker_ids: &BTreeSet<String>, now: u64) {
        let mut changed = false;
        for blocker in &mut self.active_attempt_mut().blockers {
            if blocker_ids.contains(&blocker.blocker_id) {
                assert_eq!(blocker.state, BlockerState::Open);
                blocker.state = BlockerState::Resolved;
                blocker.resolved_at = Some(now);
                changed = true;
            }
        }
        assert!(changed);
        self.revision += 1;
    }

    fn close_active(
        &mut self,
        lifecycle: AttemptLifecycle,
        terminal_reason: Option<String>,
        now: u64,
    ) {
        let attempt = self.active_attempt_mut();
        attempt.lifecycle = lifecycle;
        attempt.reason = match lifecycle {
            AttemptLifecycle::Completed => attempt.reason.clone(),
            AttemptLifecycle::Skipped | AttemptLifecycle::Abandoned => {
                terminal_reason.or_else(|| attempt.reason.clone())
            }
            AttemptLifecycle::Active => unreachable!("an active attempt cannot be closed"),
        };
        attempt.ended_at = Some(now);
        if matches!(
            lifecycle,
            AttemptLifecycle::Skipped | AttemptLifecycle::Abandoned
        ) {
            for blocker in &mut attempt.blockers {
                if blocker.state == BlockerState::Open {
                    blocker.state = BlockerState::Resolved;
                    blocker.resolved_at = Some(now);
                }
            }
        }
    }

    fn fresh_attempt(&mut self, attempt_id: &AttemptId, stage_index: usize, now: u64) {
        self.latest_attempt_numbers[stage_index] += 1;
        let number = self.latest_attempt_numbers[stage_index];
        self.active_attempt_id = Some(attempt_id.as_str().to_owned());
        self.attempts
            .push(ModelAttempt::fresh(attempt_id, stage_index, number, now));
    }

    fn retry(&mut self, next_attempt_id: &AttemptId, reason: String, now: u64) {
        let stage = self.active_stage.unwrap();
        self.close_active(AttemptLifecycle::Abandoned, Some(reason), now);
        self.fresh_attempt(next_attempt_id, stage, now);
        self.revision += 1;
    }

    fn advance(
        &mut self,
        skipped: bool,
        reason: Option<String>,
        next_attempt_id: Option<&AttemptId>,
        now: u64,
    ) {
        let stage = self.active_stage.unwrap();
        self.close_active(
            if skipped {
                AttemptLifecycle::Skipped
            } else {
                AttemptLifecycle::Completed
            },
            reason,
            now,
        );
        self.stage_states[stage] = if skipped {
            StageProgressState::Skipped
        } else {
            StageProgressState::Done
        };
        self.revision += 1;
        if stage + 1 == self.stage_states.len() {
            self.lifecycle = SessionLifecycle::Completed;
            self.completed_at = Some(now);
            self.cancelled_at = None;
            self.cancel_reason = None;
            self.active_stage = None;
            self.active_attempt_id = None;
            return;
        }

        let next = stage + 1;
        self.stage_states[next] = StageProgressState::Current;
        self.active_stage = Some(next);
        self.fresh_attempt(next_attempt_id.unwrap(), next, now);
    }

    fn return_to(&mut self, destination: usize, attempt_id: &AttemptId, reason: String, now: u64) {
        let current = self.active_stage.unwrap();
        assert!(destination < current);
        self.close_active(AttemptLifecycle::Abandoned, Some(reason), now);
        let highest_reached = self
            .stage_states
            .iter()
            .rposition(|state| *state != StageProgressState::Pending)
            .unwrap();
        self.stage_states[destination] = StageProgressState::Current;
        for state in &mut self.stage_states[destination + 1..=highest_reached] {
            *state = StageProgressState::Redo;
        }
        self.active_stage = Some(destination);
        self.fresh_attempt(attempt_id, destination, now);
        self.revision += 1;
    }

    fn reopen(&mut self, destination: usize, attempt_id: &AttemptId, reason: String, now: u64) {
        assert_eq!(self.lifecycle, SessionLifecycle::Completed);
        assert!(!reason.trim().is_empty());
        self.lifecycle = SessionLifecycle::Running;
        self.completed_at = None;
        self.cancelled_at = None;
        self.cancel_reason = None;
        self.stage_states[destination] = StageProgressState::Current;
        for state in &mut self.stage_states[destination + 1..] {
            if *state != StageProgressState::Pending {
                *state = StageProgressState::Redo;
            }
        }
        self.active_stage = Some(destination);
        self.fresh_attempt(attempt_id, destination, now);
        self.active_attempt_mut().reason = Some(reason);
        self.revision += 1;
    }

    fn cancel(&mut self, reason: String, now: u64) {
        let stage = self.active_stage.unwrap();
        self.close_active(AttemptLifecycle::Abandoned, Some(reason.clone()), now);
        self.stage_states[stage] = StageProgressState::Abandoned;
        self.lifecycle = SessionLifecycle::Cancelled;
        self.completed_at = None;
        self.cancelled_at = Some(now);
        self.cancel_reason = Some(reason);
        self.active_stage = None;
        self.active_attempt_id = None;
        self.revision += 1;
    }

    fn start_replace(
        &mut self,
        stage_count: usize,
        session_id: &SessionId,
        first_attempt_id: &AttemptId,
        now: u64,
    ) {
        *self = Self::started(stage_count, session_id, first_attempt_id, now);
    }

    fn assert_matches(&self, session: &SessionAggregateV1) {
        assert_eq!(session.session_id().as_str(), self.session_id);
        assert_eq!(session.lifecycle(), self.lifecycle);
        assert_eq!(session.revision(), Revision::new(self.revision));
        assert_eq!(session.created_at(), UnixMillis::new(self.created_at));
        assert_eq!(
            session.completed_at(),
            self.completed_at.map(UnixMillis::new)
        );
        assert_eq!(
            session.cancelled_at(),
            self.cancelled_at.map(UnixMillis::new)
        );
        assert_eq!(session.cancel_reason(), self.cancel_reason.as_deref());
        assert_eq!(
            session.latest_recorded_at(),
            UnixMillis::new(self.latest_recorded_at())
        );
        assert_eq!(
            session
                .stage_progress()
                .iter()
                .map(|progress| progress.state())
                .collect::<Vec<_>>(),
            self.stage_states
        );
        assert_eq!(
            session
                .stage_progress()
                .iter()
                .map(|progress| progress.latest_attempt_number())
                .collect::<Vec<_>>(),
            self.latest_attempt_numbers
        );
        for (stage_index, progress) in session.stage_progress().iter().enumerate() {
            assert_eq!(
                progress.latest_attempt_id().map(AttemptId::as_str),
                self.latest_attempt_id(stage_index)
            );
        }
        assert_eq!(
            session.active_stage_id(),
            self.active_stage
                .map(|index| session.snapshot().stages()[index].id())
        );
        assert_eq!(
            session.active_attempt_id().map(AttemptId::as_str),
            self.active_attempt_id.as_deref()
        );
        assert_eq!(session.attempts().len(), self.attempts.len());

        let mut expected_attempts = self.attempts.iter().collect::<Vec<_>>();
        expected_attempts.sort_by_key(|attempt| (attempt.stage_index, attempt.number));
        assert_eq!(
            session
                .attempts()
                .iter()
                .map(|attempt| attempt.attempt_id().as_str())
                .collect::<Vec<_>>(),
            expected_attempts
                .iter()
                .map(|attempt| attempt.attempt_id.as_str())
                .collect::<Vec<_>>()
        );

        for expected in &self.attempts {
            let actual = session
                .attempts()
                .iter()
                .find(|attempt| attempt.attempt_id().as_str() == expected.attempt_id.as_str())
                .unwrap();
            assert_eq!(
                actual.stage_id(),
                session.snapshot().stages()[expected.stage_index].id()
            );
            assert_eq!(actual.number(), expected.number);
            assert_eq!(actual.lifecycle(), expected.lifecycle);
            assert_eq!(actual.started_at(), UnixMillis::new(expected.started_at));
            assert_eq!(actual.ended_at(), expected.ended_at.map(UnixMillis::new));
            assert_eq!(actual.reason(), expected.reason.as_deref());
            assert_eq!(actual.item_slots().len(), expected.slots.len());

            for (item_id, expected_slot) in &expected.slots {
                let actual_slot = actual
                    .item_slots()
                    .iter()
                    .find(|slot| slot.item_id().as_str() == item_id)
                    .unwrap();
                let expected_value = expected_slot.value.as_ref().map(ModelValue::as_item_value);
                assert_eq!(
                    actual_slot.revision(),
                    Revision::new(expected_slot.revision)
                );
                assert_eq!(actual_slot.value(), expected_value.as_ref());
                assert_eq!(
                    actual_slot.created_at(),
                    expected_slot.created_at.map(UnixMillis::new)
                );
                assert_eq!(
                    actual_slot.updated_at(),
                    expected_slot.updated_at.map(UnixMillis::new)
                );
            }

            assert_eq!(actual.blockers().len(), expected.blockers.len());
            for expected_blocker in &expected.blockers {
                let actual_blocker = actual
                    .blockers()
                    .iter()
                    .find(|blocker| {
                        blocker.blocker_id().as_str() == expected_blocker.blocker_id.as_str()
                    })
                    .unwrap();
                assert_eq!(actual_blocker.attempt_id(), actual.attempt_id());
                assert_eq!(actual_blocker.reason(), expected_blocker.reason);
                assert_eq!(actual_blocker.state(), expected_blocker.state);
                assert_eq!(
                    actual_blocker.created_at(),
                    UnixMillis::new(expected_blocker.created_at)
                );
                assert_eq!(
                    actual_blocker.resolved_at(),
                    expected_blocker.resolved_at.map(UnixMillis::new)
                );
            }
        }

        let actual_open_blocker_ids: BTreeSet<String> = session
            .active_attempt_id()
            .map(|_| {
                active_attempt(session)
                    .blockers()
                    .iter()
                    .filter(|blocker| blocker.state() == BlockerState::Open)
                    .map(|blocker| blocker.blocker_id().as_str().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(actual_open_blocker_ids, self.open_blocker_ids());
    }
}

fn uuid(value: u32) -> String {
    format!("123e4567-e89b-12d3-a456-{value:012x}")
}

fn digest() -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap()
}

fn item_id(value: &str) -> ItemId {
    ItemId::new(value).unwrap()
}

fn stage_id(value: &str) -> StageId {
    StageId::new(value).unwrap()
}

fn common(id: &str) -> ItemCommonV1 {
    common_with_required(id, false)
}

fn common_with_required(id: &str, required: bool) -> ItemCommonV1 {
    ItemCommonV1::new(item_id(id), format!("Prompt for {id}"), None, required).unwrap()
}

fn generated_snapshot(seed: u32, stage_count: usize) -> ProcedureSnapshotV1 {
    generated_snapshot_with_required_artifact(seed, stage_count, false)
}

fn generated_snapshot_with_required_artifact(
    seed: u32,
    stage_count: usize,
    artifact_required: bool,
) -> ProcedureSnapshotV1 {
    let stages = (0..stage_count)
        .map(|index| {
            StageSpecV1::new(
                stage_id(&format!("stage{index}")),
                format!("Stage {index}"),
                Vec::new(),
                vec![
                    ItemSpecV1::confirm(common("confirm")),
                    ItemSpecV1::text(common("text"), 0, 32, true).unwrap(),
                    ItemSpecV1::choice(
                        common("choice"),
                        vec!["left".to_owned(), "right".to_owned()],
                    )
                    .unwrap(),
                    ItemSpecV1::integer(common("integer"), Some(-10), Some(10)).unwrap(),
                    ItemSpecV1::list(common("list"), 0, 4, 20, false).unwrap(),
                    ItemSpecV1::artifact(
                        common_with_required("artifact", artifact_required),
                        vec!["text/plain".to_owned()],
                    )
                    .unwrap(),
                ],
                SkipPolicyV1::allowed(true),
            )
            .unwrap()
        })
        .collect();
    let accepted_warning_codes = match (stage_count, artifact_required) {
        (1, false) => vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::FinalStageSkippable,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
        (1, true) => vec![
            ProcedureWarningCodeV1::FinalStageSkippable,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
        (_, false) => vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::FinalStageSkippable,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
            ProcedureWarningCodeV1::RepeatedPrompt,
        ],
        (_, true) => vec![
            ProcedureWarningCodeV1::FinalStageSkippable,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
            ProcedureWarningCodeV1::RepeatedPrompt,
        ],
    };
    ProcedureSnapshotV1::assemble(ProcedureSnapshotAssemblyInputV1 {
        snapshot_id: ProcedureSnapshotId::new(uuid(seed)).unwrap(),
        procedure_id: format!("property-{seed}-{stage_count}"),
        procedure_version: "1".to_owned(),
        name: "Property procedure".to_owned(),
        description: None,
        stages,
        return_policy: ReturnPolicyV1::any_previous(),
        source_label: ProcedureSourceLabelV1::new("test").unwrap(),
        accepted_warning_codes,
        created_at: UnixMillis::new(1),
    })
    .unwrap()
}

fn context(session: &SessionAggregateV1, now: u64) -> CommandContextV1 {
    CommandContextV1 {
        expected_revision: session.revision(),
        now: UnixMillis::new(now),
    }
}

fn active_attempt(session: &SessionAggregateV1) -> &podway_core::AttemptV1 {
    session
        .attempts()
        .iter()
        .find(|attempt| Some(attempt.attempt_id()) == session.active_attempt_id())
        .unwrap()
}
fn attempt_by_id<'a>(
    session: &'a SessionAggregateV1,
    attempt_id: &AttemptId,
) -> &'a podway_core::AttemptV1 {
    session
        .attempts()
        .iter()
        .find(|attempt| attempt.attempt_id() == attempt_id)
        .unwrap()
}

fn attempt_by_stage_number(
    session: &SessionAggregateV1,
    stage_index: usize,
    number: u32,
) -> &podway_core::AttemptV1 {
    let stage_id = session.snapshot().stages()[stage_index].id();
    session
        .attempts()
        .iter()
        .find(|attempt| attempt.stage_id() == stage_id && attempt.number() == number)
        .unwrap()
}

fn item_preconditions(session: &SessionAggregateV1, id: &str) -> ItemMutationPreconditionsV1 {
    let attempt = active_attempt(session);
    let slot = attempt
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id(id))
        .unwrap();
    ItemMutationPreconditionsV1 {
        expected_attempt_id: attempt.attempt_id().clone(),
        expected_item_revision: slot.revision(),
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
    applied.unwrap_or_else(|error| panic!("command {command:?} failed unexpectedly: {error}"))
}

fn apply_next(
    session: &SessionAggregateV1,
    command: SessionCommandV1,
    now: u64,
) -> (podway_core::TransitionOutcomeV1, SessionAggregateV1) {
    let outcome = apply_equivalent(Some(session), &command, context(session, now));
    let next = outcome.next_aggregate().unwrap().clone();
    (outcome, next)
}

fn assert_outcome_revision(
    prior: &SessionAggregateV1,
    outcome: &podway_core::TransitionOutcomeV1,
    resets_session: bool,
) {
    assert_eq!(outcome.revision_before(), Some(prior.revision()));
    let expected = if resets_session {
        Revision::new(1)
    } else if outcome.changed() {
        prior.revision().checked_next().unwrap()
    } else {
        prior.revision()
    };
    assert_eq!(outcome.revision_after(), Some(expected));
}

fn assert_fresh_active_attempt(session: &SessionAggregateV1) {
    let attempt = active_attempt(session);
    assert_eq!(attempt.lifecycle(), AttemptLifecycle::Active);
    assert_eq!(attempt.ended_at(), None);
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
    assert!(
        attempt
            .item_slots()
            .iter()
            .all(|slot| slot.created_at().is_none() && slot.updated_at().is_none())
    );
}

fn assert_invariants(
    session: &SessionAggregateV1,
    seen_slot_revisions: &mut BTreeMap<(String, String), Revision>,
) {
    session.validate().unwrap();
    assert!(session.revision() > Revision::ZERO);
    let latest_recorded_at = session.latest_recorded_at();
    assert!(latest_recorded_at >= session.created_at());
    assert_eq!(
        session.stage_progress().len(),
        session.snapshot().stages().len()
    );

    for (index, (progress, stage)) in session
        .stage_progress()
        .iter()
        .zip(session.snapshot().stages())
        .enumerate()
    {
        assert_eq!(progress.stage_index(), index);
        assert_eq!(progress.stage_id(), stage.id());
        match progress.latest_attempt_id() {
            Some(attempt_id) => {
                assert!(progress.latest_attempt_number() > 0);
                let latest_attempt =
                    attempt_by_stage_number(session, index, progress.latest_attempt_number());
                assert_eq!(latest_attempt.attempt_id(), attempt_id);
                assert_eq!(attempt_by_id(session, attempt_id), latest_attempt);
            }
            None => assert_eq!(progress.latest_attempt_number(), 0),
        }
    }

    let active_attempts = session
        .attempts()
        .iter()
        .filter(|attempt| attempt.lifecycle() == AttemptLifecycle::Active)
        .collect::<Vec<_>>();
    match session.lifecycle() {
        SessionLifecycle::Running => {
            assert_eq!(active_attempts.len(), 1);
            assert_eq!(
                active_attempt(session).attempt_id(),
                session.active_attempt_id().unwrap()
            );
            assert!(session.active_stage_id().is_some());
            assert_eq!(
                session
                    .stage_progress()
                    .iter()
                    .filter(|progress| progress.state() == StageProgressState::Current)
                    .count(),
                1
            );
        }
        SessionLifecycle::Completed | SessionLifecycle::Cancelled => {
            assert!(active_attempts.is_empty());
            assert!(session.active_attempt_id().is_none());
            assert!(session.active_stage_id().is_none());
        }
    }

    let mut previous_attempt_key = None;
    for attempt in session.attempts() {
        let stage_index = session
            .snapshot()
            .stages()
            .iter()
            .position(|stage| stage.id() == attempt.stage_id())
            .unwrap();
        let attempt_key = (stage_index, attempt.number());
        if let Some(previous) = previous_attempt_key {
            assert!(attempt_key > previous);
        }
        previous_attempt_key = Some(attempt_key);
        for slot in attempt.item_slots() {
            let key = (
                attempt.attempt_id().as_str().to_owned(),
                slot.item_id().as_str().to_owned(),
            );
            if let Some(previous) = seen_slot_revisions.insert(key, slot.revision()) {
                assert!(slot.revision() >= previous);
            }
        }
        assert!(attempt.started_at() <= latest_recorded_at);
        if let Some(ended_at) = attempt.ended_at() {
            assert!(ended_at <= latest_recorded_at);
        }
        for blocker in attempt.blockers() {
            assert!(blocker.created_at() <= latest_recorded_at);
            if let Some(resolved_at) = blocker.resolved_at() {
                assert!(resolved_at >= blocker.created_at());
                assert!(resolved_at <= latest_recorded_at);
            }
        }
    }
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

#[derive(Default)]
struct OperationHits {
    start: usize,
    start_replace: usize,
    check: usize,
    uncheck: usize,
    set: usize,
    add: usize,
    remove: usize,
    attach: usize,
    clear: usize,
    complete_non_final: usize,
    complete_final: usize,
    skip_non_final: usize,
    skip_final: usize,
    retry: usize,
    return_to: usize,
    block: usize,
    unblock_targeted: usize,
    unblock_all: usize,
    cancel: usize,
    reopen: usize,
    reset: usize,
    reset_all: usize,
    no_op: usize,
}

impl OperationHits {
    fn record_start(&mut self) {
        self.start += 1;
    }

    fn record(&mut self, command: &SessionCommandV1, outcome: &podway_core::TransitionOutcomeV1) {
        match command {
            SessionCommandV1::Start(_) => self.start += 1,
            SessionCommandV1::StartReplace(_) => self.start_replace += 1,
            SessionCommandV1::Check(_) => self.check += 1,
            SessionCommandV1::Uncheck(_) => self.uncheck += 1,
            SessionCommandV1::Set(_) => self.set += 1,
            SessionCommandV1::Add(_) => self.add += 1,
            SessionCommandV1::Remove(_) => self.remove += 1,
            SessionCommandV1::Attach(_) => self.attach += 1,
            SessionCommandV1::Clear(_) => self.clear += 1,
            SessionCommandV1::Complete(input) => {
                if input.next_attempt_id.is_some() {
                    self.complete_non_final += 1;
                } else {
                    self.complete_final += 1;
                }
            }
            SessionCommandV1::Skip(input) => {
                if input.next_attempt_id.is_some() {
                    self.skip_non_final += 1;
                } else {
                    self.skip_final += 1;
                }
            }
            SessionCommandV1::Retry(_) => self.retry += 1,
            SessionCommandV1::Return(_) => self.return_to += 1,
            SessionCommandV1::Block(_) => self.block += 1,
            SessionCommandV1::Unblock(input) => {
                if input.unblock_all {
                    self.unblock_all += 1;
                } else {
                    self.unblock_targeted += 1;
                }
            }
            SessionCommandV1::Cancel(_) => self.cancel += 1,
            SessionCommandV1::Reopen(_) => self.reopen += 1,
            SessionCommandV1::Reset(_) => self.reset += 1,
            SessionCommandV1::ResetAll(_) => self.reset_all += 1,
        }
        if !outcome.changed() {
            self.no_op += 1;
        }
    }

    fn assert_all_covered(&self, source: &str) {
        assert!(self.start > 0, "{source}: start was not exercised");
        assert!(
            self.start_replace > 0,
            "{source}: start-replace was not exercised"
        );
        assert!(self.check > 0, "{source}: check was not exercised");
        assert!(self.uncheck > 0, "{source}: uncheck was not exercised");
        assert!(self.set > 0, "{source}: set was not exercised");
        assert!(self.add > 0, "{source}: add was not exercised");
        assert!(self.remove > 0, "{source}: remove was not exercised");
        assert!(self.attach > 0, "{source}: attach was not exercised");
        assert!(self.clear > 0, "{source}: clear was not exercised");
        assert!(
            self.complete_non_final > 0,
            "{source}: non-final completion was not exercised"
        );
        assert!(
            self.complete_final > 0,
            "{source}: final completion was not exercised"
        );
        assert!(
            self.skip_non_final > 0,
            "{source}: non-final skip was not exercised"
        );
        assert!(
            self.skip_final > 0,
            "{source}: final skip was not exercised"
        );
        assert!(self.retry > 0, "{source}: retry was not exercised");
        assert!(self.return_to > 0, "{source}: return was not exercised");
        assert!(self.block > 0, "{source}: block was not exercised");
        assert!(
            self.unblock_targeted > 0,
            "{source}: targeted unblock was not exercised"
        );
        assert!(
            self.unblock_all > 0,
            "{source}: unblock-all was not exercised"
        );
        assert!(self.cancel > 0, "{source}: cancel was not exercised");
        assert!(self.reopen > 0, "{source}: reopen was not exercised");
        assert!(self.reset > 0, "{source}: reset was not exercised");
        assert!(self.reset_all > 0, "{source}: reset-all was not exercised");
        assert!(self.no_op > 0, "{source}: no-op command was not exercised");
    }
}

fn apply_counted(
    session: &SessionAggregateV1,
    command: SessionCommandV1,
    now: u64,
    hits: &mut OperationHits,
) -> SessionAggregateV1 {
    let (outcome, next) = apply_next(session, command.clone(), now);
    assert_outcome_revision(
        session,
        &outcome,
        matches!(&command, SessionCommandV1::StartReplace(_)),
    );
    assert_eq!(
        next.latest_recorded_at(),
        if outcome.changed() {
            UnixMillis::new(now)
        } else {
            session.latest_recorded_at()
        }
    );
    if !outcome.changed() {
        assert_eq!(&next, session);
    }
    hits.record(&command, &outcome);
    next
}

fn exercise_operation_coverage_prelude(hits: &mut OperationHits) {
    let procedure = generated_snapshot(50_000, 3);
    let mut ids = IdSource::new(50_000);
    let start = StartSessionV1 {
        task_title: "Coverage prelude".to_owned(),
        snapshot: procedure.clone(),
        session_id: ids.next_session(),
        first_attempt_id: ids.next_attempt(),
    };
    let started = apply_equivalent(
        None,
        &SessionCommandV1::Start(start),
        CommandContextV1 {
            expected_revision: Revision::ZERO,
            now: UnixMillis::new(10),
        },
    );
    hits.record_start();
    let mut session = started.next_aggregate().unwrap().clone();

    session = apply_counted(
        &session,
        SessionCommandV1::Check(CheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: item_preconditions(&session, "confirm"),
        }),
        11,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Check(CheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: item_preconditions(&session, "confirm"),
        }),
        12,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Uncheck(podway_core::UncheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: item_preconditions(&session, "confirm"),
        }),
        13,
        hits,
    );
    let text = ItemValueV1::text("coverage");
    session = apply_counted(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("text"),
            value: text.clone(),
            preconditions: item_preconditions(&session, "text"),
        }),
        14,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("text"),
            value: text,
            preconditions: item_preconditions(&session, "text"),
        }),
        15,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("choice"),
            value: ItemValueV1::choice("left").unwrap(),
            preconditions: item_preconditions(&session, "choice"),
        }),
        16,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("integer"),
            value: ItemValueV1::integer(1),
            preconditions: item_preconditions(&session, "integer"),
        }),
        17,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Add(AddItemV1 {
            item_id: item_id("list"),
            value: "coverage".to_owned(),
            preconditions: item_preconditions(&session, "list"),
        }),
        18,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Remove(podway_core::RemoveItemV1 {
            item_id: item_id("list"),
            value: "coverage".to_owned(),
            ignore_missing: true,
            preconditions: item_preconditions(&session, "list"),
        }),
        19,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Remove(podway_core::RemoveItemV1 {
            item_id: item_id("list"),
            value: "missing".to_owned(),
            ignore_missing: true,
            preconditions: item_preconditions(&session, "list"),
        }),
        20,
        hits,
    );
    let artifact =
        ArtifactValueV1::external_reference("artifact:coverage", digest(), 1, "text/plain")
            .unwrap();
    session = apply_counted(
        &session,
        SessionCommandV1::Attach(AttachItemV1 {
            item_id: item_id("artifact"),
            value: artifact.clone(),
            preconditions: item_preconditions(&session, "artifact"),
        }),
        21,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Attach(AttachItemV1 {
            item_id: item_id("artifact"),
            value: artifact,
            preconditions: item_preconditions(&session, "artifact"),
        }),
        22,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Clear(ClearItemV1 {
            item_id: item_id("text"),
            preconditions: item_preconditions(&session, "text"),
        }),
        23,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Clear(ClearItemV1 {
            item_id: item_id("text"),
            preconditions: item_preconditions(&session, "text"),
        }),
        24,
        hits,
    );

    let first_blocker = ids.next_blocker();
    session = apply_counted(
        &session,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            blocker_id: first_blocker.clone(),
            reason: "targeted unblock".to_owned(),
        }),
        25,
        hits,
    );
    let first_blocker_record = active_attempt(&session)
        .blockers()
        .iter()
        .find(|blocker| blocker.blocker_id() == &first_blocker)
        .unwrap();
    assert_eq!(first_blocker_record.created_at(), UnixMillis::new(25));
    assert_eq!(first_blocker_record.resolved_at(), None);
    session = apply_counted(
        &session,
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            blocker_id: Some(first_blocker.clone()),
            unblock_all: false,
        }),
        26,
        hits,
    );
    let first_blocker_record = active_attempt(&session)
        .blockers()
        .iter()
        .find(|blocker| blocker.blocker_id() == &first_blocker)
        .unwrap();
    assert_eq!(
        first_blocker_record.resolved_at(),
        Some(UnixMillis::new(26))
    );
    for now in [27, 28] {
        session = apply_counted(
            &session,
            SessionCommandV1::Block(BlockSessionV1 {
                expected_attempt_id: active_attempt(&session).attempt_id().clone(),
                blocker_id: ids.next_blocker(),
                reason: format!("unblock all {now}"),
            }),
            now,
            hits,
        );
    }
    session = apply_counted(
        &session,
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            blocker_id: None,
            unblock_all: true,
        }),
        29,
        hits,
    );
    assert!(
        active_attempt(&session)
            .blockers()
            .iter()
            .all(|blocker| blocker.resolved_at().is_some())
    );
    assert!(
        active_attempt(&session)
            .blockers()
            .iter()
            .filter(|blocker| blocker.blocker_id() != &first_blocker)
            .all(|blocker| blocker.resolved_at() == Some(UnixMillis::new(29)))
    );
    let terminal_blocker = ids.next_blocker();
    session = apply_counted(
        &session,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            blocker_id: terminal_blocker.clone(),
            reason: "terminal resolution".to_owned(),
        }),
        30,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Retry(RetrySessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            reason: "coverage retry".to_owned(),
            next_attempt_id: ids.next_attempt(),
        }),
        31,
        hits,
    );
    let terminal_blocker_record = session
        .attempts()
        .iter()
        .flat_map(|attempt| attempt.blockers())
        .find(|blocker| blocker.blocker_id() == &terminal_blocker)
        .unwrap();
    assert_eq!(
        terminal_blocker_record.resolved_at(),
        Some(UnixMillis::new(31))
    );
    let cancelled_session = apply_counted(
        &session,
        SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            reason: "coverage cancel".to_owned(),
        }),
        32,
        hits,
    );
    let replacement_session_id = ids.next_session();
    let replacement = SessionCommandV1::StartReplace(StartReplaceSessionV1 {
        expected_session_id: session.session_id().clone(),
        confirmed: true,
        start: StartSessionV1 {
            task_title: "Coverage replacement".to_owned(),
            snapshot: procedure,
            session_id: replacement_session_id,
            first_attempt_id: ids.next_attempt(),
        },
    });
    session = apply_counted(&cancelled_session, replacement, 32, hits);

    session = apply_counted(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: Some(ids.next_attempt()),
            local_artifact_verifications: Vec::new(),
        }),
        33,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Return(ReturnSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            destination_stage_id: stage_id("stage0"),
            reason: "coverage return".to_owned(),
            destination_attempt_id: ids.next_attempt(),
        }),
        34,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            reason: Some("coverage skip".to_owned()),
            next_attempt_id: Some(ids.next_attempt()),
        }),
        35,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            reason: Some("coverage skip".to_owned()),
            next_attempt_id: Some(ids.next_attempt()),
        }),
        36,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            reason: Some("coverage final skip".to_owned()),
            next_attempt_id: None,
        }),
        37,
        hits,
    );
    let reopened_attempt_id = ids.next_attempt();
    session = apply_counted(
        &session,
        SessionCommandV1::Reopen(ReopenSessionV1 {
            expected_session_id: session.session_id().clone(),
            destination_stage_id: stage_id("stage0"),
            reason: "coverage reopen".to_owned(),
            destination_attempt_id: reopened_attempt_id.clone(),
        }),
        38,
        hits,
    );
    assert_eq!(active_attempt(&session).reason(), Some("coverage reopen"));
    session = apply_counted(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: Some(ids.next_attempt()),
            local_artifact_verifications: Vec::new(),
        }),
        39,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: Some(ids.next_attempt()),
            local_artifact_verifications: Vec::new(),
        }),
        40,
        hits,
    );
    session = apply_counted(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        41,
        hits,
    );
    let reopened_attempt = attempt_by_id(&session, &reopened_attempt_id);
    assert_eq!(reopened_attempt.reason(), Some("coverage reopen"));
    assert_eq!(session.lifecycle(), SessionLifecycle::Completed);
    let original_session = session.clone();
    let reset = SessionCommandV1::Reset(ResetSessionV1 {
        expected_session_id: session.session_id().clone(),
        confirmed: true,
    });
    let reset_context = context(&session, 42);
    assert!(reset_context.now > session.latest_recorded_at());
    let reset_preview = preview_transition_v1(Some(&session), &reset, reset_context);
    let reset_applied = apply_transition_v1(Some(&session), &reset, reset_context);
    assert_eq!(reset_preview, reset_applied);
    let reset_outcome = reset_applied.unwrap();
    assert!(reset_outcome.changed());
    assert_eq!(reset_outcome.revision_before(), Some(session.revision()));
    assert_eq!(reset_outcome.revision_after(), None);
    assert_eq!(
        reset_outcome.affected_stages().len(),
        session.stage_progress().len()
    );
    assert!(reset_outcome.next_aggregate().is_none());
    assert_eq!(session, original_session);
    hits.record(&reset, &reset_outcome);

    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::Reset(ResetSessionV1 {
            expected_session_id: session.session_id().clone(),
            confirmed: false,
        }),
        context(&session, 42),
        DomainError::InvalidState {
            reason: "explicit confirmation is required",
        },
    );
    let stale_session_id = ids.next_session();
    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::Reset(ResetSessionV1 {
            expected_session_id: stale_session_id.clone(),
            confirmed: true,
        }),
        context(&session, 42),
        DomainError::SessionIdentityMismatch {
            expected: stale_session_id,
            actual: Some(session.session_id().clone()),
        },
    );
    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::Reset(ResetSessionV1 {
            expected_session_id: session.session_id().clone(),
            confirmed: true,
        }),
        context(&session, 40),
        DomainError::InvalidState {
            reason: "transition timestamp precedes the latest retained timestamp",
        },
    );

    let reset_all = SessionCommandV1::ResetAll(ResetAllWorkspaceV1 {
        workspace_id: None,
        confirmed: true,
    });
    let reset_all_context = context(&session, 42);
    let reset_all_preview = preview_transition_v1(Some(&session), &reset_all, reset_all_context);
    let reset_all_applied = apply_transition_v1(Some(&session), &reset_all, reset_all_context);
    assert_eq!(reset_all_preview, reset_all_applied);
    let reset_all_outcome = reset_all_applied.unwrap();
    assert!(reset_all_outcome.changed());
    assert_eq!(
        reset_all_outcome.revision_before(),
        Some(session.revision())
    );
    assert_eq!(reset_all_outcome.revision_after(), None);
    assert!(reset_all_outcome.next_aggregate().is_none());
    assert_eq!(
        reset_all_outcome.effect(),
        Some(&TransitionEffectV1::WorkspaceResetAll { workspace_id: None })
    );
    assert_eq!(session, original_session);
    hits.record(&reset_all, &reset_all_outcome);
    let reset_all_without_session = SessionCommandV1::ResetAll(ResetAllWorkspaceV1 {
        workspace_id: None,
        confirmed: true,
    });
    let empty_workspace_context = CommandContextV1 {
        expected_revision: Revision::ZERO,
        now: UnixMillis::new(42),
    };
    let reset_all_without_session_preview =
        preview_transition_v1(None, &reset_all_without_session, empty_workspace_context);
    let reset_all_without_session_applied =
        apply_transition_v1(None, &reset_all_without_session, empty_workspace_context);
    assert_eq!(
        reset_all_without_session_preview,
        reset_all_without_session_applied
    );
    let reset_all_without_session_outcome = reset_all_without_session_applied.unwrap();
    assert!(reset_all_without_session_outcome.changed());
    assert_eq!(reset_all_without_session_outcome.revision_before(), None);
    assert_eq!(reset_all_without_session_outcome.revision_after(), None);
    assert!(reset_all_without_session_outcome.next_aggregate().is_none());
    hits.record(
        &reset_all_without_session,
        &reset_all_without_session_outcome,
    );

    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::ResetAll(ResetAllWorkspaceV1 {
            workspace_id: None,
            confirmed: false,
        }),
        context(&session, 42),
        DomainError::InvalidState {
            reason: "explicit confirmation is required",
        },
    );
    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::ResetAll(ResetAllWorkspaceV1 {
            workspace_id: None,
            confirmed: true,
        }),
        context(&session, 40),
        DomainError::InvalidState {
            reason: "transition timestamp precedes the latest retained timestamp",
        },
    );
}

#[test]
fn seeded_command_sequences_match_the_reference_model_and_preserve_invariants() {
    let mut scripted_hits = OperationHits::default();
    exercise_operation_coverage_prelude(&mut scripted_hits);
    scripted_hits.assert_all_covered("scripted prelude");

    let mut model_hits = OperationHits::default();
    for stage_count in STAGE_COUNTS {
        for seed in SEEDS {
            let procedure = generated_snapshot(seed, stage_count);
            let mut ids = IdSource::new(seed * 10 + stage_count as u32);
            let mut rng = Lcg::new(seed ^ (stage_count as u32).wrapping_mul(0xa5a5_5a5a));
            let start = StartSessionV1 {
                task_title: format!("Task {seed}-{stage_count}"),
                snapshot: procedure.clone(),
                session_id: ids.next_session(),
                first_attempt_id: ids.next_attempt(),
            };
            let mut model = ReferenceModel::started(
                stage_count,
                &start.session_id,
                &start.first_attempt_id,
                10,
            );
            let started = apply_equivalent(
                None,
                &SessionCommandV1::Start(start),
                CommandContextV1 {
                    expected_revision: Revision::ZERO,
                    now: UnixMillis::new(10),
                },
            );
            model_hits.record_start();
            let mut session = started.next_aggregate().unwrap().clone();
            let mut seen_slot_revisions = BTreeMap::new();
            assert_invariants(&session, &mut seen_slot_revisions);
            model.assert_matches(&session);
            assert_fresh_active_attempt(&session);

            for step in 0..STEPS_PER_SEED {
                let now = 11 + (step as u64 * 2);
                let prior = session.clone();
                let command;
                let mut fresh_attempt = false;

                match model.lifecycle {
                    SessionLifecycle::Running => {
                        let stage = model.active_stage.unwrap();
                        match rng.next() % 19 {
                            0 => {
                                command = SessionCommandV1::Check(CheckItemV1 {
                                    item_id: item_id("confirm"),
                                    preconditions: item_preconditions(&session, "confirm"),
                                });
                                model.check(now);
                            }
                            1 => {
                                command = SessionCommandV1::Uncheck(podway_core::UncheckItemV1 {
                                    item_id: item_id("confirm"),
                                    preconditions: item_preconditions(&session, "confirm"),
                                });
                                model.uncheck(now);
                            }
                            2 => {
                                let value = format!("text-{seed}-{stage_count}-{step}");
                                command = SessionCommandV1::Set(SetItemV1 {
                                    item_id: item_id("text"),
                                    value: ItemValueV1::text(value.clone()),
                                    preconditions: item_preconditions(&session, "text"),
                                });
                                model.write("text", ModelValue::Text(value), now);
                            }
                            3 => {
                                let value = if rng.next() & 1 == 0 {
                                    "left".to_owned()
                                } else {
                                    "right".to_owned()
                                };
                                command = SessionCommandV1::Set(SetItemV1 {
                                    item_id: item_id("choice"),
                                    value: ItemValueV1::choice(value.clone()).unwrap(),
                                    preconditions: item_preconditions(&session, "choice"),
                                });
                                model.write("choice", ModelValue::Choice(value), now);
                            }
                            4 => {
                                let value = (rng.next() % 21) as i64 - 10;
                                command = SessionCommandV1::Set(SetItemV1 {
                                    item_id: item_id("integer"),
                                    value: ItemValueV1::integer(value),
                                    preconditions: item_preconditions(&session, "integer"),
                                });
                                model.write("integer", ModelValue::Integer(value), now);
                            }
                            5 if model.list_values().len() < 4 => {
                                let value = format!("value-{seed}-{stage_count}-{step}");
                                command = SessionCommandV1::Add(AddItemV1 {
                                    item_id: item_id("list"),
                                    value: value.clone(),
                                    preconditions: item_preconditions(&session, "list"),
                                });
                                model.add(value, now);
                            }
                            6 => {
                                let value = model
                                    .list_values()
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| "missing".to_owned());
                                command = SessionCommandV1::Remove(podway_core::RemoveItemV1 {
                                    item_id: item_id("list"),
                                    value: value.clone(),
                                    ignore_missing: true,
                                    preconditions: item_preconditions(&session, "list"),
                                });
                                model.remove(&value, now);
                            }
                            7 => {
                                let value = ArtifactValueV1::external_reference(
                                    format!("artifact:{seed}:{stage_count}:{step}"),
                                    digest(),
                                    step as u64 + 1,
                                    "text/plain",
                                )
                                .unwrap();
                                command = SessionCommandV1::Attach(AttachItemV1 {
                                    item_id: item_id("artifact"),
                                    value: value.clone(),
                                    preconditions: item_preconditions(&session, "artifact"),
                                });
                                model.write("artifact", ModelValue::Artifact(value), now);
                            }
                            8 => {
                                command = SessionCommandV1::Clear(ClearItemV1 {
                                    item_id: item_id("text"),
                                    preconditions: item_preconditions(&session, "text"),
                                });
                                model.clear("text", now);
                            }
                            9 => {
                                let blocker_id = ids.next_blocker();
                                let reason = format!("waiting-{seed}-{step}");
                                command = SessionCommandV1::Block(BlockSessionV1 {
                                    expected_attempt_id: active_attempt(&session)
                                        .attempt_id()
                                        .clone(),
                                    blocker_id: blocker_id.clone(),
                                    reason: reason.clone(),
                                });
                                model.block(&blocker_id, &reason, now);
                            }
                            10 if !model.open_blocker_ids().is_empty() => {
                                let blocker_id =
                                    model.open_blocker_ids().into_iter().next().unwrap();
                                let selected = BTreeSet::from([blocker_id.clone()]);
                                command = SessionCommandV1::Unblock(UnblockSessionV1 {
                                    expected_attempt_id: active_attempt(&session)
                                        .attempt_id()
                                        .clone(),
                                    blocker_id: Some(BlockerId::new(blocker_id).unwrap()),
                                    unblock_all: false,
                                });
                                model.unblock(&selected, now);
                            }
                            11 if !model.open_blocker_ids().is_empty() => {
                                let selected = model.open_blocker_ids();
                                command = SessionCommandV1::Unblock(UnblockSessionV1 {
                                    expected_attempt_id: active_attempt(&session)
                                        .attempt_id()
                                        .clone(),
                                    blocker_id: None,
                                    unblock_all: true,
                                });
                                model.unblock(&selected, now);
                            }
                            12 if model.open_blocker_ids().is_empty() => {
                                let next_attempt_id =
                                    (stage + 1 < stage_count).then(|| ids.next_attempt());
                                command = SessionCommandV1::Complete(CompleteSessionV1 {
                                    expected_attempt_id: active_attempt(&session)
                                        .attempt_id()
                                        .clone(),
                                    next_attempt_id: next_attempt_id.clone(),
                                    local_artifact_verifications: Vec::new(),
                                });
                                fresh_attempt = next_attempt_id.is_some();
                                model.advance(false, None, next_attempt_id.as_ref(), now);
                            }
                            13 if model.open_blocker_ids().is_empty() => {
                                let next_attempt_id =
                                    (stage + 1 < stage_count).then(|| ids.next_attempt());
                                let reason = "not needed".to_owned();
                                command = SessionCommandV1::Skip(SkipSessionV1 {
                                    expected_attempt_id: active_attempt(&session)
                                        .attempt_id()
                                        .clone(),
                                    reason: Some(reason.clone()),
                                    next_attempt_id: next_attempt_id.clone(),
                                });
                                fresh_attempt = next_attempt_id.is_some();
                                model.advance(true, Some(reason), next_attempt_id.as_ref(), now);
                            }
                            14 if stage > 0 => {
                                let destination = (rng.next() as usize) % stage;
                                let destination_attempt_id = ids.next_attempt();
                                let reason = format!("redo-{seed}-{step}");
                                command = SessionCommandV1::Return(ReturnSessionV1 {
                                    expected_attempt_id: active_attempt(&session)
                                        .attempt_id()
                                        .clone(),
                                    destination_stage_id: procedure.stages()[destination]
                                        .id()
                                        .clone(),
                                    reason: reason.clone(),
                                    destination_attempt_id: destination_attempt_id.clone(),
                                });
                                fresh_attempt = true;
                                model.return_to(destination, &destination_attempt_id, reason, now);
                            }
                            15 => {
                                let next_attempt_id = ids.next_attempt();
                                let reason = format!("retry-{seed}-{step}");
                                command = SessionCommandV1::Retry(RetrySessionV1 {
                                    expected_attempt_id: active_attempt(&session)
                                        .attempt_id()
                                        .clone(),
                                    reason: reason.clone(),
                                    next_attempt_id: next_attempt_id.clone(),
                                });
                                fresh_attempt = true;
                                model.retry(&next_attempt_id, reason, now);
                            }
                            16 => {
                                let reason = format!("cancelled-{seed}-{step}");
                                command = SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
                                    expected_attempt_id: active_attempt(&session)
                                        .attempt_id()
                                        .clone(),
                                    reason: reason.clone(),
                                });
                                model.cancel(reason, now);
                            }
                            _ => {
                                let replacement_session_id = ids.next_session();
                                let replacement_attempt_id = ids.next_attempt();
                                command = SessionCommandV1::StartReplace(StartReplaceSessionV1 {
                                    expected_session_id: session.session_id().clone(),
                                    confirmed: true,
                                    start: StartSessionV1 {
                                        task_title: format!(
                                            "Replacement {seed}-{stage_count}-{step}"
                                        ),
                                        snapshot: procedure.clone(),
                                        session_id: replacement_session_id.clone(),
                                        first_attempt_id: replacement_attempt_id.clone(),
                                    },
                                });
                                fresh_attempt = true;
                                model.start_replace(
                                    stage_count,
                                    &replacement_session_id,
                                    &replacement_attempt_id,
                                    now,
                                );
                            }
                        }
                    }
                    SessionLifecycle::Completed => {
                        let destination = (rng.next() as usize) % stage_count;
                        let destination_attempt_id = ids.next_attempt();
                        let reason = format!("follow-up-{seed}-{step}");
                        command = SessionCommandV1::Reopen(ReopenSessionV1 {
                            expected_session_id: session.session_id().clone(),
                            destination_stage_id: procedure.stages()[destination].id().clone(),
                            reason: reason.clone(),
                            destination_attempt_id: destination_attempt_id.clone(),
                        });
                        fresh_attempt = true;
                        model.reopen(destination, &destination_attempt_id, reason, now);
                    }
                    SessionLifecycle::Cancelled => {
                        let replacement_session_id = ids.next_session();
                        let replacement_attempt_id = ids.next_attempt();
                        command = SessionCommandV1::StartReplace(StartReplaceSessionV1 {
                            expected_session_id: session.session_id().clone(),
                            confirmed: true,
                            start: StartSessionV1 {
                                task_title: format!("Replacement {seed}-{stage_count}-{step}"),
                                snapshot: procedure.clone(),
                                session_id: replacement_session_id.clone(),
                                first_attempt_id: replacement_attempt_id.clone(),
                            },
                        });
                        fresh_attempt = true;
                        model.start_replace(
                            stage_count,
                            &replacement_session_id,
                            &replacement_attempt_id,
                            now,
                        );
                    }
                }

                let resets_session = matches!(&command, SessionCommandV1::StartReplace(_));
                let (outcome, next) = apply_next(&session, command.clone(), now);
                assert_outcome_revision(&prior, &outcome, resets_session);
                model_hits.record(&command, &outcome);
                session = next;
                assert_invariants(&session, &mut seen_slot_revisions);
                model.assert_matches(&session);
                if fresh_attempt {
                    assert_fresh_active_attempt(&session);
                }
            }
            let reset_now = 11 + (STEPS_PER_SEED as u64 * 2);
            let reset = SessionCommandV1::Reset(ResetSessionV1 {
                expected_session_id: session.session_id().clone(),
                confirmed: true,
            });
            let reset_outcome =
                apply_equivalent(Some(&session), &reset, context(&session, reset_now));
            assert!(reset_outcome.changed());
            assert_eq!(reset_outcome.revision_before(), Some(session.revision()));
            assert_eq!(reset_outcome.revision_after(), None);
            assert!(reset_outcome.next_aggregate().is_none());
            model_hits.record(&reset, &reset_outcome);

            let reset_all = SessionCommandV1::ResetAll(ResetAllWorkspaceV1 {
                workspace_id: None,
                confirmed: true,
            });
            let reset_all_outcome =
                apply_equivalent(Some(&session), &reset_all, context(&session, reset_now));
            assert!(reset_all_outcome.changed());
            assert_eq!(
                reset_all_outcome.revision_before(),
                Some(session.revision())
            );
            assert_eq!(reset_all_outcome.revision_after(), None);
            assert!(reset_all_outcome.next_aggregate().is_none());
            assert_eq!(
                reset_all_outcome.effect(),
                Some(&TransitionEffectV1::WorkspaceResetAll { workspace_id: None })
            );
            model_hits.record(&reset_all, &reset_all_outcome);
            model.assert_matches(&session);
        }
    }
    model_hits.assert_all_covered("seeded reference model");
}
#[test]
fn required_local_artifact_completion_matches_the_reference_model() {
    let procedure = generated_snapshot_with_required_artifact(60_000, 1, true);
    let mut ids = IdSource::new(60_000);
    let start = StartSessionV1 {
        task_title: "Required local artifact".to_owned(),
        snapshot: procedure,
        session_id: ids.next_session(),
        first_attempt_id: ids.next_attempt(),
    };
    let mut model = ReferenceModel::started(1, &start.session_id, &start.first_attempt_id, 10);
    let started = apply_equivalent(
        None,
        &SessionCommandV1::Start(start),
        CommandContextV1 {
            expected_revision: Revision::ZERO,
            now: UnixMillis::new(10),
        },
    );
    let mut session = started.next_aggregate().unwrap().clone();
    let mut seen_slot_revisions = BTreeMap::new();
    assert_invariants(&session, &mut seen_slot_revisions);
    model.assert_matches(&session);
    assert_rejected_without_mutation(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        context(&session, 11),
    );

    let unverified_session = session.clone();
    let artifact =
        ArtifactValueV1::local_path("reports/artifact.txt", digest(), 42, "text/plain").unwrap();
    let prior = session.clone();
    let (outcome, next) = apply_next(
        &session,
        SessionCommandV1::Attach(AttachItemV1 {
            item_id: item_id("artifact"),
            value: artifact.clone(),
            preconditions: item_preconditions(&session, "artifact"),
        }),
        11,
    );
    assert_outcome_revision(&prior, &outcome, false);
    model.write("artifact", ModelValue::Artifact(artifact.clone()), 11);
    session = next;
    assert_invariants(&session, &mut seen_slot_revisions);
    model.assert_matches(&session);
    let verification = LocalArtifactVerificationV1 {
        item_id: item_id("artifact"),
        location: artifact.location().to_owned(),
        digest: artifact.digest().clone(),
        size_bytes: artifact.size_bytes(),
    };

    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        context(&session, 12),
        DomainError::InvalidState {
            reason: "required local artifact was not verified",
        },
    );
    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![LocalArtifactVerificationV1 {
                item_id: item_id("confirm"),
                ..verification.clone()
            }],
        }),
        context(&session, 12),
        DomainError::InvalidState {
            reason: "item command does not match the item type",
        },
    );
    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![LocalArtifactVerificationV1 {
                location: "reports/stale.txt".to_owned(),
                ..verification.clone()
            }],
        }),
        context(&session, 12),
        DomainError::InvalidState {
            reason: "local artifact verification does not match the attached artifact",
        },
    );
    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![LocalArtifactVerificationV1 {
                digest: Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap(),
                ..verification.clone()
            }],
        }),
        context(&session, 12),
        DomainError::InvalidState {
            reason: "local artifact verification does not match the attached artifact",
        },
    );
    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![LocalArtifactVerificationV1 {
                size_bytes: artifact.size_bytes() + 1,
                ..verification.clone()
            }],
        }),
        context(&session, 12),
        DomainError::InvalidState {
            reason: "local artifact verification does not match the attached artifact",
        },
    );
    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![LocalArtifactVerificationV1 {
                item_id: item_id("unknown"),
                ..verification.clone()
            }],
        }),
        context(&session, 12),
        DomainError::ItemNotFound {
            item_id: item_id("unknown"),
        },
    );
    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![verification.clone(), verification.clone()],
        }),
        context(&session, 12),
        DomainError::InvalidState {
            reason: "local artifact verification item identifiers must be unique",
        },
    );
    assert_rejected_without_mutation_with_error(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![
                verification.clone(),
                LocalArtifactVerificationV1 {
                    item_id: item_id("unknown"),
                    ..verification.clone()
                },
            ],
        }),
        context(&session, 12),
        DomainError::ItemNotFound {
            item_id: item_id("unknown"),
        },
    );

    let non_local_artifact =
        ArtifactValueV1::external_reference("artifact:external", digest(), 42, "text/plain")
            .unwrap();
    let (_, non_local_session) = apply_next(
        &unverified_session,
        SessionCommandV1::Attach(AttachItemV1 {
            item_id: item_id("artifact"),
            value: non_local_artifact.clone(),
            preconditions: item_preconditions(&unverified_session, "artifact"),
        }),
        11,
    );
    assert_rejected_without_mutation_with_error(
        &non_local_session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&non_local_session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![LocalArtifactVerificationV1 {
                item_id: item_id("artifact"),
                location: non_local_artifact.location().to_owned(),
                digest: non_local_artifact.digest().clone(),
                size_bytes: non_local_artifact.size_bytes(),
            }],
        }),
        context(&non_local_session, 12),
        DomainError::InvalidState {
            reason: "local artifact verification does not match the attached artifact",
        },
    );

    let prior = session.clone();
    let (outcome, next) = apply_next(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: active_attempt(&session).attempt_id().clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![LocalArtifactVerificationV1 {
                item_id: item_id("artifact"),
                location: artifact.location().to_owned(),
                digest: artifact.digest().clone(),
                size_bytes: artifact.size_bytes(),
            }],
        }),
        12,
    );
    assert_outcome_revision(&prior, &outcome, false);
    model.advance(false, None, None, 12);
    session = next;
    assert_invariants(&session, &mut seen_slot_revisions);
    model.assert_matches(&session);

    let artifact_attempt = attempt_by_stage_number(&session, 0, 1);
    let artifact_slot = artifact_attempt
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id("artifact"))
        .unwrap();
    assert_eq!(artifact_attempt.started_at(), UnixMillis::new(10));
    assert_eq!(artifact_attempt.ended_at(), Some(UnixMillis::new(12)));
    assert_eq!(artifact_slot.created_at(), Some(UnixMillis::new(11)));
    assert_eq!(artifact_slot.updated_at(), Some(UnixMillis::new(11)));
    assert_eq!(session.latest_recorded_at(), UnixMillis::new(12));
}

#[test]
fn item_slot_timestamp_metadata_and_reachability_are_validated() {
    let procedure = generated_snapshot(60_001, 1);
    let list = procedure.stages()[0].item(&item_id("list")).unwrap();
    let text = procedure.stages()[0].item(&item_id("text")).unwrap();
    let minimum_only_integer =
        ItemSpecV1::integer(common("minimum-only-integer"), Some(i64::MAX), None).unwrap();
    let maximum_only_integer =
        ItemSpecV1::integer(common("maximum-only-integer"), None, Some(i64::MIN)).unwrap();
    let mut ids = IdSource::new(60_001);

    assert!(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: ids.next_attempt(),
            specification: list,
            revision: Revision::new(1),
            value: Some(ItemValueV1::list(vec!["first".to_owned(), "second".to_owned()]).unwrap(),),
            created_at: Some(UnixMillis::new(10)),
            updated_at: Some(UnixMillis::new(10)),
        })
        .is_ok()
    );
    assert!(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: ids.next_attempt(),
            specification: list,
            revision: Revision::new(3),
            value: Some(ItemValueV1::list(Vec::new()).unwrap()),
            created_at: Some(UnixMillis::new(10)),
            updated_at: Some(UnixMillis::new(12)),
        })
        .is_ok()
    );
    assert!(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: ids.next_attempt(),
            specification: list,
            revision: Revision::new(3),
            value: Some(ItemValueV1::list(vec!["first".to_owned(), "second".to_owned()]).unwrap(),),
            created_at: Some(UnixMillis::new(10)),
            updated_at: Some(UnixMillis::new(12)),
        })
        .is_ok()
    );
    assert!(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: ids.next_attempt(),
            specification: &minimum_only_integer,
            revision: Revision::new(2),
            value: Some(ItemValueV1::integer(i64::MAX)),
            created_at: Some(UnixMillis::new(10)),
            updated_at: Some(UnixMillis::new(11)),
        })
        .is_err()
    );
    assert!(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: ids.next_attempt(),
            specification: &maximum_only_integer,
            revision: Revision::new(2),
            value: Some(ItemValueV1::integer(i64::MIN)),
            created_at: Some(UnixMillis::new(10)),
            updated_at: Some(UnixMillis::new(11)),
        })
        .is_err()
    );
    assert!(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: ids.next_attempt(),
            specification: text,
            revision: Revision::new(2),
            value: Some(ItemValueV1::text("reversed timestamps")),
            created_at: Some(UnixMillis::new(11)),
            updated_at: Some(UnixMillis::new(10)),
        })
        .is_err()
    );
    assert!(
        BlockerV1::new(BlockerInputV1 {
            blocker_id: ids.next_blocker(),
            attempt_id: ids.next_attempt(),
            reason: "resolution predates creation".to_owned(),
            state: BlockerState::Resolved,
            created_at: UnixMillis::new(11),
            resolved_at: Some(UnixMillis::new(10)),
        })
        .is_err()
    );
}

#[test]
fn incomplete_persisted_timestamp_and_lifecycle_metadata_are_rejected() {
    let procedure = generated_snapshot(60_002, 1);
    let stage = &procedure.stages()[0];
    let text = stage.item(&item_id("text")).unwrap();
    let mut ids = IdSource::new(60_002);
    let valid_slot = ItemSlotInputV1 {
        attempt_id: ids.next_attempt(),
        specification: text,
        revision: Revision::new(1),
        value: Some(ItemValueV1::text("value")),
        created_at: Some(UnixMillis::new(10)),
        updated_at: Some(UnixMillis::new(10)),
    };
    for input in [
        ItemSlotInputV1 {
            created_at: None,
            ..valid_slot.clone()
        },
        ItemSlotInputV1 {
            updated_at: None,
            ..valid_slot.clone()
        },
    ] {
        assert_eq!(
            ItemSlotV1::new(input).unwrap_err(),
            DomainError::InvalidState {
                reason: "invalid populated item slot metadata",
            }
        );
    }

    let terminal_attempt_id = ids.next_attempt();
    let valid_terminal_attempt = AttemptInputV1 {
        attempt_id: terminal_attempt_id.clone(),
        session_id: ids.next_session(),
        stage,
        number: 1,
        lifecycle: AttemptLifecycle::Skipped,
        started_at: UnixMillis::new(10),
        ended_at: Some(UnixMillis::new(11)),
        reason: Some("skipped".to_owned()),
        item_slots: stage
            .items()
            .iter()
            .map(|item| ItemSlotV1::new_empty(terminal_attempt_id.clone(), item))
            .collect(),
        blockers: Vec::new(),
    };
    assert_eq!(
        AttemptV1::new(AttemptInputV1 {
            ended_at: None,
            ..valid_terminal_attempt
        })
        .unwrap_err(),
        DomainError::InvalidState {
            reason: "attempt lifecycle metadata is inconsistent",
        }
    );

    let valid_resolved_blocker = BlockerInputV1 {
        blocker_id: ids.next_blocker(),
        attempt_id: ids.next_attempt(),
        reason: "waiting".to_owned(),
        state: BlockerState::Resolved,
        created_at: UnixMillis::new(10),
        resolved_at: Some(UnixMillis::new(11)),
    };
    for input in [
        BlockerInputV1 {
            resolved_at: None,
            ..valid_resolved_blocker.clone()
        },
        BlockerInputV1 {
            state: BlockerState::Open,
            ..valid_resolved_blocker.clone()
        },
    ] {
        assert_eq!(
            BlockerV1::new(input).unwrap_err(),
            DomainError::InvalidState {
                reason: "invalid blocker resolution metadata",
            }
        );
    }
}

#[test]
fn stale_and_lifecycle_rejections_are_preview_equivalent_and_immutable() {
    for seed in SEEDS {
        let procedure = generated_snapshot(seed, 3);
        let mut ids = IdSource::new(seed + 2_000);
        let start = StartSessionV1 {
            task_title: format!("Task {seed}"),
            snapshot: procedure,
            session_id: ids.next_session(),
            first_attempt_id: ids.next_attempt(),
        };
        let session = apply_equivalent(
            None,
            &SessionCommandV1::Start(start),
            CommandContextV1 {
                expected_revision: Revision::ZERO,
                now: UnixMillis::new(10),
            },
        )
        .next_aggregate()
        .unwrap()
        .clone();

        let stale_item = item_preconditions(&session, "confirm");
        let (_, checked) = apply_next(
            &session,
            SessionCommandV1::Check(CheckItemV1 {
                item_id: item_id("confirm"),
                preconditions: stale_item.clone(),
            }),
            11,
        );
        assert_rejected_without_mutation(
            &checked,
            SessionCommandV1::Check(CheckItemV1 {
                item_id: item_id("confirm"),
                preconditions: stale_item,
            }),
            context(&checked, 12),
        );
        assert_rejected_without_mutation(
            &checked,
            SessionCommandV1::Check(CheckItemV1 {
                item_id: item_id("confirm"),
                preconditions: item_preconditions(&checked, "confirm"),
            }),
            CommandContextV1 {
                expected_revision: checked.revision(),
                now: UnixMillis::new(checked.latest_recorded_at().get() - 1),
            },
        );
        assert_rejected_without_mutation(
            &checked,
            SessionCommandV1::Retry(RetrySessionV1 {
                expected_attempt_id: ids.next_attempt(),
                reason: "stale attempt".to_owned(),
                next_attempt_id: ids.next_attempt(),
            }),
            context(&checked, 12),
        );
        assert_rejected_without_mutation(
            &checked,
            SessionCommandV1::Retry(RetrySessionV1 {
                expected_attempt_id: active_attempt(&checked).attempt_id().clone(),
                reason: "stale revision".to_owned(),
                next_attempt_id: ids.next_attempt(),
            }),
            CommandContextV1 {
                expected_revision: Revision::ZERO,
                now: UnixMillis::new(12),
            },
        );

        let (_, cancelled) = apply_next(
            &checked,
            SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
                expected_attempt_id: active_attempt(&checked).attempt_id().clone(),
                reason: "stop".to_owned(),
            }),
            13,
        );
        assert_rejected_without_mutation(
            &cancelled,
            SessionCommandV1::Check(CheckItemV1 {
                item_id: item_id("confirm"),
                preconditions: ItemMutationPreconditionsV1 {
                    expected_attempt_id: checked.active_attempt_id().unwrap().clone(),
                    expected_item_revision: Revision::new(1),
                },
            }),
            context(&cancelled, 14),
        );
        let (_, second_stage) = apply_next(
            &checked,
            SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: active_attempt(&checked).attempt_id().clone(),
                next_attempt_id: Some(ids.next_attempt()),
                local_artifact_verifications: Vec::new(),
            }),
            13,
        );
        let (_, final_stage) = apply_next(
            &second_stage,
            SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: active_attempt(&second_stage).attempt_id().clone(),
                next_attempt_id: Some(ids.next_attempt()),
                local_artifact_verifications: Vec::new(),
            }),
            14,
        );
        let (_, completed) = apply_next(
            &final_stage,
            SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: active_attempt(&final_stage).attempt_id().clone(),
                next_attempt_id: None,
                local_artifact_verifications: Vec::new(),
            }),
            15,
        );
        assert_rejected_without_mutation(
            &completed,
            SessionCommandV1::Reopen(ReopenSessionV1 {
                expected_session_id: ids.next_session(),
                destination_stage_id: completed.snapshot().stages()[0].id().clone(),
                reason: "stale session identity".to_owned(),
                destination_attempt_id: ids.next_attempt(),
            }),
            context(&completed, 16),
        );
    }
}
#[test]
fn session_revision_advances_for_legal_non_item_composites() {
    let snapshot = generated_snapshot(60_003, 1);
    let mut ids = IdSource::new(60_003);
    let start = |session_id, first_attempt_id| {
        apply_equivalent(
            None,
            &SessionCommandV1::Start(StartSessionV1 {
                task_title: "Composite transitions".to_owned(),
                snapshot: snapshot.clone(),
                session_id,
                first_attempt_id,
            }),
            CommandContextV1 {
                expected_revision: Revision::ZERO,
                now: UnixMillis::new(10),
            },
        )
        .next_aggregate()
        .unwrap()
        .clone()
    };

    let terminal_and_start = start(ids.next_session(), ids.next_attempt());
    let terminal_attempt_id = active_attempt(&terminal_and_start).attempt_id().clone();
    let retried_attempt_id = ids.next_attempt();
    let (_, retried) = apply_next(
        &terminal_and_start,
        SessionCommandV1::Retry(RetrySessionV1 {
            expected_attempt_id: terminal_attempt_id.clone(),
            reason: "retry".to_owned(),
            next_attempt_id: retried_attempt_id.clone(),
        }),
        11,
    );
    assert_eq!(retried.revision(), Revision::new(2));
    let terminal_attempt = attempt_by_id(&retried, &terminal_attempt_id);
    assert_eq!(terminal_attempt.lifecycle(), AttemptLifecycle::Abandoned);
    assert_eq!(terminal_attempt.ended_at(), Some(UnixMillis::new(11)));
    assert_eq!(terminal_attempt.reason(), Some("retry"));
    let retried_attempt = attempt_by_id(&retried, &retried_attempt_id);
    assert_eq!(retried_attempt.lifecycle(), AttemptLifecycle::Active);
    assert_eq!(retried_attempt.started_at(), UnixMillis::new(11));
    retried.validate().unwrap();

    let terminal_with_blocker_batch = start(ids.next_session(), ids.next_attempt());
    let terminal_attempt_id = active_attempt(&terminal_with_blocker_batch)
        .attempt_id()
        .clone();
    let first_blocker_id = ids.next_blocker();
    let (_, first_blocked) = apply_next(
        &terminal_with_blocker_batch,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: terminal_attempt_id.clone(),
            blocker_id: first_blocker_id.clone(),
            reason: "first wait".to_owned(),
        }),
        11,
    );
    let second_blocker_id = ids.next_blocker();
    let (_, second_blocked) = apply_next(
        &first_blocked,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: terminal_attempt_id.clone(),
            blocker_id: second_blocker_id.clone(),
            reason: "second wait".to_owned(),
        }),
        12,
    );
    let (_, cancelled) = apply_next(
        &second_blocked,
        SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
            expected_attempt_id: terminal_attempt_id.clone(),
            reason: "cancelled".to_owned(),
        }),
        13,
    );
    assert_eq!(cancelled.revision(), Revision::new(4));
    let cancelled_attempt = attempt_by_id(&cancelled, &terminal_attempt_id);
    assert_eq!(cancelled_attempt.lifecycle(), AttemptLifecycle::Abandoned);
    assert_eq!(cancelled_attempt.ended_at(), Some(UnixMillis::new(13)));
    assert_eq!(cancelled_attempt.reason(), Some("cancelled"));
    for blocker_id in [&first_blocker_id, &second_blocker_id] {
        let blocker = cancelled_attempt
            .blockers()
            .iter()
            .find(|blocker| blocker.blocker_id() == blocker_id)
            .unwrap();
        assert_eq!(blocker.state(), BlockerState::Resolved);
        assert_eq!(blocker.resolved_at(), Some(UnixMillis::new(13)));
    }
    cancelled.validate().unwrap();

    let explicit_unblock = start(ids.next_session(), ids.next_attempt());
    let terminal_attempt_id = active_attempt(&explicit_unblock).attempt_id().clone();
    let blocker_id = ids.next_blocker();
    let (_, blocked) = apply_next(
        &explicit_unblock,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: terminal_attempt_id.clone(),
            blocker_id: blocker_id.clone(),
            reason: "waiting".to_owned(),
        }),
        11,
    );
    let (_, unblocked) = apply_next(
        &blocked,
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: terminal_attempt_id.clone(),
            blocker_id: Some(blocker_id.clone()),
            unblock_all: false,
        }),
        12,
    );
    assert_eq!(unblocked.revision(), Revision::new(3));
    let retried_attempt_id = ids.next_attempt();
    let (_, terminal_after_unblock) = apply_next(
        &unblocked,
        SessionCommandV1::Retry(RetrySessionV1 {
            expected_attempt_id: terminal_attempt_id.clone(),
            reason: "retry".to_owned(),
            next_attempt_id: retried_attempt_id.clone(),
        }),
        12,
    );
    assert_eq!(terminal_after_unblock.revision(), Revision::new(4));
    let terminal_attempt = attempt_by_id(&terminal_after_unblock, &terminal_attempt_id);
    let blocker = terminal_attempt
        .blockers()
        .iter()
        .find(|blocker| blocker.blocker_id() == &blocker_id)
        .unwrap();
    assert_eq!(blocker.resolved_at(), Some(UnixMillis::new(12)));
    assert_eq!(terminal_attempt.ended_at(), Some(UnixMillis::new(12)));
    assert_eq!(terminal_attempt.reason(), Some("retry"));
    let retried_attempt = attempt_by_id(&terminal_after_unblock, &retried_attempt_id);
    assert_eq!(retried_attempt.started_at(), UnixMillis::new(12));
    terminal_after_unblock.validate().unwrap();
}
