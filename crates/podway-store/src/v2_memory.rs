//! Procedure v2 attempt-local workflow-memory persistence.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use podway_core::{
    ActorAttributionV2, ArtifactLocationKindV1, ArtifactValueV1, AttemptId, AttemptLifecycle,
    AttemptNumberV2, AttemptValidityV2, BlockerId, BlockerState, DecisionRecordInputV2,
    DecisionRecordV2, EvidenceReferenceSnapshotV2, GoalRevisionNumberV2, GraphNodeId, ItemCommonV2,
    ItemId, ItemSpecV2, ItemTypeV1, NodeDefinitionId, OptionId, ProcedureSnapshotId, ReasonV2,
    RecordedItemSetV2, RecordedItemV2, RecordedItemValueV2, ResolvedEvidenceReferenceV2,
    ResolvedEvidenceSetV2, Revision, ReworkKindV2, ReworkRecordInputV2, ReworkRecordV2,
    SessionAttemptV2, SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2,
    TransitionEffectV2, UnixMillis, canonicalize_json_v1, verify_canonical_json_v1,
};
use rusqlite::{Connection, Transaction, params};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::v2_state::{AttemptMetadataV2, ProcedureSnapshotV2};
use crate::{
    RusqliteErrorContextV1, StoreErrorV1, StoreRecordKindV1, StoreValueErrorV1,
    map_rusqlite_error_v1,
};

const MAX_OPEN_BLOCKERS_V2: usize = 64;
const MAX_BLOCKER_REASON_CHARS_V2: usize = 1_000;

fn invalid(reason: &'static str) -> StoreValueErrorV1 {
    StoreValueErrorV1::InvalidProcedureV2State { reason }
}

fn corrupt(record: StoreRecordKindV1) -> StoreErrorV1 {
    StoreErrorV1::CorruptStateV1 { record }
}

fn record_error(error: rusqlite::Error, record: StoreRecordKindV1) -> StoreErrorV1 {
    map_rusqlite_error_v1(error, RusqliteErrorContextV1::Record(record))
}

fn sqlite_u64(value: u64, field: &'static str) -> Result<i64, StoreErrorV1> {
    i64::try_from(value)
        .map_err(|_| StoreErrorV1::InvalidStateV1(StoreValueErrorV1::IntegerOutOfRange { field }))
}

fn persisted_u64(value: i64, record: StoreRecordKindV1) -> Result<u64, StoreErrorV1> {
    u64::try_from(value).map_err(|_| corrupt(record))
}

/// One version-neutral item command applied to the active Procedure v2 attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActiveItemMutationV2 {
    Check,
    Uncheck,
    Set { value: String },
    Add { value: String },
    Remove { value: String, ignore_missing: bool },
    Attach { value: ArtifactValueV1 },
    Clear,
}

/// Stable typed failures produced before a Procedure v2 graph mutation reaches persistence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphMutationErrorV2 {
    SessionNotRunning,
    SessionRevisionConflict {
        expected: Revision,
        actual: Revision,
    },
    AttemptNotCurrent {
        expected: AttemptId,
        actual: Option<AttemptId>,
    },
    GraphNodeTypeMismatch {
        graph_node_id: GraphNodeId,
        actual: podway_core::NodeKindV2,
    },
    ItemNotFound {
        item_id: ItemId,
    },
    ItemRevisionConflict {
        expected: Revision,
        actual: Revision,
    },
    ItemTypeMismatch,
    ItemConstraintFailed,
    ListValueNotFound,
    ListValueDuplicate,
    RequiredItemsMissing {
        item_ids: Vec<ItemId>,
    },
    BlockersPresent,
    SessionGoalMissing,
    FreshGoalAssessmentMissing {
        goal_revision: GoalRevisionNumberV2,
    },
    InvalidState(StoreValueErrorV1),
    Domain(podway_core::DomainError),
}

impl fmt::Display for GraphMutationErrorV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotRunning => formatter.write_str("Procedure v2 session is not running"),
            Self::SessionRevisionConflict { .. } => {
                formatter.write_str("Procedure v2 session revision changed")
            }
            Self::AttemptNotCurrent { .. } => {
                formatter.write_str("Procedure v2 attempt is not current")
            }
            Self::GraphNodeTypeMismatch { .. } => {
                formatter.write_str("Procedure v2 graph node has the wrong type")
            }
            Self::ItemNotFound { .. } => formatter.write_str("Procedure v2 item was not found"),
            Self::ItemRevisionConflict { .. } => {
                formatter.write_str("Procedure v2 item revision changed")
            }
            Self::ItemTypeMismatch => {
                formatter.write_str("Procedure v2 item command does not match its type")
            }
            Self::ItemConstraintFailed => {
                formatter.write_str("Procedure v2 item value violates its declaration")
            }
            Self::ListValueNotFound => formatter.write_str("list item value is not present"),
            Self::ListValueDuplicate => formatter.write_str("list item value is already present"),
            Self::RequiredItemsMissing { .. } => {
                formatter.write_str("Procedure v2 required items are missing")
            }
            Self::BlockersPresent => formatter.write_str("Procedure v2 blockers are present"),
            Self::SessionGoalMissing => {
                formatter.write_str("the Procedure v2 session goal is missing")
            }
            Self::FreshGoalAssessmentMissing { .. } => {
                formatter.write_str("a fresh final goal assessment is missing")
            }
            Self::InvalidState(error) => error.fmt(formatter),
            Self::Domain(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for GraphMutationErrorV2 {}

impl From<StoreValueErrorV1> for GraphMutationErrorV2 {
    fn from(error: StoreValueErrorV1) -> Self {
        Self::InvalidState(error)
    }
}

impl From<podway_core::DomainError> for GraphMutationErrorV2 {
    fn from(error: podway_core::DomainError) -> Self {
        Self::Domain(error)
    }
}

pub(crate) struct ActiveItemMemoryMutationV2 {
    pub memory: WorkflowMemoryStateV2,
    pub changed: bool,
    pub item_revision: Revision,
    pub value_digest: Option<Sha256Digest>,
}

/// One mutable item slot belonging to a Procedure v2 attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemSlotStateV2 {
    attempt_id: AttemptId,
    item_id: ItemId,
    item_type: ItemTypeV1,
    revision: Revision,
    value: Option<RecordedItemValueV2>,
    created_at: UnixMillis,
    updated_at: UnixMillis,
}

impl ItemSlotStateV2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        attempt_id: AttemptId,
        item_id: ItemId,
        item_type: ItemTypeV1,
        revision: Revision,
        value: Option<RecordedItemValueV2>,
        created_at: UnixMillis,
        updated_at: UnixMillis,
    ) -> Result<Self, StoreValueErrorV1> {
        if value
            .as_ref()
            .is_some_and(|value| value.item_type() != item_type)
        {
            return Err(invalid("Procedure v2 item slot value type is inconsistent"));
        }
        if revision == Revision::ZERO && (value.is_some() || created_at != updated_at) {
            return Err(invalid(
                "Procedure v2 unset item slot metadata is inconsistent",
            ));
        }
        if updated_at < created_at {
            return Err(invalid("Procedure v2 item slot timestamps are unordered"));
        }
        Ok(Self {
            attempt_id,
            item_id,
            item_type,
            revision,
            value,
            created_at,
            updated_at,
        })
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }
    pub fn item_id(&self) -> &ItemId {
        &self.item_id
    }
    pub const fn item_type(&self) -> ItemTypeV1 {
        self.item_type
    }
    pub const fn revision(&self) -> Revision {
        self.revision
    }
    pub fn value(&self) -> Option<&RecordedItemValueV2> {
        self.value.as_ref()
    }
    pub const fn created_at(&self) -> UnixMillis {
        self.created_at
    }
    pub const fn updated_at(&self) -> UnixMillis {
        self.updated_at
    }
}

/// A bounded Procedure v2 blocker. It intentionally does not reuse the larger v1 reason bound.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockerStateV2 {
    blocker_id: BlockerId,
    attempt_id: AttemptId,
    reason: String,
    state: BlockerState,
    created_at: UnixMillis,
    resolved_at: Option<UnixMillis>,
}

impl BlockerStateV2 {
    pub fn new(
        blocker_id: BlockerId,
        attempt_id: AttemptId,
        reason: impl Into<String>,
        state: BlockerState,
        created_at: UnixMillis,
        resolved_at: Option<UnixMillis>,
    ) -> Result<Self, StoreValueErrorV1> {
        let reason = reason.into();
        if reason.trim().is_empty() || reason.chars().count() > MAX_BLOCKER_REASON_CHARS_V2 {
            return Err(invalid("Procedure v2 blocker reason is invalid"));
        }
        let timestamps_valid = match state {
            BlockerState::Open => resolved_at.is_none(),
            BlockerState::Resolved => resolved_at.is_some_and(|value| value >= created_at),
        };
        if !timestamps_valid {
            return Err(invalid(
                "Procedure v2 blocker state metadata is inconsistent",
            ));
        }
        Ok(Self {
            blocker_id,
            attempt_id,
            reason,
            state,
            created_at,
            resolved_at,
        })
    }

    pub fn blocker_id(&self) -> &BlockerId {
        &self.blocker_id
    }
    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }
    pub fn reason(&self) -> &str {
        &self.reason
    }
    pub const fn state(&self) -> BlockerState {
        self.state
    }
    pub const fn created_at(&self) -> UnixMillis {
        self.created_at
    }
    pub const fn resolved_at(&self) -> Option<UnixMillis> {
        self.resolved_at
    }
}

/// Declaration metadata paired with the immutable result of resolving one evidence reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceResolutionStateV2 {
    ordinal: u32,
    required: bool,
    selected_item_ids: Vec<ItemId>,
    resolution: ResolvedEvidenceReferenceV2,
}

impl EvidenceResolutionStateV2 {
    pub fn new(
        ordinal: u32,
        required: bool,
        selected_item_ids: Vec<ItemId>,
        resolution: ResolvedEvidenceReferenceV2,
    ) -> Result<Self, StoreValueErrorV1> {
        if ordinal >= 8 || selected_item_ids.len() > 16 {
            return Err(invalid(
                "Procedure v2 evidence reference metadata is out of bounds",
            ));
        }
        let unique = selected_item_ids.iter().collect::<BTreeSet<_>>();
        if unique.len() != selected_item_ids.len() {
            return Err(invalid("Procedure v2 evidence selectors must be unique"));
        }
        if required && resolution.is_unresolved() {
            return Err(invalid(
                "required Procedure v2 evidence cannot be unresolved",
            ));
        }
        if required && matches!(resolution, ResolvedEvidenceReferenceV2::Skipped(_)) {
            return Err(invalid("required Procedure v2 evidence cannot be skipped"));
        }
        Ok(Self {
            ordinal,
            required,
            selected_item_ids,
            resolution,
        })
    }

    pub const fn ordinal(&self) -> u32 {
        self.ordinal
    }
    pub const fn required(&self) -> bool {
        self.required
    }
    pub fn selected_item_ids(&self) -> &[ItemId] {
        &self.selected_item_ids
    }
    pub fn resolution(&self) -> &ResolvedEvidenceReferenceV2 {
        &self.resolution
    }
}

/// All attempt-local workflow memory, preserving Procedure author order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptWorkflowMemoryV2 {
    attempt_id: AttemptId,
    item_slots: Vec<ItemSlotStateV2>,
    blockers: Vec<BlockerStateV2>,
    evidence: Vec<EvidenceResolutionStateV2>,
}

impl AttemptWorkflowMemoryV2 {
    pub fn new(
        attempt_id: AttemptId,
        item_slots: Vec<ItemSlotStateV2>,
        blockers: Vec<BlockerStateV2>,
        evidence: Vec<EvidenceResolutionStateV2>,
    ) -> Result<Self, StoreValueErrorV1> {
        if item_slots
            .iter()
            .any(|slot| slot.attempt_id() != &attempt_id)
            || blockers
                .iter()
                .any(|blocker| blocker.attempt_id() != &attempt_id)
        {
            return Err(invalid(
                "Procedure v2 workflow memory owner is inconsistent",
            ));
        }
        if item_slots
            .iter()
            .map(ItemSlotStateV2::item_id)
            .collect::<BTreeSet<_>>()
            .len()
            != item_slots.len()
            || blockers
                .iter()
                .map(BlockerStateV2::blocker_id)
                .collect::<BTreeSet<_>>()
                .len()
                != blockers.len()
        {
            return Err(invalid(
                "Procedure v2 workflow memory identities must be unique",
            ));
        }
        if blockers
            .iter()
            .filter(|blocker| blocker.state() == BlockerState::Open)
            .count()
            > MAX_OPEN_BLOCKERS_V2
        {
            return Err(invalid(
                "a Procedure v2 attempt holds at most 64 open blockers",
            ));
        }
        if evidence.iter().enumerate().any(|(index, reference)| {
            reference.ordinal() != u32::try_from(index).unwrap_or(u32::MAX)
        }) {
            return Err(invalid(
                "Procedure v2 evidence ordinals must be consecutive",
            ));
        }
        Ok(Self {
            attempt_id,
            item_slots,
            blockers,
            evidence,
        })
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }
    pub fn item_slots(&self) -> &[ItemSlotStateV2] {
        &self.item_slots
    }
    pub fn blockers(&self) -> &[BlockerStateV2] {
        &self.blockers
    }
    pub fn evidence(&self) -> &[EvidenceResolutionStateV2] {
        &self.evidence
    }

