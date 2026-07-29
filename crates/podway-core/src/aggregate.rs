use std::collections::{BTreeMap, BTreeSet};

use crate::procedure::{
    ArtifactItemSpecV1, ItemSpecV1, ItemTypeV1, ListItemSpecV1, ProcedureSnapshotV1, StageSpecV1,
    TextItemSpecV1, validate_media_type,
};
use crate::{
    AttemptId, AttemptLifecycle, BlockerId, BlockerState, DomainError, ItemId, Revision, SessionId,
    SessionLifecycle, Sha256Digest, StageId, StageProgressState, UnixMillis,
};

/// Maximum blocker records retained by one attempt, bounding compact status output.
pub const MAX_BLOCKERS_PER_ATTEMPT_V1: usize = 1_024;

/// The location mode of stored artifact metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactLocationKindV1 {
    LocalPath,
    ExternalReference,
}

/// Complete metadata for either a worktree-local artifact or an opaque external reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactValueV1 {
    location_kind: ArtifactLocationKindV1,
    location: String,
    digest: Sha256Digest,
    size_bytes: u64,
    media_type: String,
}

impl ArtifactValueV1 {
    pub fn local_path(
        path: impl Into<String>,
        digest: Sha256Digest,
        size_bytes: u64,
        media_type: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let path = path.into();
        validate_safe_local_path(&path)?;
        Self::new(
            ArtifactLocationKindV1::LocalPath,
            path,
            digest,
            size_bytes,
            media_type.into(),
        )
    }

    pub fn external_reference(
        reference: impl Into<String>,
        digest: Sha256Digest,
        size_bytes: u64,
        media_type: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let reference = reference.into();
        validate_external_reference(&reference)?;
        Self::new(
            ArtifactLocationKindV1::ExternalReference,
            reference,
            digest,
            size_bytes,
            media_type.into(),
        )
    }

    fn new(
        location_kind: ArtifactLocationKindV1,
        location: String,
        digest: Sha256Digest,
        size_bytes: u64,
        media_type: String,
    ) -> Result<Self, DomainError> {
        validate_media_type(&media_type)?;
        Ok(Self {
            location_kind,
            location,
            digest,
            size_bytes,
            media_type,
        })
    }

    pub const fn location_kind(&self) -> ArtifactLocationKindV1 {
        self.location_kind
    }

    pub fn location(&self) -> &str {
        &self.location
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }
}

/// The kind of data contained in an item value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemValueTypeV1 {
    Confirm,
    Text,
    Choice,
    Integer,
    List,
    Artifact,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ItemValueKindV1 {
    Confirm,
    Text(String),
    Choice(String),
    Integer(i64),
    List(Vec<String>),
    Artifact(ArtifactValueV1),
}

/// A typed candidate value admitted into a slot only through an item specification's storage rules.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemValueV1 {
    kind: ItemValueKindV1,
}

impl ItemValueV1 {
    pub const fn confirm() -> Self {
        Self {
            kind: ItemValueKindV1::Confirm,
        }
    }

    pub fn text(value: impl Into<String>) -> Self {
        Self {
            kind: ItemValueKindV1::Text(value.into()),
        }
    }

    pub fn choice(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty() || value.chars().count() > 120 {
            return Err(invalid("choice value must be between one and 120 scalars"));
        }
        Ok(Self {
            kind: ItemValueKindV1::Choice(value),
        })
    }

    pub const fn integer(value: i64) -> Self {
        Self {
            kind: ItemValueKindV1::Integer(value),
        }
    }

    pub fn list(values: Vec<String>) -> Result<Self, DomainError> {
        if values.len() > crate::procedure::MAX_LIST_ITEMS as usize {
            return Err(invalid("list value exceeds the procedure hard maximum"));
        }
        for value in &values {
            if value.trim().is_empty()
                || value.chars().count() > crate::procedure::MAX_LIST_ITEM_LENGTH as usize
            {
                return Err(invalid("list value contains an invalid entry"));
            }
        }
        Ok(Self {
            kind: ItemValueKindV1::List(values),
        })
    }

    pub fn artifact(value: ArtifactValueV1) -> Self {
        Self {
            kind: ItemValueKindV1::Artifact(value),
        }
    }