    pub fn recorded_items(&self) -> Result<RecordedItemSetV2, StoreValueErrorV1> {
        RecordedItemSetV2::new(
            self.item_slots
                .iter()
                .filter_map(|slot| {
                    slot.value()
                        .cloned()
                        .map(|value| RecordedItemV2::new(slot.item_id().clone(), value))
                })
                .collect(),
        )
        .map_err(|_| invalid("Procedure v2 recorded item set is invalid"))
    }

    pub fn recorded_items_digest(&self) -> Result<Sha256Digest, StoreValueErrorV1> {
        recorded_items_digest_v2(&self.recorded_items()?)
    }
}

/// One evidence reference and the exact selected values reconstructed for read-back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceReadbackV2 {
    reference: EvidenceResolutionStateV2,
    items: RecordedItemSetV2,
    decision: Option<DecisionRecordV2>,
    stale: bool,
}

impl EvidenceReadbackV2 {
    pub fn reference(&self) -> &EvidenceResolutionStateV2 {
        &self.reference
    }
    pub fn items(&self) -> &RecordedItemSetV2 {
        &self.items
    }
    pub fn decision(&self) -> Option<&DecisionRecordV2> {
        self.decision.as_ref()
    }
    pub const fn stale(&self) -> bool {
        self.stale
    }
}

/// Complete workflow memory and immutable decision/rework history for one v2 session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowMemoryStateV2 {
    attempts: Vec<AttemptWorkflowMemoryV2>,
    decisions: Vec<DecisionRecordV2>,
    reworks: Vec<ReworkRecordV2>,
}

/// Goal-state change that supplies the causal record for a workflow transition.
///
/// Goal revisions intentionally do not synthesize [`ReworkRecordV2`] rows. The goal-state
/// validator establishes the revision-specific invariants before this context is passed to the
/// workflow-memory successor validator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowGoalTransitionV2<'a> {
    None,
    InitialBinding { attempt_id: &'a AttemptId },
    Rework { target_attempt_id: &'a AttemptId },
}

impl WorkflowMemoryStateV2 {
    pub fn new(
        attempts: Vec<AttemptWorkflowMemoryV2>,
        decisions: Vec<DecisionRecordV2>,
        reworks: Vec<ReworkRecordV2>,
    ) -> Result<Self, StoreValueErrorV1> {
        if attempts
            .iter()
            .map(AttemptWorkflowMemoryV2::attempt_id)
            .collect::<BTreeSet<_>>()
            .len()
            != attempts.len()
            || decisions
                .iter()
                .map(DecisionRecordV2::attempt_id)
                .collect::<BTreeSet<_>>()
                .len()
                != decisions.len()
            || reworks
                .iter()
                .map(ReworkRecordV2::trace)
                .collect::<BTreeSet<_>>()
                .len()
                != reworks.len()
        {
            return Err(invalid(
                "Procedure v2 workflow history identities must be unique",
            ));
        }
        Ok(Self {
            attempts,
            decisions,
            reworks,
        })
    }

    pub fn attempts(&self) -> &[AttemptWorkflowMemoryV2] {
        &self.attempts
    }
    pub fn decisions(&self) -> &[DecisionRecordV2] {
        &self.decisions
    }
    pub fn reworks(&self) -> &[ReworkRecordV2] {
        &self.reworks
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn mutate_active_item_v2(
        &self,
        snapshot: &ProcedureSnapshotV2,
        trace: &SessionTraceV2,
        expected_attempt_id: &AttemptId,
        item_id: &ItemId,
        expected_item_revision: Revision,
        mutation: ActiveItemMutationV2,
        now: UnixMillis,
    ) -> Result<ActiveItemMemoryMutationV2, GraphMutationErrorV2> {
        if trace.lifecycle() != SessionLifecycle::Running {
            return Err(GraphMutationErrorV2::SessionNotRunning);
        }
        let active =
            trace
                .active_attempt()
                .ok_or_else(|| GraphMutationErrorV2::AttemptNotCurrent {
                    expected: expected_attempt_id.clone(),
                    actual: None,
                })?;
        if active.attempt_id() != expected_attempt_id {
            return Err(GraphMutationErrorV2::AttemptNotCurrent {
                expected: expected_attempt_id.clone(),
                actual: Some(active.attempt_id().clone()),
            });
        }
        let model = SnapshotMemoryModelV2::from_snapshot(snapshot)?;
        let node = model.node(active.graph_node_id())?;
        let specification = node
            .items
            .iter()
            .find(|specification| specification.id() == item_id)
            .ok_or_else(|| GraphMutationErrorV2::ItemNotFound {
                item_id: item_id.clone(),
            })?;
        let attempt_index = self
            .attempts
            .iter()
            .position(|memory| memory.attempt_id() == expected_attempt_id)
            .ok_or_else(|| invalid("active Procedure v2 workflow memory is absent"))?;
        let slot_index = self.attempts[attempt_index]
            .item_slots
            .iter()
            .position(|slot| slot.item_id() == item_id)
            .ok_or_else(|| invalid("active Procedure v2 item slot is absent"))?;
        let current = &self.attempts[attempt_index].item_slots[slot_index];
        if current.revision() != expected_item_revision {
            return Err(GraphMutationErrorV2::ItemRevisionConflict {
                expected: expected_item_revision,
                actual: current.revision(),
            });
        }
        if now < current.updated_at() {
            return Err(invalid("Procedure v2 item mutation timestamp regressed").into());
        }

        let next_value = mutate_item_value_v2(specification, current.value(), mutation)?;
        let value_digest = next_value
            .as_ref()
            .and_then(RecordedItemValueV2::as_artifact)
            .map(|artifact| artifact.digest().clone());
        if current.value() == next_value.as_ref() {
            return Ok(ActiveItemMemoryMutationV2 {
                memory: self.clone(),
                changed: false,
                item_revision: current.revision(),
                value_digest,
            });
        }
        let next_revision = current
            .revision()
            .checked_next()
            .map_err(GraphMutationErrorV2::Domain)?;
        let next_slot = ItemSlotStateV2::new(
            current.attempt_id().clone(),
            current.item_id().clone(),
            current.item_type(),
            next_revision,
            next_value,
            current.created_at(),
            now,
        )?;
        let mut attempts = self.attempts.clone();
        let active_memory = &self.attempts[attempt_index];
        let mut slots = active_memory.item_slots.clone();
        slots[slot_index] = next_slot;
        attempts[attempt_index] = AttemptWorkflowMemoryV2::new(
            active_memory.attempt_id.clone(),
            slots,
            active_memory.blockers.clone(),
            active_memory.evidence.clone(),
        )?;
        Ok(ActiveItemMemoryMutationV2 {
            memory: Self::new(attempts, self.decisions.clone(), self.reworks.clone())?,
            changed: true,
            item_revision: next_revision,
            value_digest,
        })
    }

    pub(crate) fn complete_action_successor_v2(
        &self,
        snapshot: &ProcedureSnapshotV2,
        previous_trace: &SessionTraceV2,
        next_trace: &SessionTraceV2,
        ended_at: UnixMillis,
    ) -> Result<Self, GraphMutationErrorV2> {
        let source = previous_trace
            .active_attempt()
            .ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        let source_memory = self
            .attempts
            .iter()
            .find(|memory| memory.attempt_id() == source.attempt_id())
            .ok_or_else(|| invalid("active Procedure v2 workflow memory is absent"))?;
        let model = SnapshotMemoryModelV2::from_snapshot(snapshot)?;
        let source_specification = model.node(source.graph_node_id())?;
        if source_specification.node_kind != podway_core::NodeKindV2::Action {
            return Err(GraphMutationErrorV2::GraphNodeTypeMismatch {
                graph_node_id: source.graph_node_id().clone(),
                actual: source_specification.node_kind,
            });
        }
        let missing = source_specification
            .items
            .iter()
            .zip(source_memory.item_slots())
            .filter(|(specification, slot)| {
                specification.common().required()
                    && !slot
                        .value()
                        .is_some_and(|value| specification.admits_recorded_value(value))
            })
            .map(|(specification, _)| specification.id().clone())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(GraphMutationErrorV2::RequiredItemsMissing { item_ids: missing });
        }
        if source_memory
            .blockers()
            .iter()
            .any(|blocker| blocker.state() == BlockerState::Open)
        {
            return Err(GraphMutationErrorV2::BlockersPresent);
        }

        let mut attempts = self.attempts.clone();
        if let Some(fresh) = next_trace.active_attempt() {
            let fresh_specification = model.node(fresh.graph_node_id())?;
            let slots = fresh_specification
                .items
                .iter()
                .map(|item| {
                    ItemSlotStateV2::new(
                        fresh.attempt_id().clone(),
                        item.id().clone(),
                        item.item_type(),
                        Revision::ZERO,
                        None,
                        ended_at,
                        ended_at,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let evidence =
                resolve_evidence_at_activation_v2(fresh_specification, next_trace, self, ended_at)?;
            attempts.push(AttemptWorkflowMemoryV2::new(
                fresh.attempt_id().clone(),
                slots,
                Vec::new(),
                evidence,
            )?);
        }
        Self::new(attempts, self.decisions.clone(), self.reworks.clone()).map_err(Into::into)
    }

    pub(crate) fn retry_successor_v2(
        &self,
        snapshot: &ProcedureSnapshotV2,
        next_trace: &SessionTraceV2,
        started_at: UnixMillis,
    ) -> Result<Self, GraphMutationErrorV2> {
        let fresh = next_trace
            .active_attempt()
            .ok_or(GraphMutationErrorV2::SessionNotRunning)?;
        let model = SnapshotMemoryModelV2::from_snapshot(snapshot)?;
        let specification = model.node(fresh.graph_node_id())?;
        let slots = specification
            .items
            .iter()
            .map(|item| {
                ItemSlotStateV2::new(
                    fresh.attempt_id().clone(),
                    item.id().clone(),
                    item.item_type(),
                    Revision::ZERO,
                    None,
                    started_at,
                    started_at,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let evidence =
            resolve_evidence_at_activation_v2(specification, next_trace, self, started_at)?;
        let mut attempts = self.attempts.clone();
        attempts.push(AttemptWorkflowMemoryV2::new(
            fresh.attempt_id().clone(),
            slots,
            Vec::new(),
            evidence,
        )?);
        Self::new(attempts, self.decisions.clone(), self.reworks.clone()).map_err(Into::into)
    }

    pub fn empty_for_trace(
        snapshot: &ProcedureSnapshotV2,
        trace: &SessionTraceV2,
        metadata: &[AttemptMetadataV2],
    ) -> Result<Self, StoreValueErrorV1> {
        let model = SnapshotMemoryModelV2::from_snapshot(snapshot)?;
        let mut attempts = Vec::with_capacity(trace.attempts().len());
        for (attempt, metadata) in trace.attempts().iter().zip(metadata) {
            let node = model.node(attempt.graph_node_id())?;
            if !node.evidence.is_empty() {
                return Err(invalid(
                    "Procedure v2 evidence resolution is required for an activated attempt",
                ));
            }
            let slots = node
                .items
                .iter()
                .map(|item| {
                    ItemSlotStateV2::new(
                        attempt.attempt_id().clone(),
                        item.id().clone(),
                        item.item_type(),
                        Revision::ZERO,
                        None,
                        metadata.started_at(),
                        metadata.started_at(),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            attempts.push(AttemptWorkflowMemoryV2::new(
                attempt.attempt_id().clone(),
                slots,
                Vec::new(),
                Vec::new(),
            )?);
        }
        Self::new(attempts, Vec::new(), Vec::new())
    }

    /// Constructs memory for the first attempt of a newly admitted graph session. Optional
    /// references may legitimately be unresolved at activation; a required unresolved reference
    /// is rejected because graph vetting must have ruled it out before runtime admission.
    pub fn initial_for_trace(
        snapshot: &ProcedureSnapshotV2,
        trace: &SessionTraceV2,
        metadata: &[AttemptMetadataV2],
    ) -> Result<Self, StoreValueErrorV1> {
        let initial_attempt = trace.attempts().first();
        if trace.lifecycle() != SessionLifecycle::Running
            || trace.revision() != Revision::new(1)
            || trace.attempts().len() != 1
            || metadata.len() != 1
            || !initial_attempt.is_some_and(|attempt| {
                attempt.lifecycle() == AttemptLifecycle::Active
                    && attempt.validity() == AttemptValidityV2::Valid
                    && attempt.number() == AttemptNumberV2::FIRST
                    && attempt.trace() == TraceSequenceV2::FIRST
                    && metadata[0].attempt_id() == attempt.attempt_id()
            })
        {
            return Err(invalid(
                "initial Procedure v2 workflow memory requires one fresh active attempt",
            ));
        }
        let model = SnapshotMemoryModelV2::from_snapshot(snapshot)?;
        let mut attempts = Vec::with_capacity(trace.attempts().len());
        for (attempt, metadata) in trace.attempts().iter().zip(metadata) {
            let node = model.node(attempt.graph_node_id())?;
            let slots = node
                .items
                .iter()
                .map(|item| {
                    ItemSlotStateV2::new(
                        attempt.attempt_id().clone(),
                        item.id().clone(),
                        item.item_type(),
                        Revision::ZERO,
                        None,
                        metadata.started_at(),
                        metadata.started_at(),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let evidence = node
                .evidence
                .iter()
                .enumerate()
                .map(|(ordinal, reference)| {
                    if reference.required {
                        return Err(invalid(
                            "required Procedure v2 evidence is unresolved at initial activation",
                        ));
                    }
                    EvidenceResolutionStateV2::new(
                        u32::try_from(ordinal).map_err(|_| {
                            invalid("Procedure v2 evidence reference ordinal is out of bounds")
                        })?,
                        false,
                        reference.selected_item_ids.clone(),
                        ResolvedEvidenceReferenceV2::unresolved(reference.source_node.clone()),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            attempts.push(AttemptWorkflowMemoryV2::new(
                attempt.attempt_id().clone(),
                slots,
                Vec::new(),
                evidence,
            )?);
        }
        Self::new(attempts, Vec::new(), Vec::new())
    }

    pub(crate) fn selected_readback(
        &self,
        trace: &SessionTraceV2,
        attempt_id: &AttemptId,
    ) -> Result<Vec<EvidenceReadbackV2>, StoreValueErrorV1> {
        let consumer = self
            .attempts
            .iter()
            .find(|memory| memory.attempt_id() == attempt_id)
            .ok_or_else(|| invalid("Procedure v2 evidence consumer is absent"))?;
        let consumer_attempt = trace
            .attempts()
            .iter()
            .find(|attempt| attempt.attempt_id() == attempt_id)
            .ok_or_else(|| invalid("Procedure v2 evidence consumer trace is absent"))?;
        consumer
            .evidence()
            .iter()
            .map(|reference| {
                let source_attempt_id = reference
                    .resolution()
                    .snapshot()
                    .map(EvidenceReferenceSnapshotV2::source_attempt_id);
                let items = match reference.resolution() {
                    ResolvedEvidenceReferenceV2::Resolved(snapshot) => {
                        let source = self
                            .attempts
                            .iter()
                            .find(|memory| memory.attempt_id() == snapshot.source_attempt_id())
                            .ok_or_else(|| invalid("Procedure v2 evidence source is absent"))?;
                        let selected = if reference.selected_item_ids().is_empty() {
                            source
                                .item_slots()
                                .iter()
                                .filter_map(|slot| {
                                    slot.value().cloned().map(|value| {
                                        RecordedItemV2::new(slot.item_id().clone(), value)
                                    })
                                })
                                .collect()
                        } else {
                            source
                                .item_slots()
                                .iter()
                                .filter(|slot| {
                                    reference.selected_item_ids().contains(slot.item_id())
                                })
                                .filter_map(|slot| {
                                    slot.value().cloned().map(|value| {
                                        RecordedItemV2::new(slot.item_id().clone(), value)
                                    })
                                })
                                .collect()
                        };
                        RecordedItemSetV2::new(selected)
                            .map_err(|_| invalid("Procedure v2 selected read-back is invalid"))?
                    }
                    ResolvedEvidenceReferenceV2::Skipped(_)
                    | ResolvedEvidenceReferenceV2::Unresolved { .. } => {
                        RecordedItemSetV2::new(Vec::new())
                            .map_err(|_| invalid("Procedure v2 empty read-back is invalid"))?
                    }
                };
                let decision = source_attempt_id.and_then(|source_attempt_id| {
                    self.decisions
                        .iter()
                        .find(|record| record.attempt_id() == source_attempt_id)
                        .cloned()
                });
                let stale = source_attempt_id
                    .map(|source_attempt_id| {
                        let source_attempt = trace
                            .attempts()
                            .iter()
                            .find(|attempt| attempt.attempt_id() == source_attempt_id)
                            .ok_or_else(|| {
                                invalid("Procedure v2 evidence source trace is absent")
                            })?;
                        Ok(consumer_attempt.validity() == AttemptValidityV2::Stale
                            || source_attempt.validity() == AttemptValidityV2::Stale)
                    })
                    .transpose()?
                    .unwrap_or(false);
                Ok(EvidenceReadbackV2 {
                    reference: reference.clone(),
                    items,
                    decision,
                    stale,
                })
            })
            .collect()
    }
}

fn mutate_item_value_v2(
    specification: &ItemSpecV2,
    current: Option<&RecordedItemValueV2>,
    mutation: ActiveItemMutationV2,
) -> Result<Option<RecordedItemValueV2>, GraphMutationErrorV2> {
    let candidate = match mutation {
        ActiveItemMutationV2::Check => {
            if specification.item_type() != ItemTypeV1::Confirm {
                return Err(GraphMutationErrorV2::ItemTypeMismatch);
            }
            Some(RecordedItemValueV2::confirm())
        }
        ActiveItemMutationV2::Uncheck => {
            if specification.item_type() != ItemTypeV1::Confirm {
                return Err(GraphMutationErrorV2::ItemTypeMismatch);
            }
            None
        }
        ActiveItemMutationV2::Set { value } => Some(match specification.item_type() {
            ItemTypeV1::Text => RecordedItemValueV2::text(value)
                .map_err(|_| GraphMutationErrorV2::ItemConstraintFailed)?,
            ItemTypeV1::Choice => RecordedItemValueV2::choice(value)
                .map_err(|_| GraphMutationErrorV2::ItemConstraintFailed)?,
            ItemTypeV1::Integer => RecordedItemValueV2::integer(
                value
                    .parse::<i64>()
                    .map_err(|_| GraphMutationErrorV2::ItemConstraintFailed)?,
            ),
            ItemTypeV1::Confirm | ItemTypeV1::List | ItemTypeV1::Artifact => {
                return Err(GraphMutationErrorV2::ItemTypeMismatch);
            }
        }),
        ActiveItemMutationV2::Add { value } => {
            let ItemSpecV2::List(list_specification) = specification else {
                return Err(GraphMutationErrorV2::ItemTypeMismatch);
            };
            let mut values = current
                .and_then(RecordedItemValueV2::as_list)
                .map_or_else(Vec::new, ToOwned::to_owned);
            if list_specification.unique() && values.contains(&value) {
                return Err(GraphMutationErrorV2::ListValueDuplicate);
            }
            values.push(value);
            Some(
                RecordedItemValueV2::list(values)
                    .map_err(|_| GraphMutationErrorV2::ItemConstraintFailed)?,
            )
        }
        ActiveItemMutationV2::Remove {
            value,
            ignore_missing,
        } => {
            let ItemSpecV2::List(_) = specification else {
                return Err(GraphMutationErrorV2::ItemTypeMismatch);
            };
            let mut values = current
                .and_then(RecordedItemValueV2::as_list)
                .map_or_else(Vec::new, ToOwned::to_owned);
            let Some(index) = values.iter().position(|candidate| candidate == &value) else {
                if ignore_missing {
                    return Ok(current.cloned());
                }
                return Err(GraphMutationErrorV2::ListValueNotFound);
            };
            values.remove(index);
            if values.is_empty() {
                None
            } else {
                Some(
                    RecordedItemValueV2::list(values)
                        .map_err(|_| GraphMutationErrorV2::ItemConstraintFailed)?,
                )
            }
        }
        ActiveItemMutationV2::Attach { value } => {
            if specification.item_type() != ItemTypeV1::Artifact {
                return Err(GraphMutationErrorV2::ItemTypeMismatch);
            }
            Some(RecordedItemValueV2::artifact(value))
        }
        ActiveItemMutationV2::Clear => None,
    };
    if candidate
        .as_ref()
        .is_some_and(|value| !specification.admits_recorded_value(value))
    {
        return Err(GraphMutationErrorV2::ItemConstraintFailed);
    }
    Ok(candidate)
}

fn resolve_evidence_at_activation_v2(
    specification: &NodeMemorySpecV2,
    trace: &SessionTraceV2,
    memory: &WorkflowMemoryStateV2,
    resolved_at: UnixMillis,
) -> Result<Vec<EvidenceResolutionStateV2>, GraphMutationErrorV2> {
    specification
        .evidence
        .iter()
        .enumerate()
        .map(|(ordinal, reference)| {
            let mut sources = trace.attempts().iter().filter(|attempt| {
                attempt.graph_node_id() == &reference.source_node
                    && attempt.validity() == AttemptValidityV2::Valid
            });
            let source = sources.next();
            if sources.next().is_some() {
                return Err(invalid(
                    "Procedure v2 evidence source is not uniquely current at activation",
                )
                .into());
            }
            let resolution = match source {
                None if reference.required => {
                    return Err(invalid(
                        "required Procedure v2 evidence is unresolved at activation",
                    )
                    .into());
                }
                None => ResolvedEvidenceReferenceV2::unresolved(reference.source_node.clone()),
                Some(source)
                    if !matches!(
                        source.lifecycle(),
                        AttemptLifecycle::Completed | AttemptLifecycle::Skipped
                    ) =>
                {
                    return Err(invalid(
                        "Procedure v2 evidence source is not terminal at activation",
                    )
                    .into());
                }
                Some(source) => {
                    let source_memory = memory
                        .attempts()
                        .iter()
                        .find(|candidate| candidate.attempt_id() == source.attempt_id())
                        .ok_or_else(|| invalid("Procedure v2 evidence source memory is absent"))?;
                    let snapshot = EvidenceReferenceSnapshotV2::new(
                        reference.source_node.clone(),
                        source.attempt_id().clone(),
                        source.number(),
                        source_memory.recorded_items_digest()?,
                        resolved_at,
                    )
                    .map_err(GraphMutationErrorV2::Domain)?;
                    if source.lifecycle() == AttemptLifecycle::Skipped {
                        ResolvedEvidenceReferenceV2::skipped(snapshot)
                    } else {
                        ResolvedEvidenceReferenceV2::resolved(snapshot)
                    }
                }
            };
            EvidenceResolutionStateV2::new(
                u32::try_from(ordinal).map_err(|_| {
                    invalid("Procedure v2 evidence reference ordinal is out of bounds")
                })?,
                reference.required,
                reference.selected_item_ids.clone(),
                resolution,
            )
            .map_err(Into::into)
        })
        .collect()
}

/// SHA-256 over canonical JSON of complete recorded values in Procedure author order.
pub fn recorded_items_digest_v2(
    items: &RecordedItemSetV2,
) -> Result<Sha256Digest, StoreValueErrorV1> {
    let canonical = canonical_recorded_items_json_v2(items)?;
    Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
        .map_err(|_| invalid("Procedure v2 recorded item digest is invalid"))
}

pub fn canonical_recorded_items_json_v2(
    items: &RecordedItemSetV2,
) -> Result<String, StoreValueErrorV1> {
    let values = items
        .items()
        .iter()
        .map(|item| {
            Ok(json!({"id": item.id().as_str(), "value": item_value_json_v2(item.value())?}))
        })
        .collect::<Result<Vec<_>, StoreValueErrorV1>>()?;
    canonicalize_json_v1(&values)
        .map_err(|_| invalid("Procedure v2 recorded items are not canonicalizable"))
}

#[derive(Clone, Debug)]
struct EvidenceSpecV2 {
    source_node: GraphNodeId,
    required: bool,
    selected_item_ids: Vec<ItemId>,
}

#[derive(Clone, Debug)]
struct NodeMemorySpecV2 {
    node_kind: podway_core::NodeKindV2,
    node_definition_id: podway_core::NodeDefinitionId,
    items: Vec<ItemSpecV2>,
    evidence: Vec<EvidenceSpecV2>,
    advance_target: Option<GraphNodeId>,
    routes: BTreeMap<OptionId, (GraphNodeId, TransitionEffectV2)>,
}

#[derive(Clone, Debug)]
struct SnapshotMemoryModelV2 {
    nodes: BTreeMap<GraphNodeId, NodeMemorySpecV2>,
    manual_rework_targets: BTreeSet<GraphNodeId>,
}

impl SnapshotMemoryModelV2 {
    fn from_snapshot(snapshot: &ProcedureSnapshotV2) -> Result<Self, StoreValueErrorV1> {
        let document: Value = serde_json::from_str(snapshot.canonical_json().as_str())
            .map_err(|_| invalid("Procedure v2 snapshot memory metadata is invalid"))?;
        let root = document
            .as_object()
            .ok_or_else(|| invalid("Procedure v2 snapshot memory metadata is invalid"))?;
        let definitions = root
            .get("node_definitions")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("Procedure v2 node definitions are missing"))?;
        let mut nodes = BTreeMap::new();
        for node in snapshot.graph_nodes() {
            let placement: Value = serde_json::from_str(node.canonical_placement_json())
                .map_err(|_| invalid("Procedure v2 placement memory metadata is invalid"))?;
            let placement = placement
                .as_object()
                .ok_or_else(|| invalid("Procedure v2 placement memory metadata is invalid"))?;
            let definition = definitions
                .get(node.node_definition_id().as_str())
                .and_then(Value::as_object)
                .ok_or_else(|| invalid("Procedure v2 memory definition is absent"))?;
            let items = definition.get("items").and_then(Value::as_array).map_or(
                Ok(Vec::new()),
                |items| {
                    items
                        .iter()
                        .map(|item| {
                            let item = item.as_object().ok_or_else(|| {
                                invalid("Procedure v2 item definition is invalid")
                            })?;
                            parse_item_spec_v2(item)
                        })
                        .collect()
                },
            )?;
            let evidence = placement
                .get("evidence_from")
                .and_then(Value::as_array)
                .map_or(Ok(Vec::new()), |references| {
                    references
                        .iter()
                        .map(|reference| {
                            let reference = reference.as_object().ok_or_else(|| invalid("Procedure v2 evidence declaration is invalid"))?;
                            let selected_item_ids = reference
                                .get("items")
                                .and_then(Value::as_array)
                                .map_or(Ok(Vec::new()), |items| {
                                    items.iter().map(|item| {
                                        ItemId::new(item.as_str().ok_or_else(|| invalid("Procedure v2 evidence selector is invalid"))?.to_owned())
                                            .map_err(|_| invalid("Procedure v2 evidence selector is invalid"))
                                    }).collect()
                                })?;
                            Ok(EvidenceSpecV2 {
                                source_node: GraphNodeId::new(required_text(reference, "node")?.to_owned())
                                    .map_err(|_| invalid("Procedure v2 evidence source is invalid"))?,
                                required: reference.get("required").and_then(Value::as_bool).unwrap_or(true),
                                selected_item_ids,
                            })
                        })
                        .collect()
                })?;
            let routes =
                placement.get("routes").and_then(Value::as_object).map_or(
                    Ok(BTreeMap::new()),
                    |routes| {
                        routes
                            .iter()
                            .map(|(option, route)| {
                                let route = route.as_object().ok_or_else(|| {
                                    invalid("Procedure v2 decision route is invalid")
                                })?;
                                Ok((
                                    OptionId::new(option.clone()).map_err(|_| {
                                        invalid("Procedure v2 option identity is invalid")
                                    })?,
                                    (
                                        GraphNodeId::new(required_text(route, "to")?.to_owned())
                                            .map_err(|_| {
                                                invalid("Procedure v2 route target is invalid")
                                            })?,
                                        TransitionEffectV2::from_str(required_text(
                                            route, "effect",
                                        )?)
                                        .map_err(
                                            |_| invalid("Procedure v2 route effect is invalid"),
                                        )?,
                                    ),
                                ))
                            })
                            .collect()
                    },
                )?;
            let advance_target = placement
                .get("next")
                .and_then(Value::as_str)
                .map(|target| {
                    GraphNodeId::new(target.to_owned())
                        .map_err(|_| invalid("Procedure v2 action target is invalid"))
                })
                .transpose()?;
            nodes.insert(
                node.graph_node_id().clone(),
                NodeMemorySpecV2 {
                    node_kind: node.node_kind(),
                    node_definition_id: node.node_definition_id().clone(),
                    items,
                    evidence,
                    advance_target,
                    routes,
                },
            );
        }
        let manual_rework_targets = root
            .get("manual_rework")
            .and_then(Value::as_object)
            .and_then(|value| value.get("allowed_targets"))
            .and_then(Value::as_array)
            .map_or(Ok(BTreeSet::new()), |targets| {
                targets
                    .iter()
                    .map(|target| {
                        GraphNodeId::new(
                            target
                                .as_str()
                                .ok_or_else(|| {
                                    invalid("Procedure v2 manual rework target is invalid")
                                })?
                                .to_owned(),
                        )
                        .map_err(|_| invalid("Procedure v2 manual rework target is invalid"))
                    })
                    .collect()
            })?;
        Ok(Self {
            nodes,
            manual_rework_targets,
        })
    }

    fn node(&self, id: &GraphNodeId) -> Result<&NodeMemorySpecV2, StoreValueErrorV1> {
        self.nodes
            .get(id)
            .ok_or_else(|| invalid("Procedure v2 workflow-memory node is absent"))
    }
}

fn parse_item_spec_v2(
    item: &serde_json::Map<String, Value>,
) -> Result<ItemSpecV2, StoreValueErrorV1> {
    let common = ItemCommonV2::new(
        ItemId::new(required_text(item, "id")?.to_owned())
            .map_err(|_| invalid("Procedure v2 item identity is invalid"))?,
        required_text(item, "prompt")?,
        item.get("help")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        item.get("required")
            .and_then(Value::as_bool)
            .ok_or_else(|| invalid("Procedure v2 item requirement is invalid"))?,
    )
    .map_err(|_| invalid("Procedure v2 item declaration is invalid"))?;
    match parse_item_type(required_text(item, "type")?)? {
        ItemTypeV1::Confirm => Ok(ItemSpecV2::confirm(common)),
        ItemTypeV1::Text => ItemSpecV2::text(
            common,
            optional_u32(item, "min_length", 0)?,
            optional_u32(item, "max_length", 4_000)?,
            item.get("multiline")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        ),
        ItemTypeV1::Choice => ItemSpecV2::choice(
            common,
            item.get("choices")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("Procedure v2 choice declaration is invalid"))?
                .iter()
                .map(|choice| {
                    choice
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| invalid("Procedure v2 choice declaration is invalid"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ItemTypeV1::Integer => ItemSpecV2::integer(
            common,
            item.get("minimum").and_then(Value::as_i64),
            item.get("maximum").and_then(Value::as_i64),
        ),
        ItemTypeV1::List => ItemSpecV2::list(
            common,
            optional_u16(item, "min_items", 0)?,
            optional_u16(item, "max_items", 50)?,
            optional_u16(item, "max_item_length", 500)?,
            item.get("unique").and_then(Value::as_bool).unwrap_or(true),
        ),
        ItemTypeV1::Artifact => ItemSpecV2::artifact(
            common,
            item.get("allowed_media_types")
                .and_then(Value::as_array)
                .map_or(Ok(Vec::new()), |media_types| {
                    media_types
                        .iter()
                        .map(|media_type| {
                            media_type.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                                invalid("Procedure v2 artifact media type is invalid")
                            })
                        })
                        .collect()
                })?,
        ),
    }
    .map_err(|_| invalid("Procedure v2 item declaration is invalid"))
}

fn optional_u32(
    object: &serde_json::Map<String, Value>,
    key: &'static str,
    default: u32,
) -> Result<u32, StoreValueErrorV1> {
    object.get(key).map_or(Ok(default), |value| {
        value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| invalid("Procedure v2 item constraint is invalid"))
    })
}

fn optional_u16(
    object: &serde_json::Map<String, Value>,
    key: &'static str,
    default: u16,
) -> Result<u16, StoreValueErrorV1> {
    object.get(key).map_or(Ok(default), |value| {
        value
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| invalid("Procedure v2 item constraint is invalid"))
    })
}

fn required_text<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, StoreValueErrorV1> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("Procedure v2 memory metadata is incomplete"))
}

fn parse_item_type(value: &str) -> Result<ItemTypeV1, StoreValueErrorV1> {
    match value {
        "confirm" => Ok(ItemTypeV1::Confirm),
        "text" => Ok(ItemTypeV1::Text),
        "choice" => Ok(ItemTypeV1::Choice),
        "integer" => Ok(ItemTypeV1::Integer),
        "list" => Ok(ItemTypeV1::List),
        "artifact" => Ok(ItemTypeV1::Artifact),
        _ => Err(invalid("Procedure v2 item type is invalid")),
    }
}

fn item_type_text(value: ItemTypeV1) -> &'static str {
    match value {
        ItemTypeV1::Confirm => "confirm",
        ItemTypeV1::Text => "text",
        ItemTypeV1::Choice => "choice",
        ItemTypeV1::Integer => "integer",
        ItemTypeV1::List => "list",
        ItemTypeV1::Artifact => "artifact",
    }
}

fn blocker_state_text(value: BlockerState) -> &'static str {
    match value {
        BlockerState::Open => "open",
        BlockerState::Resolved => "resolved",
    }
}

fn item_value_json_v2(value: &RecordedItemValueV2) -> Result<Value, StoreValueErrorV1> {
    Ok(match value.item_type() {
        ItemTypeV1::Confirm => json!({"kind":"confirm"}),
        ItemTypeV1::Text => {
            json!({"kind":"text","value":value.as_text().ok_or_else(|| invalid("Procedure v2 text item value is invalid"))?})
        }
        ItemTypeV1::Choice => {
            json!({"kind":"choice","value":value.as_choice().ok_or_else(|| invalid("Procedure v2 choice item value is invalid"))?})
        }
        ItemTypeV1::Integer => {
            json!({"kind":"integer","value":value.as_integer().ok_or_else(|| invalid("Procedure v2 integer item value is invalid"))?})
        }
        ItemTypeV1::List => {
            json!({"kind":"list","value":value.as_list().ok_or_else(|| invalid("Procedure v2 list item value is invalid"))?})
        }
        ItemTypeV1::Artifact => {
            let artifact = value
                .as_artifact()
                .ok_or_else(|| invalid("Procedure v2 artifact item value is invalid"))?;
            json!({
                "kind":"artifact",
                "location_kind": match artifact.location_kind() {
                    ArtifactLocationKindV1::LocalPath => "local_path",
                    ArtifactLocationKindV1::ExternalReference => "external_reference",
                },
                "location":artifact.location(),
                "digest":artifact.digest().as_str(),
                "size_bytes":artifact.size_bytes(),
                "media_type":artifact.media_type(),
            })
        }
    })
}

fn encode_item_value_v2(value: &RecordedItemValueV2) -> Result<String, StoreErrorV1> {
    canonicalize_json_v1(&item_value_json_v2(value).map_err(StoreErrorV1::InvalidStateV1)?)
        .map_err(|_| corrupt(StoreRecordKindV1::Item))
}

fn decode_item_value_v2(
    encoded: &str,
    expected: ItemTypeV1,
) -> Result<RecordedItemValueV2, StoreErrorV1> {
    verify_canonical_json_v1(encoded.as_bytes()).map_err(|_| corrupt(StoreRecordKindV1::Item))?;
    let value: Value =
        serde_json::from_str(encoded).map_err(|_| corrupt(StoreRecordKindV1::Item))?;
    let object = value
        .as_object()
        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
    if parse_item_type(kind).map_err(|_| corrupt(StoreRecordKindV1::Item))? != expected {
        return Err(corrupt(StoreRecordKindV1::Item));
    }
    let decoded = match kind {
        "confirm" if exact_keys(object, &["kind"]) => RecordedItemValueV2::confirm(),
        "text" if exact_keys(object, &["kind", "value"]) => {
            RecordedItemValueV2::text(required_json_text(object, "value")?)
                .map_err(|_| corrupt(StoreRecordKindV1::Item))?
        }
        "choice" if exact_keys(object, &["kind", "value"]) => {
            RecordedItemValueV2::choice(required_json_text(object, "value")?)
                .map_err(|_| corrupt(StoreRecordKindV1::Item))?
        }
        "integer" if exact_keys(object, &["kind", "value"]) => RecordedItemValueV2::integer(
            object
                .get("value")
                .and_then(Value::as_i64)
                .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?,
        ),
        "list" if exact_keys(object, &["kind", "value"]) => RecordedItemValueV2::list(
            object
                .get("value")
                .and_then(Value::as_array)
                .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
        "artifact"
            if exact_keys(
                object,
                &[
                    "kind",
                    "location_kind",
                    "location",
                    "digest",
                    "size_bytes",
                    "media_type",
                ],
            ) =>
        {
            let location = required_json_text(object, "location")?;
            let digest = Sha256Digest::new(required_json_text(object, "digest")?)
                .map_err(|_| corrupt(StoreRecordKindV1::Item))?;
            let size = object
                .get("size_bytes")
                .and_then(Value::as_u64)
                .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
            let media_type = required_json_text(object, "media_type")?;
            let artifact = match object.get("location_kind").and_then(Value::as_str) {
                Some("local_path") => {
                    ArtifactValueV1::local_path(location, digest, size, media_type)
                }
                Some("external_reference") => {
                    ArtifactValueV1::external_reference(location, digest, size, media_type)
                }
                _ => return Err(corrupt(StoreRecordKindV1::Item)),
            }
            .map_err(|_| corrupt(StoreRecordKindV1::Item))?;
            RecordedItemValueV2::artifact(artifact)
        }
        _ => return Err(corrupt(StoreRecordKindV1::Item)),
    };
    if encode_item_value_v2(&decoded)? != encoded {
        return Err(corrupt(StoreRecordKindV1::Item));
    }
    Ok(decoded)
}

fn exact_keys(object: &serde_json::Map<String, Value>, keys: &[&str]) -> bool {
    object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
}

fn required_json_text(
    object: &serde_json::Map<String, Value>,
    key: &'static str,
) -> Result<String, StoreErrorV1> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| corrupt(StoreRecordKindV1::Item))
}

pub(crate) fn validate_workflow_memory_v2(
    snapshot: &ProcedureSnapshotV2,
    trace: &SessionTraceV2,
    metadata: &[AttemptMetadataV2],
    memory: &WorkflowMemoryStateV2,
    goal_rework_target_attempt_ids: &BTreeSet<AttemptId>,
) -> Result<(), StoreValueErrorV1> {
    if memory.attempts().len() != trace.attempts().len() || metadata.len() != trace.attempts().len()
    {
        return Err(invalid("Procedure v2 workflow memory is incomplete"));
    }
    let model = SnapshotMemoryModelV2::from_snapshot(snapshot)?;
    let attempts_by_id: BTreeMap<_, _> = trace
        .attempts()
        .iter()
        .map(|attempt| (attempt.attempt_id(), attempt))
        .collect();
    let metadata_by_id: BTreeMap<_, _> = metadata
        .iter()
        .map(|metadata| (metadata.attempt_id(), metadata))
        .collect();
    let memory_by_id: BTreeMap<_, _> = memory
        .attempts()
        .iter()
        .map(|memory| (memory.attempt_id(), memory))
        .collect();
    let mut blocker_ids = BTreeSet::new();

    for ((attempt, attempt_metadata), attempt_memory) in
        trace.attempts().iter().zip(metadata).zip(memory.attempts())
    {
        if attempt.attempt_id() != attempt_metadata.attempt_id()
            || attempt.attempt_id() != attempt_memory.attempt_id()
        {
            return Err(invalid(
                "Procedure v2 workflow memory order is inconsistent",
            ));
        }
        let specification = model.node(attempt.graph_node_id())?;
        if attempt_memory.item_slots().len() != specification.items.len()
            || attempt_memory
                .item_slots()
                .iter()
                .zip(&specification.items)
                .any(|(slot, item)| {
                    slot.item_id() != item.id()
                        || slot.item_type() != item.item_type()
                        || slot.attempt_id() != attempt.attempt_id()
                })
        {
            return Err(invalid(
                "Procedure v2 item slots do not match the immutable definition",
            ));
        }
        for (slot, item) in attempt_memory.item_slots().iter().zip(&specification.items) {
            if slot.created_at() != attempt_metadata.started_at()
                || slot.updated_at() < attempt_metadata.started_at()
                || attempt_metadata
                    .ended_at()
                    .is_some_and(|ended| slot.updated_at() > ended)
                || (slot.revision() == Revision::ZERO
                    && (slot.created_at() != attempt_metadata.started_at()
                        || slot.updated_at() != attempt_metadata.started_at()))
                || (slot.revision() == Revision::new(1) && slot.value().is_none())
            {
                return Err(invalid(
                    "Procedure v2 item slot timestamps are inconsistent",
                ));
            }
            if slot
                .value()
                .is_some_and(|value| !item.admits_recorded_value(value))
            {
                return Err(invalid(
                    "Procedure v2 item value violates its immutable declaration",
                ));
            }
        }
        if attempt.lifecycle() == AttemptLifecycle::Completed
            && attempt_memory
                .item_slots()
                .iter()
                .zip(&specification.items)
                .any(|(slot, item)| item.common().required() && slot.value().is_none())
        {
            return Err(invalid(
                "completed Procedure v2 attempt is missing a required item",
            ));
        }
        if attempt.lifecycle() == AttemptLifecycle::Skipped
            && attempt_memory
                .item_slots()
                .iter()
                .any(|slot| slot.value().is_some())
        {
            return Err(invalid(
                "skipped Procedure v2 attempts cannot retain recorded item values",
            ));
        }
        let mut prior_blocker: Option<(UnixMillis, &BlockerId)> = None;
        for blocker in attempt_memory.blockers() {
            if !blocker_ids.insert(blocker.blocker_id())
                || blocker.created_at() < attempt_metadata.started_at()
                || attempt_metadata.ended_at().is_some_and(|ended| {
                    blocker.created_at() > ended
                        || blocker
                            .resolved_at()
                            .is_some_and(|resolved| resolved > ended)
                })
            {
                return Err(invalid("Procedure v2 blocker history is inconsistent"));
            }
            let current = (blocker.created_at(), blocker.blocker_id());
            if prior_blocker.is_some_and(|prior| prior >= current) {
                return Err(invalid("Procedure v2 blockers are not canonically ordered"));
            }
            prior_blocker = Some(current);
        }
        if attempt.lifecycle() == AttemptLifecycle::Completed
            && attempt_memory
                .blockers()
                .iter()
                .any(|blocker| blocker.state() == BlockerState::Open)
        {
            return Err(invalid(
                "completed Procedure v2 attempts cannot retain open blockers",
            ));
        }
        validate_attempt_evidence_v2(
            attempt,
            attempt_memory,
            specification,
            attempt_metadata.started_at(),
            &attempts_by_id,
            &memory_by_id,
        )?;
    }

    validate_decision_history_v2(snapshot, trace, metadata, memory, &model)?;
    validate_rework_history_v2(
        trace,
        metadata,
        memory,
        &model,
        goal_rework_target_attempt_ids,
    )?;
    let _ = metadata_by_id;
    Ok(())
}

fn validate_attempt_evidence_v2<'a>(
    consumer: &SessionAttemptV2,
    consumer_memory: &AttemptWorkflowMemoryV2,
    specification: &NodeMemorySpecV2,
    consumer_started_at: UnixMillis,
    attempts_by_id: &BTreeMap<&'a AttemptId, &'a SessionAttemptV2>,
    memory_by_id: &BTreeMap<&'a AttemptId, &'a AttemptWorkflowMemoryV2>,
) -> Result<(), StoreValueErrorV1> {
    if consumer_memory.evidence().len() != specification.evidence.len() {
        return Err(invalid(
            "Procedure v2 evidence snapshots do not match the immutable placement",
        ));
    }
    for ((index, persisted), declared) in consumer_memory
        .evidence()
        .iter()
        .enumerate()
        .zip(&specification.evidence)
    {
        if persisted.ordinal() != u32::try_from(index).unwrap_or(u32::MAX)
            || persisted.required() != declared.required
            || persisted.selected_item_ids() != declared.selected_item_ids
            || persisted.resolution().source_node() != &declared.source_node
        {
            return Err(invalid(
                "Procedure v2 evidence snapshot declaration metadata changed",
            ));
        }
        let Some(reference) = persisted.resolution().snapshot() else {
            if persisted.required() {
                return Err(invalid("required Procedure v2 evidence is unresolved"));
            }
            if attempts_by_id.values().any(|source| {
                source.graph_node_id() == &declared.source_node
                    && source.trace() < consumer.trace()
                    && source.validity() == AttemptValidityV2::Valid
                    && matches!(
                        source.lifecycle(),
                        AttemptLifecycle::Completed | AttemptLifecycle::Skipped
                    )
            }) {
                return Err(invalid(
                    "optional Procedure v2 evidence is unresolved despite a valid source",
                ));
            }
            continue;
        };
        let source = attempts_by_id
            .get(reference.source_attempt_id())
            .copied()
            .ok_or_else(|| invalid("Procedure v2 evidence source attempt is absent"))?;
        let source_memory = memory_by_id
            .get(reference.source_attempt_id())
            .copied()
            .ok_or_else(|| invalid("Procedure v2 evidence source memory is absent"))?;
        if source.graph_node_id() != reference.source_node()
            || source.number() != reference.source_attempt_number()
            || source.trace() >= consumer.trace()
            || source.lifecycle() == AttemptLifecycle::Active
            || reference.resolved_at() != consumer_started_at
            || source_memory.recorded_items_digest()? != *reference.items_digest()
        {
            return Err(invalid(
                "Procedure v2 evidence source snapshot is inconsistent",
            ));
        }
        match persisted.resolution() {
            ResolvedEvidenceReferenceV2::Resolved(_) => {
                if source.lifecycle() != AttemptLifecycle::Completed {
                    return Err(invalid(
                        "resolved Procedure v2 evidence source is not completed",
                    ));
                }
            }
            ResolvedEvidenceReferenceV2::Skipped(_) => {
                if persisted.required()
                    || source.lifecycle() != AttemptLifecycle::Skipped
                    || !source_memory.recorded_items()?.items().is_empty()
                {
                    return Err(invalid(
                        "skipped Procedure v2 evidence source is inconsistent",
                    ));
                }
            }
            ResolvedEvidenceReferenceV2::Unresolved { .. } => unreachable!(),
        }
        if consumer.validity() == AttemptValidityV2::Valid
            && source.validity() != AttemptValidityV2::Valid
        {
            return Err(invalid("a valid Procedure v2 attempt holds stale evidence"));
        }
    }
    Ok(())
}

fn validate_decision_history_v2(
    snapshot: &ProcedureSnapshotV2,
    trace: &SessionTraceV2,
    metadata: &[AttemptMetadataV2],
    memory: &WorkflowMemoryStateV2,
    model: &SnapshotMemoryModelV2,
) -> Result<(), StoreValueErrorV1> {
    let decisions: BTreeMap<_, _> = memory
        .decisions()
        .iter()
        .map(|record| (record.attempt_id(), record))
        .collect();
    let memory_by_id: BTreeMap<_, _> = memory
        .attempts()
        .iter()
        .map(|attempt| (attempt.attempt_id(), attempt))
        .collect();
    for (attempt, attempt_metadata) in trace.attempts().iter().zip(metadata) {
        let specification = model.node(attempt.graph_node_id())?;
        let record = decisions.get(attempt.attempt_id()).copied();
        let requires_record = specification.node_kind == podway_core::NodeKindV2::Decision
            && attempt.lifecycle() == AttemptLifecycle::Completed;
        if requires_record != record.is_some() {
            return Err(invalid("Procedure v2 decision history is incomplete"));
        }
        let Some(record) = record else { continue };
        let route = specification
            .routes
            .get(record.selected_option())
            .ok_or_else(|| invalid("Procedure v2 decision selected no declared route"))?;
        let expected_evidence = ResolvedEvidenceSetV2::new(
            memory_by_id[attempt.attempt_id()]
                .evidence()
                .iter()
                .map(|reference| reference.resolution().clone())
                .collect(),
        )
        .map_err(|_| invalid("Procedure v2 decision evidence is invalid"))?;
        if record.trace() != attempt.trace()
            || record.session_id() != trace.session_id()
            || record.session_revision() == Revision::ZERO
            || record.session_revision() > trace.revision()
            || record.procedure_snapshot_id() != snapshot.snapshot_id()
            || record.procedure_digest() != snapshot.digest()
            || record.graph_node_id() != attempt.graph_node_id()
            || record.node_definition_id() != &specification.node_definition_id
            || record.attempt_number() != attempt.number()
            || record.goal_revision() != attempt.goal_revision()
            || record.route_target() != &route.0
            || record.route_effect() != route.1
            || record.evidence() != &expected_evidence
            || attempt_metadata.ended_at() != Some(record.recorded_at())
        {
            return Err(invalid("Procedure v2 decision record is inconsistent"));
        }
    }
    Ok(())
}

fn validate_rework_history_v2(
    trace: &SessionTraceV2,
    metadata: &[AttemptMetadataV2],
    memory: &WorkflowMemoryStateV2,
    model: &SnapshotMemoryModelV2,
    goal_rework_target_attempt_ids: &BTreeSet<AttemptId>,
) -> Result<(), StoreValueErrorV1> {
    let attempts_by_trace: BTreeMap<_, _> = trace
        .attempts()
        .iter()
        .map(|attempt| (attempt.trace(), attempt))
        .collect();
    let metadata_by_id: BTreeMap<_, _> = metadata
        .iter()
        .map(|metadata| (metadata.attempt_id(), metadata))
        .collect();
    let decisions_by_attempt: BTreeMap<_, _> = memory
        .decisions()
        .iter()
        .map(|record| (record.attempt_id(), record))
        .collect();
    let rework_targets: BTreeSet<_> = memory
        .reworks()
        .iter()
        .map(ReworkRecordV2::target_attempt_id)
        .collect();
    for target_attempt_id in goal_rework_target_attempt_ids {
        let target = trace
            .attempts()
            .iter()
            .find(|attempt| attempt.attempt_id() == target_attempt_id)
            .ok_or_else(|| invalid("Procedure v2 goal rework target is absent"))?;
        if target.number() == AttemptNumberV2::FIRST || rework_targets.contains(target_attempt_id) {
            return Err(invalid(
                "Procedure v2 goal rework target cause is inconsistent",
            ));
        }
    }
    for attempt in trace.attempts() {
        if attempt.number() == AttemptNumberV2::FIRST {
            continue;
        }
        let predecessor = attempts_by_trace
            .get(&TraceSequenceV2::new(attempt.trace().get() - 1))
            .copied()
            .ok_or_else(|| invalid("Procedure v2 repeated attempt has no predecessor"))?;
        let retry = predecessor.graph_node_id() == attempt.graph_node_id()
            && predecessor.lifecycle() == AttemptLifecycle::Abandoned;
        let predecessor_specification = model.node(predecessor.graph_node_id())?;
        let ordinary_advance = match predecessor_specification.node_kind {
            podway_core::NodeKindV2::Action => {
                predecessor_specification.advance_target.as_ref() == Some(attempt.graph_node_id())
            }
            podway_core::NodeKindV2::Decision => decisions_by_attempt
                .get(predecessor.attempt_id())
                .is_some_and(|decision| {
                    decision.route_effect() == TransitionEffectV2::Advance
                        && decision.route_target() == attempt.graph_node_id()
                }),
        };
        if !retry
            && !ordinary_advance
            && !rework_targets.contains(attempt.attempt_id())
            && !goal_rework_target_attempt_ids.contains(attempt.attempt_id())
        {
            return Err(invalid("Procedure v2 rework history is incomplete"));
        }
    }
    for record in memory.reworks() {
        let target = attempts_by_trace
            .get(&record.trace())
            .copied()
            .ok_or_else(|| invalid("Procedure v2 rework target trace is absent"))?;
        let source_trace = TraceSequenceV2::new(
            record
                .trace()
                .get()
                .checked_sub(1)
                .ok_or_else(|| invalid("Procedure v2 rework trace is invalid"))?,
        );
        let source = attempts_by_trace
            .get(&source_trace)
            .copied()
            .ok_or_else(|| invalid("Procedure v2 rework source trace is absent"))?;
        if target.attempt_id() != record.target_attempt_id()
            || target.graph_node_id() != record.to_node()
            || source.graph_node_id() != record.from_node()
            || metadata_by_id[target.attempt_id()].started_at() != record.recorded_at()
        {
            return Err(invalid(
                "Procedure v2 rework record identity is inconsistent",
            ));
        }
        if !trace.attempts().iter().any(|attempt| {
            attempt.trace() < target.trace() && attempt.graph_node_id() == target.graph_node_id()
        }) {
            return Err(invalid("Procedure v2 rework target has no earlier attempt"));
        }
        match record.kind() {
            ReworkKindV2::Declared => {
                let decision = decisions_by_attempt
                    .get(source.attempt_id())
                    .copied()
                    .ok_or_else(|| invalid("declared Procedure v2 rework has no decision"))?;
                if record.reactivated()
                    || decision.route_effect() != TransitionEffectV2::Rework
                    || decision.route_target() != record.to_node()
                    || decision.reason() != record.reason()
                {
                    return Err(invalid(
                        "declared Procedure v2 rework record is inconsistent",
                    ));
                }
            }
            ReworkKindV2::Manual => {
                if !model.manual_rework_targets.contains(record.to_node()) {
                    return Err(invalid("manual Procedure v2 rework target is not declared"));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_workflow_memory_successor_v2(
    snapshot: &ProcedureSnapshotV2,
    previous_trace: &SessionTraceV2,
    previous: &WorkflowMemoryStateV2,
    next_trace: &SessionTraceV2,
    next: &WorkflowMemoryStateV2,
    goal_transition: WorkflowGoalTransitionV2<'_>,
) -> Result<(), StoreValueErrorV1> {
    if next.attempts().len() < previous.attempts().len()
        || next.attempts().len() > previous.attempts().len() + 1
        || next.decisions().len() < previous.decisions().len()
        || next.decisions().len() > previous.decisions().len() + 1
        || next.reworks().len() < previous.reworks().len()
        || next.reworks().len() > previous.reworks().len() + 1
        || next.decisions()[..previous.decisions().len()] != *previous.decisions()
        || next.reworks()[..previous.reworks().len()] != *previous.reworks()
    {
        return Err(invalid("Procedure v2 workflow history is not append-only"));
    }
    let model = SnapshotMemoryModelV2::from_snapshot(snapshot)?;
    let appended_decision = next.decisions().get(previous.decisions().len());
    if let Some(decision) = appended_decision {
        let source = previous_trace
            .active_attempt()
            .ok_or_else(|| invalid("new Procedure v2 decision has no active source attempt"))?;
        let next_source = next_trace
            .attempts()
            .iter()
            .find(|attempt| attempt.attempt_id() == source.attempt_id())
            .ok_or_else(|| invalid("new Procedure v2 decision source attempt is absent"))?;
        if decision.attempt_id() != source.attempt_id()
            || decision.session_revision() != next_trace.revision()
            || next_source.lifecycle() != AttemptLifecycle::Completed
        {
            return Err(invalid(
                "new Procedure v2 decision is not bound to its transition revision",
            ));
        }
    }
    let mut cursor_stable_changed = false;
    for index in 0..previous.attempts().len() {
        let old_attempt = &previous_trace.attempts()[index];
        let new_attempt = &next_trace.attempts()[index];
        let old_memory = &previous.attempts()[index];
        let new_memory = &next.attempts()[index];
        if old_attempt.lifecycle() != AttemptLifecycle::Active
            || new_attempt.lifecycle() != AttemptLifecycle::Active
        {
            if old_memory != new_memory {
                return Err(invalid(
                    "Procedure v2 terminal workflow history is immutable",
                ));
            }
            continue;
        }
        if old_memory.evidence() != new_memory.evidence()
            || old_memory.item_slots().len() != new_memory.item_slots().len()
            || old_memory.blockers().len() > new_memory.blockers().len()
        {
            return Err(invalid(
                "Procedure v2 active workflow-memory successor is invalid",
            ));
        }
        for (old, new) in old_memory.item_slots().iter().zip(new_memory.item_slots()) {
            if old.attempt_id() != new.attempt_id()
                || old.item_id() != new.item_id()
                || old.item_type() != new.item_type()
                || old.created_at() != new.created_at()
            {
                return Err(invalid("Procedure v2 item slot identity changed"));
            }
            if old != new {
                if new.revision()
                    != old
                        .revision()
                        .checked_next()
                        .map_err(|_| invalid("Procedure v2 item revision overflowed"))?
                    || new.updated_at() < old.updated_at()
                {
                    return Err(invalid("Procedure v2 item slot revision is not monotonic"));
                }
                cursor_stable_changed = true;
            }
        }
        for (old, new) in old_memory.blockers().iter().zip(new_memory.blockers()) {
            if old.blocker_id() != new.blocker_id()
                || old.attempt_id() != new.attempt_id()
                || old.reason() != new.reason()
                || old.created_at() != new.created_at()
                || !matches!(
                    (old.state(), new.state()),
                    (BlockerState::Open, BlockerState::Open)
                        | (BlockerState::Open, BlockerState::Resolved)
                        | (BlockerState::Resolved, BlockerState::Resolved)
                )
                || (old.state() == BlockerState::Resolved && old.resolved_at() != new.resolved_at())
            {
                return Err(invalid(
                    "Procedure v2 blocker history changed non-monotonically",
                ));
            }
            cursor_stable_changed |= old != new;
        }
        if new_memory.blockers()[old_memory.blockers().len()..]
            .iter()
            .any(|blocker| blocker.state() != BlockerState::Open)
        {
            return Err(invalid(
                "new Procedure v2 blockers must be appended in the open state",
            ));
        }
        cursor_stable_changed |= old_memory.blockers().len() != new_memory.blockers().len();
    }
    let cursor_stable = previous_trace.active_attempt().is_some_and(|old| {
        next_trace
            .active_attempt()
            .is_some_and(|new| new.attempt_id() == old.attempt_id())
    });
    let initial_goal_binding = match goal_transition {
        WorkflowGoalTransitionV2::InitialBinding { attempt_id } => {
            let old = previous_trace.active_attempt().ok_or_else(|| {
                invalid("Procedure v2 initial goal binding has no active attempt")
            })?;
            let new = next_trace.active_attempt().ok_or_else(|| {
                invalid("Procedure v2 initial goal binding has no successor active attempt")
            })?;
            if old.attempt_id() != attempt_id
                || new.attempt_id() != attempt_id
                || old.goal_revision().is_some()
                || new.goal_revision() != Some(GoalRevisionNumberV2::FIRST)
                || previous_trace.attempts().len() != next_trace.attempts().len()
                || cursor_stable_changed
                || previous.decisions() != next.decisions()
                || previous.reworks() != next.reworks()
            {
                return Err(invalid(
                    "Procedure v2 initial goal binding is not cursor-stable",
                ));
            }
            true
        }
        WorkflowGoalTransitionV2::None | WorkflowGoalTransitionV2::Rework { .. } => false,
    };
    if cursor_stable
        && !cursor_stable_changed
        && previous.decisions() == next.decisions()
        && previous.reworks() == next.reworks()
        && !initial_goal_binding
    {
        return Err(invalid(
            "Procedure v2 cursor-stable replacement changed no workflow memory",
        ));
    }
    if previous_trace.attempts().len() < next_trace.attempts().len() {
        let fresh = next
            .attempts()
            .last()
            .ok_or_else(|| invalid("fresh Procedure v2 workflow memory is absent"))?;
        if fresh
            .item_slots()
            .iter()
            .any(|slot| slot.revision() != Revision::ZERO || slot.value().is_some())
            || !fresh.blockers().is_empty()
        {
            return Err(invalid(
                "fresh Procedure v2 attempts must start with empty mutable memory",
            ));
        }
        let fresh_attempt = next_trace
            .attempts()
            .last()
            .ok_or_else(|| invalid("fresh Procedure v2 attempt is absent"))?;
        let source = previous_trace
            .active_attempt()
            .or_else(|| previous_trace.attempts().last())
            .ok_or_else(|| invalid("fresh Procedure v2 attempt has no source"))?;
        let next_source = next_trace
            .attempts()
            .iter()
            .find(|attempt| attempt.attempt_id() == source.attempt_id())
            .ok_or_else(|| invalid("fresh Procedure v2 attempt source is absent"))?;
        let source_specification = model.node(source.graph_node_id())?;
        let retry = source.graph_node_id() == fresh_attempt.graph_node_id()
            && next_source.lifecycle() == AttemptLifecycle::Abandoned;
        let ordinary_advance = match source_specification.node_kind {
            podway_core::NodeKindV2::Action => {
                source_specification.advance_target.as_ref() == Some(fresh_attempt.graph_node_id())
            }
            podway_core::NodeKindV2::Decision => next
                .decisions()
                .iter()
                .find(|record| record.attempt_id() == source.attempt_id())
                .is_some_and(|record| {
                    record.route_effect() == TransitionEffectV2::Advance
                        && record.route_target() == fresh_attempt.graph_node_id()
                }),
        };
        let appended_rework = next.reworks().get(previous.reworks().len());
        let prior_target = previous_trace.attempts().iter().find(|attempt| {
            attempt.graph_node_id() == fresh_attempt.graph_node_id()
                && attempt.validity() == AttemptValidityV2::Valid
        });
        let goal_rework = match goal_transition {
            WorkflowGoalTransitionV2::Rework { target_attempt_id } => {
                if target_attempt_id != fresh_attempt.attempt_id()
                    || appended_rework.is_some()
                    || prior_target.is_none()
                {
                    return Err(invalid(
                        "Procedure v2 goal rework is not bound to its fresh target",
                    ));
                }
                true
            }
            WorkflowGoalTransitionV2::None => false,
            WorkflowGoalTransitionV2::InitialBinding { .. } => {
                return Err(invalid(
                    "Procedure v2 initial goal binding cannot append an attempt",
                ));
            }
        };
        if let Some(record) = appended_rework {
            if prior_target.is_none()
                || record.target_attempt_id() != fresh_attempt.attempt_id()
                || record.trace() != fresh_attempt.trace()
                || record.from_node() != source.graph_node_id()
                || record.to_node() != fresh_attempt.graph_node_id()
                || record.reactivated()
                    != (previous_trace.lifecycle() == SessionLifecycle::Completed)
            {
                return Err(invalid(
                    "new Procedure v2 rework record is not bound to its transition",
                ));
            }
        } else if !retry && !ordinary_advance && !goal_rework {
            return Err(invalid("Procedure v2 rework transition has no record"));
        }
    } else if next.reworks().len() != previous.reworks().len() {
        return Err(invalid(
            "Procedure v2 rework history requires a fresh target attempt",
        ));
    } else if matches!(goal_transition, WorkflowGoalTransitionV2::Rework { .. }) {
        return Err(invalid(
            "Procedure v2 goal rework requires a fresh target attempt",
        ));
    }
    Ok(())
}

pub(crate) fn insert_workflow_memory_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    memory: &WorkflowMemoryStateV2,
) -> Result<(), StoreErrorV1> {
    for attempt in memory.attempts() {
        insert_attempt_memory_v2(transaction, attempt)?;
    }
    for decision in memory.decisions() {
        insert_decision_v2(transaction, decision)?;
    }
    for rework in memory.reworks() {
        insert_rework_v2(transaction, session_id, rework)?;
    }
    Ok(())
}

pub(crate) fn replace_workflow_memory_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    previous: &WorkflowMemoryStateV2,
    next: &WorkflowMemoryStateV2,
) -> Result<(), StoreErrorV1> {
    for (old_attempt, new_attempt) in previous.attempts().iter().zip(next.attempts()) {
        for (old, new) in old_attempt
            .item_slots()
            .iter()
            .zip(new_attempt.item_slots())
        {
            if old == new {
                continue;
            }
            let changed = transaction.execute(
                "UPDATE v2_item_slots SET item_revision = ?1, value_json = ?2, updated_at_ms = ?3 \
                 WHERE attempt_id = ?4 AND item_id = ?5 AND item_type = ?6 AND item_revision = ?7 \
                 AND value_json IS ?8 AND created_at_ms = ?9 AND updated_at_ms = ?10",
                params![
                    sqlite_u64(new.revision().get(), "Procedure v2 item revision")?,
                    new.value().map(encode_item_value_v2).transpose()?,
                    sqlite_u64(new.updated_at().get(), "Procedure v2 item update timestamp")?,
                    old.attempt_id().as_str(), old.item_id().as_str(), item_type_text(old.item_type()),
                    sqlite_u64(old.revision().get(), "Procedure v2 item revision")?,
                    old.value().map(encode_item_value_v2).transpose()?,
                    sqlite_u64(old.created_at().get(), "Procedure v2 item creation timestamp")?,
                    sqlite_u64(old.updated_at().get(), "Procedure v2 item update timestamp")?,
                ],
            ).map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
            if changed != 1 {
                return Err(corrupt(StoreRecordKindV1::Item));
            }
        }
        for (old, new) in old_attempt.blockers().iter().zip(new_attempt.blockers()) {
            if old == new {
                continue;
            }
            let changed = transaction
                .execute(
                    "UPDATE v2_blockers SET state = ?1, resolved_at_ms = ?2 \
                 WHERE blocker_id = ?3 AND attempt_id = ?4 AND reason = ?5 AND state = ?6 \
                 AND created_at_ms = ?7 AND resolved_at_ms IS ?8",
                    params![
                        blocker_state_text(new.state()),
                        new.resolved_at()
                            .map(|value| sqlite_u64(
                                value.get(),
                                "Procedure v2 blocker resolution timestamp"
                            ))
                            .transpose()?,
                        old.blocker_id().as_str(),
                        old.attempt_id().as_str(),
                        old.reason(),
                        blocker_state_text(old.state()),
                        sqlite_u64(
                            old.created_at().get(),
                            "Procedure v2 blocker creation timestamp"
                        )?,
                        old.resolved_at()
                            .map(|value| sqlite_u64(
                                value.get(),
                                "Procedure v2 blocker resolution timestamp"
                            ))
                            .transpose()?,
                    ],
                )
                .map_err(|error| record_error(error, StoreRecordKindV1::Blocker))?;
            if changed != 1 {
                return Err(corrupt(StoreRecordKindV1::Blocker));
            }
        }
        for blocker in new_attempt
            .blockers()
            .iter()
            .skip(old_attempt.blockers().len())
        {
            insert_blocker_v2(transaction, blocker)?;
        }
    }
    for attempt in next.attempts().iter().skip(previous.attempts().len()) {
        insert_attempt_memory_v2(transaction, attempt)?;
    }
    for decision in next.decisions().iter().skip(previous.decisions().len()) {
        insert_decision_v2(transaction, decision)?;
    }
    for rework in next.reworks().iter().skip(previous.reworks().len()) {
        insert_rework_v2(transaction, session_id, rework)?;
    }
    Ok(())
}

fn insert_attempt_memory_v2(
    transaction: &Transaction<'_>,
    attempt: &AttemptWorkflowMemoryV2,
) -> Result<(), StoreErrorV1> {
    for slot in attempt.item_slots() {
        transaction.execute(
            "INSERT INTO v2_item_slots (attempt_id, item_id, item_type, item_revision, value_json, \
             created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                slot.attempt_id().as_str(), slot.item_id().as_str(), item_type_text(slot.item_type()),
                sqlite_u64(slot.revision().get(), "Procedure v2 item revision")?,
                slot.value().map(encode_item_value_v2).transpose()?,
                sqlite_u64(slot.created_at().get(), "Procedure v2 item creation timestamp")?,
                sqlite_u64(slot.updated_at().get(), "Procedure v2 item update timestamp")?,
            ],
        ).map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
    }
    for blocker in attempt.blockers() {
        insert_blocker_v2(transaction, blocker)?;
    }
    for reference in attempt.evidence() {
        let selected = canonicalize_json_v1(
            &reference
                .selected_item_ids()
                .iter()
                .map(ItemId::as_str)
                .collect::<Vec<_>>(),
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let (state, source_attempt_id, source_attempt_number, digest, resolved_at) =
            match reference.resolution() {
                ResolvedEvidenceReferenceV2::Resolved(snapshot) => (
                    "resolved",
                    Some(snapshot.source_attempt_id().as_str()),
                    Some(sqlite_u64(
                        snapshot.source_attempt_number().get(),
                        "Procedure v2 source attempt number",
                    )?),
                    Some(snapshot.items_digest().as_str()),
                    Some(sqlite_u64(
                        snapshot.resolved_at().get(),
                        "Procedure v2 evidence timestamp",
                    )?),
                ),
                ResolvedEvidenceReferenceV2::Skipped(snapshot) => (
                    "skipped",
                    Some(snapshot.source_attempt_id().as_str()),
                    Some(sqlite_u64(
                        snapshot.source_attempt_number().get(),
                        "Procedure v2 source attempt number",
                    )?),
                    Some(snapshot.items_digest().as_str()),
                    Some(sqlite_u64(
                        snapshot.resolved_at().get(),
                        "Procedure v2 evidence timestamp",
                    )?),
                ),
                ResolvedEvidenceReferenceV2::Unresolved { .. } => {
                    ("unresolved", None, None, None, None)
                }
            };
        transaction
            .execute(
                "INSERT INTO v2_resolved_evidence_references (attempt_id, source_graph_node_id, \
             reference_ordinal, required, selected_item_ids_json, state, source_attempt_id, \
             source_attempt_number, items_digest, resolved_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    attempt.attempt_id().as_str(),
                    reference.resolution().source_node().as_str(),
                    i64::from(reference.ordinal()),
                    i64::from(reference.required()),
                    selected,
                    state,
                    source_attempt_id,
                    source_attempt_number,
                    digest,
                    resolved_at
                ],
            )
            .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    }
    Ok(())
}

fn insert_blocker_v2(
    transaction: &Transaction<'_>,
    blocker: &BlockerStateV2,
) -> Result<(), StoreErrorV1> {
    transaction.execute(
        "INSERT INTO v2_blockers (blocker_id, attempt_id, reason, state, created_at_ms, resolved_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![blocker.blocker_id().as_str(), blocker.attempt_id().as_str(), blocker.reason(),
            blocker_state_text(blocker.state()),
            sqlite_u64(blocker.created_at().get(), "Procedure v2 blocker creation timestamp")?,
            blocker.resolved_at().map(|value| sqlite_u64(value.get(), "Procedure v2 blocker resolution timestamp")).transpose()?],
    ).map_err(|error| record_error(error, StoreRecordKindV1::Blocker))?;
    Ok(())
}

fn insert_decision_v2(
    transaction: &Transaction<'_>,
    record: &DecisionRecordV2,
) -> Result<(), StoreErrorV1> {
    transaction.execute(
        "INSERT INTO v2_decision_records (attempt_id, session_id, trace_sequence, session_revision, \
         procedure_snapshot_id, procedure_digest, graph_node_id, node_definition_id, attempt_number, \
         goal_revision, selected_option_id, route_effect, route_target_graph_node_id, reason, actor, recorded_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![record.attempt_id().as_str(), record.session_id().as_str(),
            sqlite_u64(record.trace().get(), "Procedure v2 decision trace")?,
            sqlite_u64(record.session_revision().get(), "Procedure v2 decision session revision")?,
            record.procedure_snapshot_id().as_str(), record.procedure_digest().as_str(),
            record.graph_node_id().as_str(), record.node_definition_id().as_str(),
            sqlite_u64(record.attempt_number().get(), "Procedure v2 decision attempt number")?,
            record.goal_revision().map(|value| sqlite_u64(value.get(), "Procedure v2 decision goal revision")).transpose()?,
            record.selected_option().as_str(), record.route_effect().as_str(), record.route_target().as_str(),
            record.reason().as_str(), record.actor().map(ActorAttributionV2::as_str),
            sqlite_u64(record.recorded_at().get(), "Procedure v2 decision timestamp")?],
    ).map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    Ok(())
}

fn insert_rework_v2(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    record: &ReworkRecordV2,
) -> Result<(), StoreErrorV1> {
    transaction
        .execute(
            "INSERT INTO v2_rework_records (session_id, trace_sequence, kind, from_graph_node_id, \
         to_graph_node_id, target_attempt_id, reason, reactivated, actor, recorded_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                session_id.as_str(),
                sqlite_u64(record.trace().get(), "Procedure v2 rework trace")?,
                record.kind().as_str(),
                record.from_node().as_str(),
                record.to_node().as_str(),
                record.target_attempt_id().as_str(),
                record.reason().as_str(),
                i64::from(record.reactivated()),
                record.actor().map(ActorAttributionV2::as_str),
                sqlite_u64(record.recorded_at().get(), "Procedure v2 rework timestamp")?
            ],
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    Ok(())
}

pub(crate) fn load_workflow_memory_v2(
    connection: &Connection,
    snapshot: &ProcedureSnapshotV2,
    trace: &SessionTraceV2,
    metadata: &[AttemptMetadataV2],
) -> Result<WorkflowMemoryStateV2, StoreErrorV1> {
    let model = SnapshotMemoryModelV2::from_snapshot(snapshot)
        .map_err(|_| corrupt(StoreRecordKindV1::Snapshot))?;
    let mut attempts = Vec::with_capacity(trace.attempts().len());
    for (attempt, attempt_metadata) in trace.attempts().iter().zip(metadata) {
        attempts.push(load_attempt_memory_v2(
            connection,
            attempt,
            attempt_metadata,
            model
                .node(attempt.graph_node_id())
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
        )?);
    }
    let decisions = load_decisions_v2(connection, snapshot, trace, &attempts)?;
    let reworks = load_reworks_v2(connection, trace.session_id())?;
    WorkflowMemoryStateV2::new(attempts, decisions, reworks)
        .map_err(|_| corrupt(StoreRecordKindV1::Session))
}

fn load_attempt_memory_v2(
    connection: &Connection,
    attempt: &SessionAttemptV2,
    metadata: &AttemptMetadataV2,
    specification: &NodeMemorySpecV2,
) -> Result<AttemptWorkflowMemoryV2, StoreErrorV1> {
    type SlotRow = (String, String, i64, Option<String>, i64, i64);
    let mut statement = connection
        .prepare(
            "SELECT item_id, item_type, item_revision, value_json, created_at_ms, updated_at_ms \
         FROM v2_item_slots WHERE attempt_id = ?1",
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
    let rows = statement
        .query_map([attempt.attempt_id().as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
    let mut loaded = BTreeMap::new();
    for row in rows {
        let row: SlotRow = row.map_err(|error| record_error(error, StoreRecordKindV1::Item))?;
        if loaded.insert(row.0.clone(), row).is_some() {
            return Err(corrupt(StoreRecordKindV1::Item));
        }
    }
    let mut slots = Vec::with_capacity(specification.items.len());
    for item in &specification.items {
        let (_, item_type, revision, value, created_at, updated_at) = loaded
            .remove(item.id().as_str())
            .ok_or_else(|| corrupt(StoreRecordKindV1::Item))?;
        if parse_item_type(&item_type).map_err(|_| corrupt(StoreRecordKindV1::Item))?
            != item.item_type()
        {
            return Err(corrupt(StoreRecordKindV1::Item));
        }
        slots.push(
            ItemSlotStateV2::new(
                attempt.attempt_id().clone(),
                item.id().clone(),
                item.item_type(),
                Revision::new(persisted_u64(revision, StoreRecordKindV1::Item)?),
                value
                    .map(|value| decode_item_value_v2(&value, item.item_type()))
                    .transpose()?,
                UnixMillis::new(persisted_u64(created_at, StoreRecordKindV1::Item)?),
                UnixMillis::new(persisted_u64(updated_at, StoreRecordKindV1::Item)?),
            )
            .map_err(|_| corrupt(StoreRecordKindV1::Item))?,
        );
    }
    if !loaded.is_empty() {
        return Err(corrupt(StoreRecordKindV1::Item));
    }
    let blockers = load_blockers_v2(connection, attempt, metadata)?;
    let evidence = load_evidence_v2(connection, attempt)?;
    AttemptWorkflowMemoryV2::new(attempt.attempt_id().clone(), slots, blockers, evidence)
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))
}

fn load_blockers_v2(
    connection: &Connection,
    attempt: &SessionAttemptV2,
    _metadata: &AttemptMetadataV2,
) -> Result<Vec<BlockerStateV2>, StoreErrorV1> {
    let mut statement = connection
        .prepare(
            "SELECT blocker_id, reason, state, created_at_ms, resolved_at_ms FROM v2_blockers \
         WHERE attempt_id = ?1 ORDER BY created_at_ms, blocker_id",
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Blocker))?;
    let rows = statement
        .query_map([attempt.attempt_id().as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Blocker))?;
    rows.map(|row| {
        let (id, reason, state, created, resolved) =
            row.map_err(|error| record_error(error, StoreRecordKindV1::Blocker))?;
        BlockerStateV2::new(
            BlockerId::new(id).map_err(|_| corrupt(StoreRecordKindV1::Blocker))?,
            attempt.attempt_id().clone(),
            reason,
            match state.as_str() {
                "open" => BlockerState::Open,
                "resolved" => BlockerState::Resolved,
                _ => return Err(corrupt(StoreRecordKindV1::Blocker)),
            },
            UnixMillis::new(persisted_u64(created, StoreRecordKindV1::Blocker)?),
            resolved
                .map(|value| persisted_u64(value, StoreRecordKindV1::Blocker).map(UnixMillis::new))
                .transpose()?,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Blocker))
    })
    .collect()
}

fn load_evidence_v2(
    connection: &Connection,
    attempt: &SessionAttemptV2,
) -> Result<Vec<EvidenceResolutionStateV2>, StoreErrorV1> {
    type EvidenceRow = (
        String,
        i64,
        i64,
        String,
        String,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<i64>,
    );
    let mut statement = connection.prepare(
        "SELECT source_graph_node_id, reference_ordinal, required, selected_item_ids_json, state, \
         source_attempt_id, source_attempt_number, items_digest, resolved_at_ms \
         FROM v2_resolved_evidence_references WHERE attempt_id = ?1 ORDER BY reference_ordinal",
    ).map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows = statement
        .query_map([attempt.attempt_id().as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    rows.map(|row| {
        let row: EvidenceRow =
            row.map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
        verify_canonical_json_v1(row.3.as_bytes())
            .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let selected: Vec<String> =
            serde_json::from_str(&row.3).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let selected = selected
            .into_iter()
            .map(|id| ItemId::new(id).map_err(|_| corrupt(StoreRecordKindV1::Attempt)))
            .collect::<Result<Vec<_>, _>>()?;
        let source_node =
            GraphNodeId::new(row.0).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let resolution = match row.4.as_str() {
            "unresolved"
                if row.5.is_none() && row.6.is_none() && row.7.is_none() && row.8.is_none() =>
            {
                ResolvedEvidenceReferenceV2::unresolved(source_node)
            }
            "resolved" | "skipped" => {
                let snapshot = EvidenceReferenceSnapshotV2::new(
                    source_node,
                    AttemptId::new(row.5.ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?)
                        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
                    AttemptNumberV2::new(persisted_u64(
                        row.6.ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?,
                        StoreRecordKindV1::Attempt,
                    )?),
                    Sha256Digest::new(row.7.ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?)
                        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
                    UnixMillis::new(persisted_u64(
                        row.8.ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?,
                        StoreRecordKindV1::Attempt,
                    )?),
                )
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
                if row.4 == "resolved" {
                    ResolvedEvidenceReferenceV2::resolved(snapshot)
                } else {
                    ResolvedEvidenceReferenceV2::skipped(snapshot)
                }
            }
            _ => return Err(corrupt(StoreRecordKindV1::Attempt)),
        };
        EvidenceResolutionStateV2::new(
            u32::try_from(row.1).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            match row.2 {
                0 => false,
                1 => true,
                _ => return Err(corrupt(StoreRecordKindV1::Attempt)),
            },
            selected,
            resolution,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))
    })
    .collect()
}

fn load_decisions_v2(
    connection: &Connection,
    _snapshot: &ProcedureSnapshotV2,
    trace: &SessionTraceV2,
    attempts: &[AttemptWorkflowMemoryV2],
) -> Result<Vec<DecisionRecordV2>, StoreErrorV1> {
    type DecisionRow = (
        String,
        String,
        i64,
        i64,
        String,
        String,
        String,
        String,
        i64,
        Option<i64>,
        String,
        String,
        String,
        String,
        Option<String>,
        i64,
    );
    let memory_by_id: BTreeMap<_, _> = attempts
        .iter()
        .map(|memory| (memory.attempt_id(), memory))
        .collect();
    let mut statement = connection.prepare(
        "SELECT attempt_id, session_id, trace_sequence, session_revision, procedure_snapshot_id, \
         procedure_digest, graph_node_id, node_definition_id, attempt_number, goal_revision, \
         selected_option_id, route_effect, route_target_graph_node_id, reason, actor, recorded_at_ms \
         FROM v2_decision_records WHERE session_id = ?1 ORDER BY trace_sequence",
    ).map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows = statement
        .query_map([trace.session_id().as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
                row.get(12)?,
                row.get(13)?,
                row.get(14)?,
                row.get(15)?,
            ))
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    rows.map(|row| {
        let row: DecisionRow =
            row.map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
        let attempt_id = AttemptId::new(row.0).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        let evidence = ResolvedEvidenceSetV2::new(
            memory_by_id
                .get(&attempt_id)
                .ok_or_else(|| corrupt(StoreRecordKindV1::Attempt))?
                .evidence()
                .iter()
                .map(|reference| reference.resolution().clone())
                .collect(),
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?;
        DecisionRecordV2::new(DecisionRecordInputV2 {
            attempt_id,
            session_id: SessionId::new(row.1).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            trace: TraceSequenceV2::new(persisted_u64(row.2, StoreRecordKindV1::Attempt)?),
            session_revision: Revision::new(persisted_u64(row.3, StoreRecordKindV1::Attempt)?),
            procedure_snapshot_id: ProcedureSnapshotId::new(row.4)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            procedure_digest: Sha256Digest::new(row.5)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            graph_node_id: GraphNodeId::new(row.6)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            node_definition_id: NodeDefinitionId::new(row.7)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            attempt_number: AttemptNumberV2::new(persisted_u64(row.8, StoreRecordKindV1::Attempt)?),
            goal_revision: row
                .9
                .map(|value| {
                    persisted_u64(value, StoreRecordKindV1::Attempt).map(GoalRevisionNumberV2::new)
                })
                .transpose()?,
            selected_option: OptionId::new(row.10)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            route_effect: TransitionEffectV2::from_str(&row.11)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            route_target: GraphNodeId::new(row.12)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            reason: ReasonV2::new(row.13).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            evidence,
            actor: row
                .14
                .map(ActorAttributionV2::new)
                .transpose()
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            recorded_at: UnixMillis::new(persisted_u64(row.15, StoreRecordKindV1::Attempt)?),
        })
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))
    })
    .collect()
}

fn load_reworks_v2(
    connection: &Connection,
    session_id: &SessionId,
) -> Result<Vec<ReworkRecordV2>, StoreErrorV1> {
    type ReworkRow = (
        i64,
        String,
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
        i64,
    );
    let mut statement = connection
        .prepare(
            "SELECT trace_sequence, kind, from_graph_node_id, to_graph_node_id, target_attempt_id, \
         reason, reactivated, actor, recorded_at_ms FROM v2_rework_records \
         WHERE session_id = ?1 ORDER BY trace_sequence",
        )
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    let rows = statement
        .query_map([session_id.as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })
        .map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
    rows.map(|row| {
        let row: ReworkRow =
            row.map_err(|error| record_error(error, StoreRecordKindV1::Attempt))?;
        ReworkRecordV2::new(ReworkRecordInputV2 {
            trace: TraceSequenceV2::new(persisted_u64(row.0, StoreRecordKindV1::Attempt)?),
            kind: ReworkKindV2::from_str(&row.1)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            from_node: GraphNodeId::new(row.2).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            to_node: GraphNodeId::new(row.3).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            target_attempt_id: AttemptId::new(row.4)
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            reason: ReasonV2::new(row.5).map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            reactivated: match row.6 {
                0 => false,
                1 => true,
                _ => return Err(corrupt(StoreRecordKindV1::Attempt)),
            },
            actor: row
                .7
                .map(ActorAttributionV2::new)
                .transpose()
                .map_err(|_| corrupt(StoreRecordKindV1::Attempt))?,
            recorded_at: UnixMillis::new(persisted_u64(row.8, StoreRecordKindV1::Attempt)?),
        })
        .map_err(|_| corrupt(StoreRecordKindV1::Attempt))
    })
    .collect()
}