    pub fn value_type(&self) -> ItemValueTypeV1 {
        match &self.kind {
            ItemValueKindV1::Confirm => ItemValueTypeV1::Confirm,
            ItemValueKindV1::Text(_) => ItemValueTypeV1::Text,
            ItemValueKindV1::Choice(_) => ItemValueTypeV1::Choice,
            ItemValueKindV1::Integer(_) => ItemValueTypeV1::Integer,
            ItemValueKindV1::List(_) => ItemValueTypeV1::List,
            ItemValueKindV1::Artifact(_) => ItemValueTypeV1::Artifact,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match &self.kind {
            ItemValueKindV1::Text(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_choice(&self) -> Option<&str> {
        match &self.kind {
            ItemValueKindV1::Choice(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match &self.kind {
            ItemValueKindV1::Integer(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[String]> {
        match &self.kind {
            ItemValueKindV1::List(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_artifact(&self) -> Option<&ArtifactValueV1> {
        match &self.kind {
            ItemValueKindV1::Artifact(value) => Some(value),
            _ => None,
        }
    }
}

/// Returns whether an optional current value satisfies every item rule for completion.
pub fn item_satisfied(specification: &ItemSpecV1, value: Option<&ItemValueV1>) -> bool {
    let Some(value) = value else {
        return false;
    };
    if !item_value_admissible(specification, value) {
        return false;
    }
    match specification {
        ItemSpecV1::Text(specification) => value
            .as_text()
            .is_some_and(|value| text_satisfies(specification, value)),
        ItemSpecV1::List(specification) => value
            .as_list()
            .is_some_and(|values| values.len() >= specification.min_items() as usize),
        _ => true,
    }
}

/// A per-attempt item row. Clearing advances the revision and retains timestamped tombstone metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemSlotV1 {
    attempt_id: AttemptId,
    item_id: ItemId,
    item_type: ItemTypeV1,
    revision: Revision,
    value: Option<ItemValueV1>,
    created_at: Option<UnixMillis>,
    updated_at: Option<UnixMillis>,
}
/// Complete persisted input for reconstructing one item slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemSlotInputV1<'a> {
    pub attempt_id: AttemptId,
    pub specification: &'a ItemSpecV1,
    pub revision: Revision,
    pub value: Option<ItemValueV1>,
    pub created_at: Option<UnixMillis>,
    pub updated_at: Option<UnixMillis>,
}

impl ItemSlotV1 {
    pub fn new_empty(attempt_id: AttemptId, specification: &ItemSpecV1) -> Self {
        Self {
            attempt_id,
            item_id: specification.id().clone(),
            item_type: specification.item_type(),
            revision: Revision::ZERO,
            value: None,
            created_at: None,
            updated_at: None,
        }
    }

    pub fn new(input: ItemSlotInputV1<'_>) -> Result<Self, DomainError> {
        let ItemSlotInputV1 {
            attempt_id,
            specification,
            revision,
            value,
            created_at,
            updated_at,
        } = input;
        let slot = Self {
            attempt_id,
            item_id: specification.id().clone(),
            item_type: specification.item_type(),
            revision,
            value,
            created_at,
            updated_at,
        };
        slot.validate_for(specification)?;
        Ok(slot)
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

    pub fn value(&self) -> Option<&ItemValueV1> {
        self.value.as_ref()
    }

    pub const fn created_at(&self) -> Option<UnixMillis> {
        self.created_at
    }

    pub const fn updated_at(&self) -> Option<UnixMillis> {
        self.updated_at
    }

    pub fn with_value(
        &self,
        specification: &ItemSpecV1,
        value: ItemValueV1,
        updated_at: UnixMillis,
    ) -> Result<Self, DomainError> {
        self.validate_identity(specification)?;
        if !item_value_admissible(specification, &value) {
            return Err(invalid("item value does not meet its storage rules"));
        }
        if self
            .updated_at
            .is_some_and(|current_updated_at| updated_at < current_updated_at)
        {
            return Err(invalid("item value timestamp precedes its current update"));
        }
        if self.value.as_ref() == Some(&value) {
            return Ok(self.clone());
        }
        let created_at = self.created_at.unwrap_or(updated_at);
        if updated_at < created_at {
            return Err(invalid("item value timestamp precedes creation"));
        }
        Self::new(ItemSlotInputV1 {
            attempt_id: self.attempt_id.clone(),
            specification,
            revision: self.revision.checked_next()?,
            value: Some(value),
            created_at: Some(created_at),
            updated_at: Some(updated_at),
        })
    }

    pub fn with_cleared(
        &self,
        specification: &ItemSpecV1,
        updated_at: UnixMillis,
    ) -> Result<Self, DomainError> {
        self.validate_identity(specification)?;
        if self.value.is_none() {
            return Ok(self.clone());
        }
        let created_at = self
            .created_at
            .ok_or(invalid("populated item slot is missing its creation time"))?;
        let current_updated_at = self
            .updated_at
            .ok_or(invalid("populated item slot is missing its update time"))?;
        if updated_at < created_at || updated_at < current_updated_at {
            return Err(invalid("item clear timestamp precedes its current value"));
        }
        Self::new(ItemSlotInputV1 {
            attempt_id: self.attempt_id.clone(),
            specification,
            revision: self.revision.checked_next()?,
            value: None,
            created_at: Some(created_at),
            updated_at: Some(updated_at),
        })
    }

    fn validate_for(&self, specification: &ItemSpecV1) -> Result<(), DomainError> {
        self.validate_identity(specification)?;
        match (&self.value, self.created_at, self.updated_at) {
            (Some(value), Some(created_at), Some(updated_at))
                if (self.revision == Revision::new(1) && updated_at == created_at)
                    || (self.revision > Revision::new(1) && updated_at >= created_at) =>
            {
                if !item_value_admissible(specification, value) {
                    return Err(invalid("item slot value does not meet its storage rules"));
                }
            }
            (None, None, None) if self.revision == Revision::ZERO => {}
            (None, Some(created_at), Some(updated_at))
                if self.revision > Revision::new(1) && updated_at >= created_at => {}
            (Some(_), _, _) => return Err(invalid("invalid populated item slot metadata")),
            (None, _, _) => return Err(invalid("invalid empty item slot metadata")),
        }
        validate_slot_reachability(specification, self.revision, self.value.as_ref())
    }

    fn validate_identity(&self, specification: &ItemSpecV1) -> Result<(), DomainError> {
        if &self.item_id != specification.id() || self.item_type != specification.item_type() {
            return Err(invalid("item slot does not match its specification"));
        }
        Ok(())
    }
}

fn validate_slot_reachability(
    specification: &ItemSpecV1,
    revision: Revision,
    value: Option<&ItemValueV1>,
) -> Result<(), DomainError> {
    if revision == Revision::ZERO {
        return if value.is_none() {
            Ok(())
        } else {
            Err(invalid("revision-zero item slots must be pristine"))
        };
    }
    if value.is_none() && revision.get() < 2 {
        return Err(invalid(
            "an empty mutated item slot requires at least set-and-clear revisions",
        ));
    }

    let singleton = match specification {
        ItemSpecV1::Confirm(_) => true,
        ItemSpecV1::Text(_) => false,
        ItemSpecV1::Choice(choice) => choice.choices().len() == 1,
        ItemSpecV1::Integer(integer) => {
            matches!(
                (integer.minimum(), integer.maximum()),
                (Some(minimum), Some(maximum)) if minimum == maximum
            ) || matches!(
                (integer.minimum(), integer.maximum()),
                (Some(i64::MAX), None) | (None, Some(i64::MIN))
            )
        }
        ItemSpecV1::List(_) => false,
        ItemSpecV1::Artifact(_) => false,
    };
    if singleton && value.is_some() != (revision.get() % 2 == 1) {
        return Err(invalid(
            "singleton item value presence does not match revision parity",
        ));
    }
    Ok(())
}

/// A blocker is immutable apart from clone-based resolution to a new value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockerV1 {
    blocker_id: BlockerId,
    attempt_id: AttemptId,
    reason: String,
    state: BlockerState,
    created_at: UnixMillis,
    resolved_at: Option<UnixMillis>,
}
/// Complete persisted input for reconstructing one blocker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockerInputV1 {
    pub blocker_id: BlockerId,
    pub attempt_id: AttemptId,
    pub reason: String,
    pub state: BlockerState,
    pub created_at: UnixMillis,
    pub resolved_at: Option<UnixMillis>,
}

impl BlockerV1 {
    pub fn open(
        blocker_id: BlockerId,
        attempt_id: AttemptId,
        reason: impl Into<String>,
        created_at: UnixMillis,
    ) -> Result<Self, DomainError> {
        Self::new(BlockerInputV1 {
            blocker_id,
            attempt_id,
            reason: reason.into(),
            state: BlockerState::Open,
            created_at,
            resolved_at: None,
        })
    }

    pub fn new(input: BlockerInputV1) -> Result<Self, DomainError> {
        let BlockerInputV1 {
            blocker_id,
            attempt_id,
            reason,
            state,
            created_at,
            resolved_at,
        } = input;
        validate_reason(&reason)?;
        match (state, resolved_at) {
            (BlockerState::Open, None) => {}
            (BlockerState::Resolved, Some(resolved_at)) if resolved_at >= created_at => {}
            _ => return Err(invalid("invalid blocker resolution metadata")),
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

    pub fn resolve(&self, resolved_at: UnixMillis) -> Result<Self, DomainError> {
        if self.state != BlockerState::Open || resolved_at < self.created_at {
            return Err(invalid("blocker cannot be resolved at the supplied time"));
        }
        Self::new(BlockerInputV1 {
            blocker_id: self.blocker_id.clone(),
            attempt_id: self.attempt_id.clone(),
            reason: self.reason.clone(),
            state: BlockerState::Resolved,
            created_at: self.created_at,
            resolved_at: Some(resolved_at),
        })
    }
}

/// Input admitted only after `AttemptV1::new` validates attempt ownership and lifecycle.
#[derive(Clone, Debug)]
pub struct AttemptInputV1<'a> {
    pub attempt_id: AttemptId,
    pub session_id: SessionId,
    pub stage: &'a StageSpecV1,
    pub number: u32,
    pub lifecycle: AttemptLifecycle,
    pub started_at: UnixMillis,
    pub ended_at: Option<UnixMillis>,
    pub reason: Option<String>,
    pub item_slots: Vec<ItemSlotV1>,
    pub blockers: Vec<BlockerV1>,
}

/// Immutable attempt history for exactly one stage and session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptV1 {
    attempt_id: AttemptId,
    session_id: SessionId,
    stage_id: StageId,
    number: u32,
    lifecycle: AttemptLifecycle,
    started_at: UnixMillis,
    ended_at: Option<UnixMillis>,
    reason: Option<String>,
    item_slots: Vec<ItemSlotV1>,
    blockers: Vec<BlockerV1>,
}

impl AttemptV1 {
    pub fn fresh(
        attempt_id: AttemptId,
        session_id: SessionId,
        stage: &StageSpecV1,
        number: u32,
        started_at: UnixMillis,
    ) -> Result<Self, DomainError> {
        Self::fresh_with_reason(attempt_id, session_id, stage, number, started_at, None)
    }

    pub(crate) fn fresh_with_reason(
        attempt_id: AttemptId,
        session_id: SessionId,
        stage: &StageSpecV1,
        number: u32,
        started_at: UnixMillis,
        reason: Option<String>,
    ) -> Result<Self, DomainError> {
        let item_slots = stage
            .items()
            .iter()
            .map(|item| ItemSlotV1::new_empty(attempt_id.clone(), item))
            .collect();
        Self::new(AttemptInputV1 {
            attempt_id,
            session_id,
            stage,
            number,
            lifecycle: AttemptLifecycle::Active,
            started_at,
            ended_at: None,
            reason,
            item_slots,
            blockers: Vec::new(),
        })
    }

    pub fn new(input: AttemptInputV1<'_>) -> Result<Self, DomainError> {
        let AttemptInputV1 {
            attempt_id,
            session_id,
            stage,
            number,
            lifecycle,
            started_at,
            ended_at,
            reason,
            item_slots,
            blockers,
        } = input;
        if number == 0 {
            return Err(invalid("attempt number must start at one"));
        }
        if let Some(reason) = &reason {
            validate_reason(reason)?;
        }
        validate_attempt_lifecycle(lifecycle, started_at, ended_at, reason.as_deref())?;
        if number == 1
            && reason.is_some()
            && matches!(
                lifecycle,
                AttemptLifecycle::Active | AttemptLifecycle::Completed
            )
        {
            return Err(invalid(
                "first active or completed attempts cannot retain a reason",
            ));
        }
        validate_slots(&attempt_id, stage, &item_slots)?;
        validate_blockers(&attempt_id, lifecycle, &blockers)?;
        validate_terminal_attempt(stage, lifecycle, reason.as_deref(), &item_slots, &blockers)?;
        validate_attempt_record_times(started_at, ended_at, &item_slots, &blockers)?;
        Ok(Self {
            attempt_id,
            session_id,
            stage_id: stage.id().clone(),
            number,
            lifecycle,
            started_at,
            ended_at,
            reason,
            item_slots,
            blockers,
        })
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn stage_id(&self) -> &StageId {
        &self.stage_id
    }

    pub const fn number(&self) -> u32 {
        self.number
    }

    pub const fn lifecycle(&self) -> AttemptLifecycle {
        self.lifecycle
    }

    pub const fn started_at(&self) -> UnixMillis {
        self.started_at
    }

    pub const fn ended_at(&self) -> Option<UnixMillis> {
        self.ended_at
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    pub fn item_slots(&self) -> &[ItemSlotV1] {
        &self.item_slots
    }

    pub fn blockers(&self) -> &[BlockerV1] {
        &self.blockers
    }

    pub fn is_ready_to_complete(&self, stage: &StageSpecV1) -> bool {
        self.lifecycle == AttemptLifecycle::Active
            && self.stage_id == *stage.id()
            && self
                .blockers
                .iter()
                .all(|blocker| blocker.state() != BlockerState::Open)
            && required_items_satisfied(stage, &self.item_slots)
    }

    pub fn with_replaced_slot(
        &self,
        stage: &StageSpecV1,
        slot: ItemSlotV1,
    ) -> Result<Self, DomainError> {
        if self.lifecycle != AttemptLifecycle::Active {
            return Err(invalid("only an active attempt may update an item slot"));
        }
        if stage.id() != &self.stage_id {
            return Err(invalid(
                "replacement stage does not match the attempt stage",
            ));
        }
        if slot.attempt_id() != &self.attempt_id {
            return Err(invalid("replacement slot belongs to another attempt"));
        }
        let Some(specification) = stage.item(slot.item_id()) else {
            return Err(invalid("replacement slot is not in the attempt stage"));
        };
        slot.validate_for(specification)?;
        let Some(index) = self
            .item_slots
            .iter()
            .position(|existing| existing.item_id() == slot.item_id())
        else {
            return Err(invalid("replacement slot is not in the attempt"));
        };
        let current = &self.item_slots[index];
        if slot != *current {
            let updated_at = slot.updated_at().ok_or(invalid(
                "replacement item slot is missing an update timestamp",
            ))?;
            let expected = match slot.value() {
                Some(value) => current.with_value(specification, value.clone(), updated_at)?,
                None => current.with_cleared(specification, updated_at)?,
            };
            if slot != expected {
                return Err(invalid(
                    "replacement item slot is not reachable in one transition",
                ));
            }
        }
        let mut next = self.clone();
        next.item_slots[index] = slot;
        validate_slots(&next.attempt_id, stage, &next.item_slots)?;
        validate_attempt_record_times(
            next.started_at,
            next.ended_at,
            &next.item_slots,
            &next.blockers,
        )?;
        Ok(next)
    }

    pub fn with_added_blocker(&self, blocker: BlockerV1) -> Result<Self, DomainError> {
        if self.lifecycle != AttemptLifecycle::Active || blocker.attempt_id() != &self.attempt_id {
            return Err(invalid("blocker does not belong to the active attempt"));
        }
        if self
            .blockers
            .iter()
            .any(|existing| existing.blocker_id() == blocker.blocker_id())
        {
            return Err(invalid("attempt blocker identifiers must be unique"));
        }
        let mut next = self.clone();
        next.blockers.push(blocker);
        validate_blockers(&next.attempt_id, next.lifecycle, &next.blockers)?;
        validate_attempt_record_times(
            next.started_at,
            next.ended_at,
            &next.item_slots,
            &next.blockers,
        )?;
        Ok(next)
    }

    pub fn with_terminal(
        &self,
        stage: &StageSpecV1,
        lifecycle: AttemptLifecycle,
        ended_at: UnixMillis,
        terminal_reason: Option<String>,
    ) -> Result<Self, DomainError> {
        if self.lifecycle != AttemptLifecycle::Active || lifecycle == AttemptLifecycle::Active {
            return Err(invalid("only an active attempt can become terminal"));
        }
        if stage.id() != &self.stage_id {
            return Err(invalid("terminal stage does not match the attempt stage"));
        }
        if lifecycle == AttemptLifecycle::Completed && terminal_reason.is_some() {
            return Err(invalid("completion does not accept a new terminal reason"));
        }
        if let Some(reason) = &terminal_reason {
            validate_reason(reason)?;
        }
        match lifecycle {
            AttemptLifecycle::Completed if !self.is_ready_to_complete(stage) => {
                return Err(invalid("the active attempt is not ready to complete"));
            }
            AttemptLifecycle::Skipped if !stage.skip_policy().is_allowed() => {
                return Err(invalid("the active stage may not be skipped"));
            }
            AttemptLifecycle::Skipped
                if stage.skip_policy().reason_required() && terminal_reason.is_none() =>
            {
                return Err(invalid("a non-empty reason is required"));
            }
            AttemptLifecycle::Abandoned if terminal_reason.is_none() => {
                return Err(invalid("a non-empty reason is required"));
            }
            AttemptLifecycle::Active
            | AttemptLifecycle::Completed
            | AttemptLifecycle::Skipped
            | AttemptLifecycle::Abandoned => {}
        }
        let reason = match lifecycle {
            AttemptLifecycle::Completed => self.reason.clone(),
            AttemptLifecycle::Skipped | AttemptLifecycle::Abandoned => {
                terminal_reason.or_else(|| self.reason.clone())
            }
            AttemptLifecycle::Active => return Err(invalid("an attempt must become terminal")),
        };
        let blockers = match lifecycle {
            AttemptLifecycle::Completed => self.blockers.clone(),
            AttemptLifecycle::Skipped | AttemptLifecycle::Abandoned => self
                .blockers
                .iter()
                .map(|blocker| {
                    if blocker.state() == BlockerState::Open {
                        blocker.resolve(ended_at)
                    } else {
                        Ok(blocker.clone())
                    }
                })
                .collect::<Result<Vec<_>, _>>()?,
            AttemptLifecycle::Active => return Err(invalid("an attempt must become terminal")),
        };
        Self::new(AttemptInputV1 {
            attempt_id: self.attempt_id.clone(),
            session_id: self.session_id.clone(),
            stage,
            number: self.number,
            lifecycle,
            started_at: self.started_at,
            ended_at: Some(ended_at),
            reason,
            item_slots: self.item_slots.clone(),
            blockers,
        })
    }
}

/// One progress row for each snapshot stage, preserving the snapshot's ordered index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageProgressV1 {
    stage_id: StageId,
    stage_index: usize,
    state: StageProgressState,
    latest_attempt_number: u32,
    latest_attempt_id: Option<AttemptId>,
}

impl StageProgressV1 {
    pub fn new(
        stage_id: StageId,
        stage_index: usize,
        state: StageProgressState,
        latest_attempt_number: u32,
        latest_attempt_id: Option<AttemptId>,
    ) -> Result<Self, DomainError> {
        if (latest_attempt_number == 0) == latest_attempt_id.is_some() {
            return Err(invalid(
                "stage latest attempt number and identifier must agree",
            ));
        }
        match state {
            StageProgressState::Pending if latest_attempt_number == 0 => {}
            StageProgressState::Current
            | StageProgressState::Done
            | StageProgressState::Skipped
            | StageProgressState::Redo
            | StageProgressState::Abandoned
                if latest_attempt_number > 0 => {}
            _ => {
                return Err(invalid(
                    "stage progress state and attempt history must agree",
                ));
            }
        }
        Ok(Self {
            stage_id,
            stage_index,
            state,
            latest_attempt_number,
            latest_attempt_id,
        })
    }

    pub fn pending(stage_id: StageId, stage_index: usize) -> Self {
        Self {
            stage_id,
            stage_index,
            state: StageProgressState::Pending,
            latest_attempt_number: 0,
            latest_attempt_id: None,
        }
    }

    pub fn current(
        stage_id: StageId,
        stage_index: usize,
        attempt_id: AttemptId,
        attempt_number: u32,
    ) -> Result<Self, DomainError> {
        Self::new(
            stage_id,
            stage_index,
            StageProgressState::Current,
            attempt_number,
            Some(attempt_id),
        )
    }

    pub fn stage_id(&self) -> &StageId {
        &self.stage_id
    }

    pub const fn stage_index(&self) -> usize {
        self.stage_index
    }

    pub const fn state(&self) -> StageProgressState {
        self.state
    }

    pub const fn latest_attempt_number(&self) -> u32 {
        self.latest_attempt_number
    }

    pub fn latest_attempt_id(&self) -> Option<&AttemptId> {
        self.latest_attempt_id.as_ref()
    }
}

/// Input admitted only after `SessionAggregateV1::new` validates all aggregate invariants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionAggregateInputV1 {
    pub session_id: SessionId,
    pub task_title: String,
    pub snapshot: ProcedureSnapshotV1,
    pub lifecycle: SessionLifecycle,
    pub revision: Revision,
    pub stage_progress: Vec<StageProgressV1>,
    pub attempts: Vec<AttemptV1>,
    pub active_stage_id: Option<StageId>,
    pub active_attempt_id: Option<AttemptId>,
    pub created_at: UnixMillis,
    pub completed_at: Option<UnixMillis>,
    pub cancelled_at: Option<UnixMillis>,
    pub cancel_reason: Option<String>,
}

/// A validated immutable session aggregate and its complete in-session attempt history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionAggregateV1 {
    session_id: SessionId,
    task_title: String,
    snapshot: ProcedureSnapshotV1,
    lifecycle: SessionLifecycle,
    revision: Revision,
    stage_progress: Vec<StageProgressV1>,
    attempts: Vec<AttemptV1>,
    active_stage_id: Option<StageId>,
    active_attempt_id: Option<AttemptId>,
    created_at: UnixMillis,
    completed_at: Option<UnixMillis>,
    cancelled_at: Option<UnixMillis>,
    cancel_reason: Option<String>,
}

impl SessionAggregateV1 {
    pub fn start(
        session_id: SessionId,
        task_title: impl Into<String>,
        snapshot: ProcedureSnapshotV1,
        first_attempt_id: AttemptId,
        created_at: UnixMillis,
    ) -> Result<Self, DomainError> {
        if created_at < snapshot.created_at() {
            return Err(invalid(
                "session creation timestamp precedes snapshot creation",
            ));
        }
        let first_stage = snapshot
            .stages()
            .first()
            .ok_or(invalid("procedure snapshot has no stages"))?;
        let first_attempt = AttemptV1::fresh(
            first_attempt_id,
            session_id.clone(),
            first_stage,
            1,
            created_at,
        )?;
        let mut stage_progress = Vec::with_capacity(snapshot.stages().len());
        stage_progress.push(StageProgressV1::current(
            first_stage.id().clone(),
            0,
            first_attempt.attempt_id().clone(),
            first_attempt.number(),
        )?);
        for (index, stage) in snapshot.stages().iter().enumerate().skip(1) {
            stage_progress.push(StageProgressV1::pending(stage.id().clone(), index));
        }
        let active_stage_id = first_stage.id().clone();
        let active_attempt_id = first_attempt.attempt_id().clone();
        Self::new(SessionAggregateInputV1 {
            session_id,
            task_title: task_title.into(),
            snapshot,
            lifecycle: SessionLifecycle::Running,
            revision: Revision::new(1),
            stage_progress,
            attempts: vec![first_attempt],
            active_stage_id: Some(active_stage_id),
            active_attempt_id: Some(active_attempt_id),
            created_at,
            completed_at: None,
            cancelled_at: None,
            cancel_reason: None,
        })
    }

    pub fn new(input: SessionAggregateInputV1) -> Result<Self, DomainError> {
        let SessionAggregateInputV1 {
            session_id,
            task_title,
            snapshot,
            lifecycle,
            revision,
            stage_progress,
            mut attempts,
            active_stage_id,
            active_attempt_id,
            created_at,
            completed_at,
            cancelled_at,
            cancel_reason,
        } = input;
        validate_title(&task_title)?;
        attempts.sort_by_key(|attempt| {
            (
                snapshot
                    .stages()
                    .iter()
                    .position(|stage| stage.id() == attempt.stage_id())
                    .unwrap_or(usize::MAX),
                attempt.number(),
            )
        });
        let aggregate = Self {
            session_id,
            task_title,
            snapshot,
            lifecycle,
            revision,
            stage_progress,
            attempts,
            active_stage_id,
            active_attempt_id,
            created_at,
            completed_at,
            cancelled_at,
            cancel_reason,
        };
        aggregate.validate()?;
        Ok(aggregate)
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn task_title(&self) -> &str {
        &self.task_title
    }

    pub fn snapshot(&self) -> &ProcedureSnapshotV1 {
        &self.snapshot
    }

    pub const fn lifecycle(&self) -> SessionLifecycle {
        self.lifecycle
    }

    pub const fn revision(&self) -> Revision {
        self.revision
    }

    pub fn stage_progress(&self) -> &[StageProgressV1] {
        &self.stage_progress
    }

    pub fn attempts(&self) -> &[AttemptV1] {
        &self.attempts
    }

    pub fn active_stage_id(&self) -> Option<&StageId> {
        self.active_stage_id.as_ref()
    }

    pub fn active_attempt_id(&self) -> Option<&AttemptId> {
        self.active_attempt_id.as_ref()
    }

    pub const fn created_at(&self) -> UnixMillis {
        self.created_at
    }

    pub const fn completed_at(&self) -> Option<UnixMillis> {
        self.completed_at
    }

    pub const fn cancelled_at(&self) -> Option<UnixMillis> {
        self.cancelled_at
    }

    pub fn cancel_reason(&self) -> Option<&str> {
        self.cancel_reason.as_deref()
    }

    pub fn latest_recorded_at(&self) -> UnixMillis {
        let mut latest = self.created_at.max(self.snapshot.created_at());
        for timestamp in self.completed_at.into_iter().chain(self.cancelled_at) {
            latest = latest.max(timestamp);
        }
        for attempt in &self.attempts {
            latest = latest.max(attempt.started_at());
            if let Some(ended_at) = attempt.ended_at() {
                latest = latest.max(ended_at);
            }
            for slot in attempt.item_slots() {
                if let Some(created_at) = slot.created_at() {
                    latest = latest.max(created_at);
                }
                if let Some(updated_at) = slot.updated_at() {
                    latest = latest.max(updated_at);
                }
            }
            for blocker in attempt.blockers() {
                latest = latest.max(blocker.created_at());
                if let Some(resolved_at) = blocker.resolved_at() {
                    latest = latest.max(resolved_at);
                }
            }
        }
        latest
    }
    pub(crate) fn latest_attempt_boundary_at(&self) -> UnixMillis {
        let mut latest = UnixMillis::UNIX_EPOCH;
        for attempt in &self.attempts {
            latest = latest.max(attempt.started_at());
            if let Some(ended_at) = attempt.ended_at() {
                latest = latest.max(ended_at);
            }
        }
        latest
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.revision == Revision::ZERO {
            return Err(invalid("a session aggregate revision must be positive"));
        }
        let revision_floor = conservative_revision_floor(&self.snapshot, &self.attempts)?;
        if self.revision < revision_floor {
            return Err(invalid(
                "session aggregate revision is below the conservative retained-mutation floor",
            ));
        }
        if self.created_at < self.snapshot.created_at() {
            return Err(invalid(
                "session creation timestamp precedes snapshot creation",
            ));
        }
        validate_session_times(
            self.lifecycle,
            self.created_at,
            self.completed_at,
            self.cancelled_at,
            self.cancel_reason.as_deref(),
        )?;
        validate_stage_progress_order(&self.snapshot, &self.stage_progress)?;
        validate_attempt_history(
            &self.session_id,
            &self.snapshot,
            &self.stage_progress,
            &self.attempts,
            SessionHistoryMetadata {
                lifecycle: self.lifecycle,
                created_at: self.created_at,
                terminal_at: self.completed_at.or(self.cancelled_at),
                cancel_reason: self.cancel_reason.as_deref(),
            },
        )?;
        validate_active_cursor(
            self.lifecycle,
            self.active_stage_id.as_ref(),
            self.active_attempt_id.as_ref(),
            &self.stage_progress,
            &self.attempts,
        )?;
        validate_progress_frontier(self.lifecycle, &self.stage_progress)?;
        Ok(())
    }
}

fn attempt_chronological_key(attempt: &AttemptV1) -> (UnixMillis, u64) {
    (
        attempt.started_at(),
        attempt.ended_at().map_or(u64::MAX, UnixMillis::get),
    )
}

fn chronological_attempts(attempts: &[AttemptV1]) -> Vec<&AttemptV1> {
    let mut chronological_attempts: Vec<_> = attempts.iter().collect();
    chronological_attempts.sort_by_key(|attempt| attempt_chronological_key(attempt));
    chronological_attempts
}

fn conservative_revision_floor(
    snapshot: &ProcedureSnapshotV1,
    attempts: &[AttemptV1],
) -> Result<Revision, DomainError> {
    let chronological_attempts = chronological_attempts(attempts);
    let mut floor = Revision::new(1);
    let final_stage_id = snapshot.stages().last().map(StageSpecV1::id);

    for (index, attempt) in chronological_attempts.iter().enumerate() {
        if index > 0 {
            floor = floor.checked_next()?;

            let previous = chronological_attempts[index - 1];
            if attempt.reason().is_some()
                && final_stage_id.is_some_and(|stage_id| previous.stage_id() == stage_id)
                && matches!(
                    previous.lifecycle(),
                    AttemptLifecycle::Completed | AttemptLifecycle::Skipped
                )
            {
                floor = floor.checked_next()?;
            }
        }

        for slot in attempt.item_slots() {
            floor = Revision::new(
                floor
                    .get()
                    .checked_add(slot.revision().get())
                    .ok_or(DomainError::RevisionOverflow { revision: floor })?,
            );
        }

        let mut explicit_resolution_times = BTreeSet::new();
        // A single unblock-all transition can resolve multiple blockers at one timestamp.
        for blocker in attempt.blockers() {
            floor = floor.checked_next()?;
            if let Some(resolved_at) = blocker.resolved_at() {
                // Skip and abandon terminalize all open blockers in the same transition, so only
                // earlier resolutions prove an additional retained mutation.
                let must_be_explicit = match attempt.lifecycle() {
                    AttemptLifecycle::Active | AttemptLifecycle::Completed => true,
                    AttemptLifecycle::Skipped | AttemptLifecycle::Abandoned => attempt
                        .ended_at()
                        .is_some_and(|ended_at| resolved_at < ended_at),
                };
                if must_be_explicit {
                    explicit_resolution_times.insert(resolved_at);
                }
            }
        }
        for _ in explicit_resolution_times {
            floor = floor.checked_next()?;
        }
    }

    if chronological_attempts
        .last()
        .is_some_and(|attempt| attempt.lifecycle() != AttemptLifecycle::Active)
    {
        floor = floor.checked_next()?;
    }

    Ok(floor)
}
/// Required items block completion; absent optional values do not.
pub fn required_items_satisfied(stage: &StageSpecV1, slots: &[ItemSlotV1]) -> bool {
    stage
        .items()
        .iter()
        .filter(|item| item.common().required())
        .all(|item| {
            slots
                .iter()
                .find(|slot| slot.item_id() == item.id())
                .is_some_and(|slot| item_satisfied(item, slot.value()))
        })
}

fn item_value_admissible(specification: &ItemSpecV1, value: &ItemValueV1) -> bool {
    match specification {
        ItemSpecV1::Confirm(_) => matches!(&value.kind, ItemValueKindV1::Confirm),
        ItemSpecV1::Text(specification) => match &value.kind {
            ItemValueKindV1::Text(value) => text_is_storable(specification, value),
            _ => false,
        },
        ItemSpecV1::Choice(specification) => match &value.kind {
            ItemValueKindV1::Choice(value) => specification.choices().contains(value),
            _ => false,
        },
        ItemSpecV1::Integer(specification) => match &value.kind {
            ItemValueKindV1::Integer(value) => {
                specification
                    .minimum()
                    .is_none_or(|minimum| *value >= minimum)
                    && specification
                        .maximum()
                        .is_none_or(|maximum| *value <= maximum)
            }
            _ => false,
        },
        ItemSpecV1::List(specification) => match &value.kind {
            ItemValueKindV1::List(values) => list_is_storable(specification, values),
            _ => false,
        },
        ItemSpecV1::Artifact(specification) => match &value.kind {
            ItemValueKindV1::Artifact(value) => artifact_satisfies(specification, value),
            _ => false,
        },
    }
}

fn text_is_storable(specification: &TextItemSpecV1, value: &str) -> bool {
    value.chars().count() <= crate::procedure::MAX_TEXT_LENGTH as usize
        && value.trim().chars().count() <= specification.max_length() as usize
}

fn text_satisfies(specification: &TextItemSpecV1, value: &str) -> bool {
    let trimmed_length = value.trim().chars().count();
    trimmed_length >= specification.min_length() as usize
        && trimmed_length <= specification.max_length() as usize
}

fn list_is_storable(specification: &ListItemSpecV1, values: &[String]) -> bool {
    if values.len() > specification.max_items() as usize {
        return false;
    }
    if values.iter().any(|value| {
        value.trim().is_empty() || value.chars().count() > specification.max_item_length() as usize
    }) {
        return false;
    }
    if specification.unique() {
        let mut seen = BTreeSet::new();
        for value in values {
            if !seen.insert(value.as_str()) {
                return false;
            }
        }
    }
    true
}

fn artifact_satisfies(specification: &ArtifactItemSpecV1, value: &ArtifactValueV1) -> bool {
    let location_is_valid = match value.location_kind() {
        ArtifactLocationKindV1::LocalPath => validate_safe_local_path(value.location()).is_ok(),
        ArtifactLocationKindV1::ExternalReference => {
            validate_external_reference(value.location()).is_ok()
        }
    };
    location_is_valid
        && validate_media_type(value.media_type()).is_ok()
        && (specification.allowed_media_types().is_empty()
            || specification
                .allowed_media_types()
                .iter()
                .any(|media_type| media_type == value.media_type()))
}

fn validate_safe_local_path(path: &str) -> Result<(), DomainError> {
    if path.chars().count() > 4_000 {
        return Err(invalid(
            "local artifact path must contain at most 4000 scalars",
        ));
    }
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains('\0')
        || path.ends_with('/')
        || path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':'
    {
        return Err(invalid(
            "local artifact path must be normalized and worktree-relative",
        ));
    }
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(invalid("local artifact path must not escape the worktree"));
        }
    }
    Ok(())
}

fn validate_external_reference(reference: &str) -> Result<(), DomainError> {
    if reference.is_empty() || reference.chars().count() > 4_000 || reference.contains('\0') {
        return Err(invalid(
            "external artifact reference must be a non-empty opaque string",
        ));
    }
    Ok(())
}

fn validate_reason(reason: &str) -> Result<(), DomainError> {
    let length = reason.chars().count();
    if reason.trim().is_empty() || length > 4_000 {
        return Err(invalid(
            "reason must contain at most 4000 non-blank scalars",
        ));
    }
    Ok(())
}

fn validate_title(title: &str) -> Result<(), DomainError> {
    let length = title.chars().count();
    if title.trim().is_empty() || length > 500 {
        return Err(invalid(
            "task title must contain between one and 500 non-blank scalars",
        ));
    }
    Ok(())
}

fn validate_attempt_lifecycle(
    lifecycle: AttemptLifecycle,
    started_at: UnixMillis,
    ended_at: Option<UnixMillis>,
    reason: Option<&str>,
) -> Result<(), DomainError> {
    match lifecycle {
        AttemptLifecycle::Active if ended_at.is_none() => Ok(()),
        AttemptLifecycle::Completed if ended_at.is_some_and(|ended_at| ended_at >= started_at) => {
            Ok(())
        }
        AttemptLifecycle::Skipped if ended_at.is_some_and(|ended_at| ended_at >= started_at) => {
            Ok(())
        }
        AttemptLifecycle::Abandoned
            if ended_at.is_some_and(|ended_at| ended_at >= started_at) && reason.is_some() =>
        {
            Ok(())
        }
        _ => Err(invalid("attempt lifecycle metadata is inconsistent")),
    }
}
fn validate_terminal_attempt(
    stage: &StageSpecV1,
    lifecycle: AttemptLifecycle,
    reason: Option<&str>,
    slots: &[ItemSlotV1],
    blockers: &[BlockerV1],
) -> Result<(), DomainError> {
    match lifecycle {
        AttemptLifecycle::Active => Ok(()),
        AttemptLifecycle::Completed
            if required_items_satisfied(stage, slots)
                && blockers
                    .iter()
                    .all(|blocker| blocker.state() != BlockerState::Open) =>
        {
            Ok(())
        }
        AttemptLifecycle::Completed => Err(invalid(
            "completed attempts require satisfied required items and no open blockers",
        )),
        AttemptLifecycle::Skipped if !stage.skip_policy().is_allowed() => {
            Err(invalid("the active stage may not be skipped"))
        }
        AttemptLifecycle::Skipped if stage.skip_policy().reason_required() && reason.is_none() => {
            Err(invalid("a non-empty reason is required"))
        }
        AttemptLifecycle::Skipped => Ok(()),
        AttemptLifecycle::Abandoned if reason.is_some() => Ok(()),
        AttemptLifecycle::Abandoned => Err(invalid("a non-empty reason is required")),
    }
}

fn validate_attempt_record_times(
    started_at: UnixMillis,
    ended_at: Option<UnixMillis>,
    slots: &[ItemSlotV1],
    blockers: &[BlockerV1],
) -> Result<(), DomainError> {
    for slot in slots {
        match (slot.created_at(), slot.updated_at()) {
            (Some(created_at), Some(updated_at))
                if created_at < started_at
                    || updated_at < started_at
                    || ended_at
                        .is_some_and(|ended_at| created_at > ended_at || updated_at > ended_at) =>
            {
                return Err(invalid(
                    "item timestamps must fall within the attempt lifetime",
                ));
            }
            _ => {}
        }
    }
    for blocker in blockers {
        if blocker.created_at() < started_at
            || blocker
                .resolved_at()
                .is_some_and(|resolved_at| resolved_at < started_at)
            || ended_at.is_some_and(|ended_at| {
                blocker.created_at() > ended_at
                    || blocker
                        .resolved_at()
                        .is_some_and(|resolved_at| resolved_at > ended_at)
            })
        {
            return Err(invalid(
                "blocker timestamps must fall within the attempt lifetime",
            ));
        }
    }
    Ok(())
}
fn validate_slots(
    attempt_id: &AttemptId,
    stage: &StageSpecV1,
    slots: &[ItemSlotV1],
) -> Result<(), DomainError> {
    if slots.len() != stage.items().len() {
        return Err(invalid(
            "attempt must have exactly one slot for every stage item",
        ));
    }
    for (slot, specification) in slots.iter().zip(stage.items()) {
        if slot.attempt_id() != attempt_id {
            return Err(invalid("item slot belongs to another attempt"));
        }
        slot.validate_for(specification)?;
    }
    Ok(())
}

fn validate_blockers(
    attempt_id: &AttemptId,
    lifecycle: AttemptLifecycle,
    blockers: &[BlockerV1],
) -> Result<(), DomainError> {
    if blockers.len() > MAX_BLOCKERS_PER_ATTEMPT_V1 {
        return Err(invalid("attempt blocker count exceeds the v1 limit"));
    }
    let mut ids = BTreeSet::new();
    for blocker in blockers {
        if blocker.attempt_id() != attempt_id {
            return Err(invalid("blocker belongs to another attempt"));
        }
        if !ids.insert(blocker.blocker_id()) {
            return Err(invalid("attempt blocker identifiers must be unique"));
        }
        if lifecycle != AttemptLifecycle::Active && blocker.state() == BlockerState::Open {
            return Err(invalid("non-active attempts cannot retain open blockers"));
        }
    }
    Ok(())
}
fn validate_session_times(
    lifecycle: SessionLifecycle,
    created_at: UnixMillis,
    completed_at: Option<UnixMillis>,
    cancelled_at: Option<UnixMillis>,
    cancel_reason: Option<&str>,
) -> Result<(), DomainError> {
    match lifecycle {
        SessionLifecycle::Running
            if completed_at.is_none() && cancelled_at.is_none() && cancel_reason.is_none() =>
        {
            Ok(())
        }
        SessionLifecycle::Completed
            if completed_at.is_some_and(|completed_at| completed_at >= created_at)
                && cancelled_at.is_none()
                && cancel_reason.is_none() =>
        {
            Ok(())
        }
        SessionLifecycle::Cancelled
            if cancelled_at.is_some_and(|cancelled_at| cancelled_at >= created_at)
                && completed_at.is_none()
                && cancel_reason.is_some_and(|reason| validate_reason(reason).is_ok()) =>
        {
            Ok(())
        }
        _ => Err(invalid("session lifecycle timestamps are inconsistent")),
    }
}

fn validate_stage_progress_order(
    snapshot: &ProcedureSnapshotV1,
    progress: &[StageProgressV1],
) -> Result<(), DomainError> {
    if progress.len() != snapshot.stages().len() {
        return Err(invalid(
            "session progress must contain every snapshot stage",
        ));
    }
    for (index, (stage, progress)) in snapshot.stages().iter().zip(progress).enumerate() {
        if progress.stage_index() != index || progress.stage_id() != stage.id() {
            return Err(invalid(
                "stage progress order must match the immutable snapshot",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct SessionHistoryMetadata<'a> {
    lifecycle: SessionLifecycle,
    created_at: UnixMillis,
    terminal_at: Option<UnixMillis>,
    cancel_reason: Option<&'a str>,
}

fn validate_attempt_history(
    session_id: &SessionId,
    snapshot: &ProcedureSnapshotV1,
    progress: &[StageProgressV1],
    attempts: &[AttemptV1],
    session: SessionHistoryMetadata<'_>,
) -> Result<(), DomainError> {
    if attempts.is_empty() {
        return Err(invalid("session attempt history must not be empty"));
    }
    let first_stage = snapshot.stages().first().ok_or(invalid(
        "procedure snapshot must contain at least one stage",
    ))?;
    let final_stage = snapshot.stages().last().ok_or(invalid(
        "procedure snapshot must contain at least one stage",
    ))?;
    let stage_positions: BTreeMap<&StageId, usize> = snapshot
        .stages()
        .iter()
        .enumerate()
        .map(|(index, stage)| (stage.id(), index))
        .collect();
    let mut attempt_ids = BTreeSet::new();
    let mut blocker_ids = BTreeSet::new();
    let mut attempts_by_stage: BTreeMap<&StageId, Vec<&AttemptV1>> = BTreeMap::new();

    for attempt in attempts {
        let Some(&attempt_stage_index) = stage_positions.get(attempt.stage_id()) else {
            return Err(invalid(
                "attempt stage is absent from the immutable snapshot",
            ));
        };
        if attempt.session_id() != session_id {
            return Err(invalid("attempt belongs to another session"));
        }
        if attempt.started_at() < session.created_at
            || session.terminal_at.is_some_and(|terminal_at| {
                attempt.started_at() > terminal_at
                    || attempt
                        .ended_at()
                        .is_some_and(|ended_at| ended_at > terminal_at)
            })
        {
            return Err(invalid(
                "attempt timestamps must fall within the session lifetime",
            ));
        }
        if !attempt_ids.insert(attempt.attempt_id()) {
            return Err(invalid("session attempt identifiers must be unique"));
        }
        let stage = &snapshot.stages()[attempt_stage_index];
        validate_slots(attempt.attempt_id(), stage, attempt.item_slots())?;
        validate_blockers(
            attempt.attempt_id(),
            attempt.lifecycle(),
            attempt.blockers(),
        )?;
        validate_terminal_attempt(
            stage,
            attempt.lifecycle(),
            attempt.reason(),
            attempt.item_slots(),
            attempt.blockers(),
        )?;
        validate_attempt_record_times(
            attempt.started_at(),
            attempt.ended_at(),
            attempt.item_slots(),
            attempt.blockers(),
        )?;
        for blocker in attempt.blockers() {
            if !blocker_ids.insert(blocker.blocker_id()) {
                return Err(invalid("session blocker identifiers must be unique"));
            }
        }

        let stage_attempts = attempts_by_stage.entry(attempt.stage_id()).or_default();
        let expected_number = match stage_attempts.last() {
            Some(previous) => previous
                .number()
                .checked_add(1)
                .ok_or(invalid("attempt number exceeds supported range"))?,
            None => 1,
        };
        if attempt.number() != expected_number {
            return Err(invalid(
                "attempt numbers must increase by one within their stage",
            ));
        }
        if let Some(previous) = stage_attempts.last() {
            let previous_ended_at = previous
                .ended_at()
                .ok_or(invalid("only the latest stage attempt may be active"))?;
            if attempt.started_at() < previous_ended_at {
                return Err(invalid(
                    "attempt timestamps must be monotonic within their stage",
                ));
            }
        }
        stage_attempts.push(attempt);
    }
    let chronological_attempts = chronological_attempts(attempts);
    let mut prior_end = None;
    let mut active_seen = false;
    let mut previous_attempt: Option<&AttemptV1> = None;
    for attempt in chronological_attempts {
        if active_seen || prior_end.is_some_and(|ended_at| attempt.started_at() < ended_at) {
            return Err(invalid("session attempts must not overlap"));
        }

        if let Some(previous) = previous_attempt {
            if attempt_chronological_key(previous) == attempt_chronological_key(attempt) {
                return Err(invalid(
                    "attempt chronology is ambiguous at millisecond precision",
                ));
            }
            let previous_stage_index = *stage_positions.get(previous.stage_id()).ok_or(invalid(
                "attempt stage is absent from the immutable snapshot",
            ))?;
            let attempt_stage_index = *stage_positions.get(attempt.stage_id()).ok_or(invalid(
                "attempt stage is absent from the immutable snapshot",
            ))?;
            validate_chronological_attempt_transition(
                snapshot,
                previous,
                previous_stage_index,
                attempt,
                attempt_stage_index,
            )?;
        } else if attempt.stage_id() != first_stage.id()
            || attempt.number() != 1
            || attempt.started_at() != session.created_at
        {
            return Err(invalid(
                "the first stage attempt must start at session creation",
            ));
        }

        match attempt.ended_at() {
            Some(ended_at) => prior_end = Some(ended_at),
            None => active_seen = true,
        }
        previous_attempt = Some(attempt);
    }

    for (stage, progress) in snapshot.stages().iter().zip(progress) {
        let stage_attempts = attempts_by_stage.get(stage.id());
        match stage_attempts {
            None if progress.latest_attempt_number() == 0
                && progress.latest_attempt_id().is_none() => {}
            Some(stage_attempts) => {
                let latest = stage_attempts
                    .last()
                    .copied()
                    .ok_or(invalid("stage attempt history is empty"))?;
                if progress.latest_attempt_number() != latest.number()
                    || progress.latest_attempt_id() != Some(latest.attempt_id())
                {
                    return Err(invalid(
                        "stage progress latest attempt does not match history",
                    ));
                }
                validate_latest_attempt_state(progress.state(), latest.lifecycle())?;
            }
            None => {
                return Err(invalid(
                    "stage progress latest attempt does not match history",
                ));
            }
        }
    }

    match session.lifecycle {
        SessionLifecycle::Running => {}
        SessionLifecycle::Completed => {
            let terminal_at = session
                .terminal_at
                .ok_or(invalid("completed sessions require a terminal timestamp"))?;
            let final_attempt = attempts_by_stage
                .get(final_stage.id())
                .and_then(|stage_attempts| stage_attempts.last())
                .copied()
                .ok_or(invalid("completed sessions require a final-stage attempt"))?;
            if !matches!(
                final_attempt.lifecycle(),
                AttemptLifecycle::Completed | AttemptLifecycle::Skipped
            ) || final_attempt.ended_at() != Some(terminal_at)
            {
                return Err(invalid(
                    "completed session does not align with its final-stage attempt",
                ));
            }
            if previous_attempt
                .is_none_or(|attempt| attempt.attempt_id() != final_attempt.attempt_id())
            {
                return Err(invalid(
                    "completed session terminal attempt is not chronologically last",
                ));
            }
        }
        SessionLifecycle::Cancelled => {
            let terminal_at = session
                .terminal_at
                .ok_or(invalid("cancelled sessions require a terminal timestamp"))?;
            let abandoned_progress: Vec<_> = progress
                .iter()
                .filter(|entry| entry.state() == StageProgressState::Abandoned)
                .collect();
            if abandoned_progress.len() != 1 {
                return Err(invalid(
                    "cancelled sessions require exactly one abandoned stage",
                ));
            }
            let cancelled_attempt = attempts_by_stage
                .get(abandoned_progress[0].stage_id())
                .and_then(|stage_attempts| stage_attempts.last())
                .copied()
                .ok_or(invalid("cancelled stage has no latest attempt"))?;
            if cancelled_attempt.lifecycle() != AttemptLifecycle::Abandoned
                || cancelled_attempt.ended_at() != Some(terminal_at)
                || cancelled_attempt.reason() != session.cancel_reason
            {
                return Err(invalid(
                    "cancelled session does not align with its abandoned attempt",
                ));
            }
            if previous_attempt
                .is_none_or(|attempt| attempt.attempt_id() != cancelled_attempt.attempt_id())
            {
                return Err(invalid(
                    "cancelled session terminal attempt is not chronologically last",
                ));
            }
        }
    }
    Ok(())
}
fn validate_chronological_attempt_transition(
    snapshot: &ProcedureSnapshotV1,
    previous: &AttemptV1,
    previous_stage_index: usize,
    attempt: &AttemptV1,
    attempt_stage_index: usize,
) -> Result<(), DomainError> {
    let reason_bearing = attempt.reason().is_some();
    let follows_completed_final_stage = previous_stage_index + 1 == snapshot.stages().len()
        && matches!(
            previous.lifecycle(),
            AttemptLifecycle::Completed | AttemptLifecycle::Skipped
        );
    let requires_reopen_validation = reason_bearing
        && (matches!(
            attempt.lifecycle(),
            AttemptLifecycle::Active | AttemptLifecycle::Completed
        ) || follows_completed_final_stage);

    if requires_reopen_validation {
        if !follows_completed_final_stage {
            return Err(invalid(
                "reason-bearing active or completed attempts must follow a completed final-stage attempt",
            ));
        }
        if attempt.number() == 1 {
            return Err(invalid(
                "reason-bearing reopened attempts must have a prior destination attempt",
            ));
        }
        if !snapshot
            .return_policy()
            .allows_destination(attempt.stage_id())
        {
            return Err(invalid(
                "reason-bearing reopened attempts must target an allowed return destination",
            ));
        }
        return Ok(());
    }

    if attempt_stage_index == previous_stage_index {
        if previous.lifecycle() != AttemptLifecycle::Abandoned || previous.reason().is_none() {
            return Err(invalid(
                "same-stage retries must follow an abandoned reason-bearing attempt",
            ));
        }
        return Ok(());
    }

    if attempt_stage_index < previous_stage_index {
        if previous.lifecycle() != AttemptLifecycle::Abandoned || previous.reason().is_none() {
            return Err(invalid(
                "returns must follow an abandoned reason-bearing attempt",
            ));
        }
        if !snapshot
            .return_policy()
            .allows_destination(attempt.stage_id())
        {
            return Err(invalid(
                "return destination is not allowed by the immutable procedure",
            ));
        }
        return Ok(());
    }

    if attempt_stage_index != previous_stage_index + 1 {
        return Err(invalid("attempt history cannot skip stages"));
    }
    if !matches!(
        previous.lifecycle(),
        AttemptLifecycle::Completed | AttemptLifecycle::Skipped
    ) {
        return Err(invalid(
            "ordinary stage advancement requires a completed or skipped predecessor",
        ));
    }
    Ok(())
}
fn validate_latest_attempt_state(
    state: StageProgressState,
    lifecycle: AttemptLifecycle,
) -> Result<(), DomainError> {
    match (state, lifecycle) {
        (StageProgressState::Current, AttemptLifecycle::Active)
        | (StageProgressState::Done, AttemptLifecycle::Completed)
        | (StageProgressState::Skipped, AttemptLifecycle::Skipped)
        | (StageProgressState::Abandoned, AttemptLifecycle::Abandoned)
        | (StageProgressState::Redo, AttemptLifecycle::Completed)
        | (StageProgressState::Redo, AttemptLifecycle::Skipped)
        | (StageProgressState::Redo, AttemptLifecycle::Abandoned) => Ok(()),
        _ => Err(invalid(
            "stage progress state does not match the latest attempt lifecycle",
        )),
    }
}

fn validate_active_cursor(
    lifecycle: SessionLifecycle,
    active_stage_id: Option<&StageId>,
    active_attempt_id: Option<&AttemptId>,
    progress: &[StageProgressV1],
    attempts: &[AttemptV1],
) -> Result<(), DomainError> {
    let active_attempts: Vec<_> = attempts
        .iter()
        .filter(|attempt| attempt.lifecycle() == AttemptLifecycle::Active)
        .collect();
    let current_progress: Vec<_> = progress
        .iter()
        .filter(|progress| progress.state() == StageProgressState::Current)
        .collect();
    match lifecycle {
        SessionLifecycle::Running => {
            let (Some(active_stage_id), Some(active_attempt_id)) =
                (active_stage_id, active_attempt_id)
            else {
                return Err(invalid("running sessions require an active cursor"));
            };
            if active_attempts.len() != 1 || current_progress.len() != 1 {
                return Err(invalid(
                    "running sessions require exactly one active attempt and stage",
                ));
            }
            let attempt = active_attempts[0];
            let stage = current_progress[0];
            if attempt.attempt_id() != active_attempt_id
                || attempt.stage_id() != active_stage_id
                || stage.stage_id() != active_stage_id
                || stage.latest_attempt_id() != Some(active_attempt_id)
            {
                return Err(invalid(
                    "active cursor does not align with progress and attempt",
                ));
            }
            for attempt in attempts {
                if attempt.attempt_id() != active_attempt_id
                    && attempt
                        .blockers()
                        .iter()
                        .any(|blocker| blocker.state() == BlockerState::Open)
                {
                    return Err(invalid("open blockers must belong to the active attempt"));
                }
            }
            Ok(())
        }
        SessionLifecycle::Completed | SessionLifecycle::Cancelled => {
            if active_stage_id.is_some()
                || active_attempt_id.is_some()
                || !active_attempts.is_empty()
                || !current_progress.is_empty()
            {
                return Err(invalid(
                    "terminal sessions must not retain an active cursor or attempt",
                ));
            }
            Ok(())
        }
    }
}

fn validate_progress_frontier(
    lifecycle: SessionLifecycle,
    progress: &[StageProgressV1],
) -> Result<(), DomainError> {
    match lifecycle {
        SessionLifecycle::Running => {
            let mut phase = 0;
            for progress in progress {
                phase = match (phase, progress.state()) {
                    (0, StageProgressState::Done | StageProgressState::Skipped) => 0,
                    (0, StageProgressState::Current) => 1,
                    (1, StageProgressState::Redo) | (2, StageProgressState::Redo) => 2,
                    (1, StageProgressState::Pending)
                    | (2, StageProgressState::Pending)
                    | (3, StageProgressState::Pending) => 3,
                    _ => return Err(invalid("running session stage progress is inconsistent")),
                };
            }
            if phase == 0 {
                return Err(invalid(
                    "running session stage progress lacks a current stage",
                ));
            }
            Ok(())
        }
        SessionLifecycle::Completed
            if progress.iter().all(|progress| {
                matches!(
                    progress.state(),
                    StageProgressState::Done | StageProgressState::Skipped
                )
            }) =>
        {
            Ok(())
        }
        SessionLifecycle::Cancelled => {
            let mut phase = 0;
            for progress in progress {
                phase = match (phase, progress.state()) {
                    (0, StageProgressState::Done | StageProgressState::Skipped) => 0,
                    (0, StageProgressState::Abandoned) => 1,
                    (1, StageProgressState::Redo) | (2, StageProgressState::Redo) => 2,
                    (1, StageProgressState::Pending)
                    | (2, StageProgressState::Pending)
                    | (3, StageProgressState::Pending) => 3,
                    _ => return Err(invalid("cancelled session stage progress is inconsistent")),
                };
            }
            if phase == 0 {
                return Err(invalid(
                    "cancelled session stage progress lacks an abandoned stage",
                ));
            }
            Ok(())
        }
        SessionLifecycle::Completed => {
            Err(invalid("completed session stage progress is inconsistent"))
        }
    }
}

const fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}
