//! Pure Procedure v2 workflow-memory record values: recorded items, resolved evidence references,
//! decision records, and rework records.
//!
//! These additive, individually-bounded immutable values follow ADR-0007 (typed items, not a
//! general evidence ledger), ADR-0009 (artifact metadata only), and ADR-0016 (recorded items as
//! workflow memory). Each constructor enforces only the identifier, scalar, collection, uniqueness,
//! and cross-field bounds owned by that single value. Attempt-local accumulation, freshness
//! derivation, persistence, graph-wide validation, canonicalization, digest computation, protocol
//! projection, and runtime admission are owned by later tasks. The resolved-reference `stale` state
//! is deliberately not represented here: section 8.4 derives it from attempt validity and never
//! stores it on the immutable snapshot.

use std::fmt;

use crate::aggregate::ArtifactValueV1;
use crate::procedure::{ItemTypeV1, validate_text};
use crate::procedure_v2::TransitionEffectV2;
use crate::session_v2::{AttemptNumberV2, GoalRevisionNumberV2, TraceSequenceV2};
use crate::{
    AttemptId, DomainError, GraphNodeId, ItemId, NodeDefinitionId, OptionId, ProcedureSnapshotId,
    Revision, SessionId, Sha256Digest, UnixMillis,
};

// Scalar bounds fixed by dossier sections 5.1 and 6.4.
const MAX_RECORD_REASON_CHARS: usize = 2_000;
const MAX_ACTOR_ATTRIBUTION_CHARS: usize = 256;

// Recorded-item bounds fixed by dossier section 5.1 (v2 hard limits).
const MAX_RECORDED_TEXT_CHARS: usize = 16_384;
const MAX_RECORDED_CHOICE_CHARS: usize = 120;
const MAX_RECORDED_LIST_ENTRIES: usize = 200;
const MAX_RECORDED_LIST_ENTRY_CHARS: usize = 1_000;
const MAX_RECORDED_ITEMS_PER_ATTEMPT: usize = 64;

// Evidence and record collection bounds fixed by dossier sections 5.1 and 6.4.
const MAX_RESOLVED_REFERENCES: usize = 8;

const fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}

/// A bounded non-blank reason carried by a decision, rework, retry, or skip record. The reason is
/// always required for these records (dossier section 6.4); criterion-assessment and goal-revision
/// reasons reuse their own existing bounded types.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReasonV2(String);

impl ReasonV2 {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_text("record reason", &value, 1, MAX_RECORD_REASON_CHARS, true)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for ReasonV2 {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ReasonV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Optional caller-supplied actor attribution. It is never cryptographic authority (dossier section
/// 13.6); it is a bounded opaque attribution string shared by every workflow-memory record family.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ActorAttributionV2(String);

impl ActorAttributionV2 {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_text(
            "actor attribution",
            &value,
            1,
            MAX_ACTOR_ATTRIBUTION_CHARS,
            true,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for ActorAttributionV2 {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ActorAttributionV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RecordedItemValueKindV2 {
    Confirm,
    Text(String),
    Choice(String),
    Integer(i64),
    List(Vec<String>),
    Artifact(ArtifactValueV1),
}

/// A typed recorded item value under the v2 hard bounds of section 5.1. An attempt's recorded item
/// values are its only durable work record (dossier section 6.1); artifact values carry metadata
/// only and never bytes (ADR-0009). Satisfaction against an item specification is owned by V2RUN.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedItemValueV2 {
    kind: RecordedItemValueKindV2,
}

impl RecordedItemValueV2 {
    pub const fn confirm() -> Self {
        Self {
            kind: RecordedItemValueKindV2::Confirm,
        }
    }

    pub fn text(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_text(
            "recorded text value",
            &value,
            0,
            MAX_RECORDED_TEXT_CHARS,
            false,
        )?;
        Ok(Self {
            kind: RecordedItemValueKindV2::Text(value),
        })
    }

    pub fn choice(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_text(
            "recorded choice value",
            &value,
            1,
            MAX_RECORDED_CHOICE_CHARS,
            false,
        )?;
        Ok(Self {
            kind: RecordedItemValueKindV2::Choice(value),
        })
    }

    pub const fn integer(value: i64) -> Self {
        Self {
            kind: RecordedItemValueKindV2::Integer(value),
        }
    }

    pub fn list(values: Vec<String>) -> Result<Self, DomainError> {
        if values.len() > MAX_RECORDED_LIST_ENTRIES {
            return Err(invalid("recorded list value exceeds the v2 entry maximum"));
        }
        for entry in &values {
            validate_text(
                "recorded list entry",
                entry,
                1,
                MAX_RECORDED_LIST_ENTRY_CHARS,
                true,
            )?;
        }
        Ok(Self {
            kind: RecordedItemValueKindV2::List(values),
        })
    }

    pub fn artifact(value: ArtifactValueV1) -> Self {
        Self {
            kind: RecordedItemValueKindV2::Artifact(value),
        }
    }

    pub const fn item_type(&self) -> ItemTypeV1 {
        match self.kind {
            RecordedItemValueKindV2::Confirm => ItemTypeV1::Confirm,
            RecordedItemValueKindV2::Text(_) => ItemTypeV1::Text,
            RecordedItemValueKindV2::Choice(_) => ItemTypeV1::Choice,
            RecordedItemValueKindV2::Integer(_) => ItemTypeV1::Integer,
            RecordedItemValueKindV2::List(_) => ItemTypeV1::List,
            RecordedItemValueKindV2::Artifact(_) => ItemTypeV1::Artifact,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match &self.kind {
            RecordedItemValueKindV2::Text(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_choice(&self) -> Option<&str> {
        match &self.kind {
            RecordedItemValueKindV2::Choice(value) => Some(value),
            _ => None,
        }
    }

    pub const fn as_integer(&self) -> Option<i64> {
        match self.kind {
            RecordedItemValueKindV2::Integer(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[String]> {
        match &self.kind {
            RecordedItemValueKindV2::List(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_artifact(&self) -> Option<&ArtifactValueV1> {
        match &self.kind {
            RecordedItemValueKindV2::Artifact(value) => Some(value),
            _ => None,
        }
    }
}

/// One recorded item: its stable identifier and its bounded typed value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedItemV2 {
    id: ItemId,
    value: RecordedItemValueV2,
}

impl RecordedItemV2 {
    pub const fn new(id: ItemId, value: RecordedItemValueV2) -> Self {
        Self { id, value }
    }

    pub fn id(&self) -> &ItemId {
        &self.id
    }

    pub fn value(&self) -> &RecordedItemValueV2 {
        &self.value
    }
}

/// The complete immutable snapshot of recorded item values for one terminal attempt, preserving
/// author order. `items_digest` (dossier section 8.2) attests to exactly this complete set and is
/// computed by canonicalization (V2MOD-007); the snapshot itself is the immutable value form.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedItemSetV2 {
    items: Vec<RecordedItemV2>,
}

impl RecordedItemSetV2 {
    pub fn new(items: Vec<RecordedItemV2>) -> Result<Self, DomainError> {
        if items.len() > MAX_RECORDED_ITEMS_PER_ATTEMPT {
            return Err(invalid("an attempt holds at most 64 recorded items"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for item in &items {
            if !seen.insert(item.id()) {
                return Err(invalid("recorded item identifiers must be unique"));
            }
        }
        Ok(Self { items })
    }

    pub fn items(&self) -> &[RecordedItemV2] {
        &self.items
    }
}

/// The shared resolved snapshot data for a resolved or skipped evidence reference: the source
/// attempt identity, the digest attesting to its complete recorded item values, and the resolution
/// timestamp (dossier section 8.2). A skipped source carries this same attestation over its empty
/// recorded item set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceReferenceSnapshotV2 {
    source_node: GraphNodeId,
    source_attempt_id: AttemptId,
    source_attempt_number: AttemptNumberV2,
    items_digest: Sha256Digest,
    resolved_at: UnixMillis,
}

impl EvidenceReferenceSnapshotV2 {
    /// Reconstructs a resolved-or-skipped snapshot. The source attempt number must be nonzero because
    /// a resolved reference always binds to one terminal source attempt.
    pub fn new(
        source_node: GraphNodeId,
        source_attempt_id: AttemptId,
        source_attempt_number: AttemptNumberV2,
        items_digest: Sha256Digest,
        resolved_at: UnixMillis,
    ) -> Result<Self, DomainError> {
        if source_attempt_number < AttemptNumberV2::FIRST {
            return Err(invalid(
                "resolved reference source attempt number must be nonzero",
            ));
        }
        Ok(Self {
            source_node,
            source_attempt_id,
            source_attempt_number,
            items_digest,
            resolved_at,
        })
    }

    pub fn source_node(&self) -> &GraphNodeId {
        &self.source_node
    }

    pub fn source_attempt_id(&self) -> &AttemptId {
        &self.source_attempt_id
    }

    pub const fn source_attempt_number(&self) -> AttemptNumberV2 {
        self.source_attempt_number
    }

    pub fn items_digest(&self) -> &Sha256Digest {
        &self.items_digest
    }

    pub const fn resolved_at(&self) -> UnixMillis {
        self.resolved_at
    }
}

/// One declared evidence reference exactly as resolved at activation (dossier section 8.2). The
/// stored states are `Resolved`, `Skipped`, and `Unresolved`; the `stale` read-back marker of
/// section 8.4 is derived from attempt validity and is never stored on this immutable snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedEvidenceReferenceV2 {
    Resolved(EvidenceReferenceSnapshotV2),
    Skipped(EvidenceReferenceSnapshotV2),
    Unresolved { source_node: GraphNodeId },
}

impl ResolvedEvidenceReferenceV2 {
    pub fn resolved(snapshot: EvidenceReferenceSnapshotV2) -> Self {
        Self::Resolved(snapshot)
    }

    pub fn skipped(snapshot: EvidenceReferenceSnapshotV2) -> Self {
        Self::Skipped(snapshot)
    }

    pub const fn unresolved(source_node: GraphNodeId) -> Self {
        Self::Unresolved { source_node }
    }

    pub fn source_node(&self) -> &GraphNodeId {
        match self {
            Self::Resolved(snapshot) | Self::Skipped(snapshot) => snapshot.source_node(),
            Self::Unresolved { source_node } => source_node,
        }
    }

    /// Returns the resolved-or-skipped snapshot data. An unresolved reference carries no source
    /// attempt, digest, or timestamp.
    pub fn snapshot(&self) -> Option<&EvidenceReferenceSnapshotV2> {
        match self {
            Self::Resolved(snapshot) | Self::Skipped(snapshot) => Some(snapshot),
            Self::Unresolved { .. } => None,
        }
    }

    pub const fn is_unresolved(&self) -> bool {
        matches!(self, Self::Unresolved { .. })
    }
}

/// The bounded ordered set of resolved evidence references for one attempt, in declared
/// `evidence_from` order. A placement that declares no `evidence_from` carries an empty set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedEvidenceSetV2 {
    references: Vec<ResolvedEvidenceReferenceV2>,
}

impl ResolvedEvidenceSetV2 {
    pub fn new(references: Vec<ResolvedEvidenceReferenceV2>) -> Result<Self, DomainError> {
        if references.len() > MAX_RESOLVED_REFERENCES {
            return Err(invalid(
                "an attempt holds at most eight resolved references",
            ));
        }
        Ok(Self { references })
    }

    pub fn references(&self) -> &[ResolvedEvidenceReferenceV2] {
        &self.references
    }
}

/// Caller-supplied parts for assembling a decision record. Named fields remove the silent-transpose
/// hazard that two `GraphNodeId` fields (`graph_node_id` and `route_target`) would otherwise create.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionRecordInputV2 {
    pub trace: TraceSequenceV2,
    pub session_id: SessionId,
    pub session_revision: Revision,
    pub procedure_snapshot_id: ProcedureSnapshotId,
    pub procedure_digest: Sha256Digest,
    pub graph_node_id: GraphNodeId,
    pub node_definition_id: NodeDefinitionId,
    pub attempt_id: AttemptId,
    pub attempt_number: AttemptNumberV2,
    pub goal_revision: Option<GoalRevisionNumberV2>,
    pub selected_option: OptionId,
    pub route_effect: TransitionEffectV2,
    pub route_target: GraphNodeId,
    pub reason: ReasonV2,
    pub evidence: ResolvedEvidenceSetV2,
    pub actor: Option<ActorAttributionV2>,
    pub recorded_at: UnixMillis,
}

/// An immutable decision record (dossier section 6.4). It remains fully reportable for the session
/// lifetime even after its attempt or a referenced source attempt becomes stale; it never proves
/// that the selected option was semantically correct.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionRecordV2 {
    trace: TraceSequenceV2,
    session_id: SessionId,
    session_revision: Revision,
    procedure_snapshot_id: ProcedureSnapshotId,
    procedure_digest: Sha256Digest,
    graph_node_id: GraphNodeId,
    node_definition_id: NodeDefinitionId,
    attempt_id: AttemptId,
    attempt_number: AttemptNumberV2,
    goal_revision: Option<GoalRevisionNumberV2>,
    selected_option: OptionId,
    route_effect: TransitionEffectV2,
    route_target: GraphNodeId,
    reason: ReasonV2,
    evidence: ResolvedEvidenceSetV2,
    actor: Option<ActorAttributionV2>,
    recorded_at: UnixMillis,
}

impl DecisionRecordV2 {
    pub fn new(input: DecisionRecordInputV2) -> Result<Self, DomainError> {
        if input.trace < TraceSequenceV2::FIRST {
            return Err(invalid("decision record trace sequence must be nonzero"));
        }
        if input.session_revision == Revision::ZERO {
            return Err(invalid("decision record session revision must be nonzero"));
        }
        if input.attempt_number < AttemptNumberV2::FIRST {
            return Err(invalid("decision record attempt number must be nonzero"));
        }
        if matches!(input.goal_revision, Some(goal) if goal < GoalRevisionNumberV2::FIRST) {
            return Err(invalid("decision record goal revision must be nonzero"));
        }
        Ok(Self {
            trace: input.trace,
            session_id: input.session_id,
            session_revision: input.session_revision,
            procedure_snapshot_id: input.procedure_snapshot_id,
            procedure_digest: input.procedure_digest,
            graph_node_id: input.graph_node_id,
            node_definition_id: input.node_definition_id,
            attempt_id: input.attempt_id,
            attempt_number: input.attempt_number,
            goal_revision: input.goal_revision,
            selected_option: input.selected_option,
            route_effect: input.route_effect,
            route_target: input.route_target,
            reason: input.reason,
            evidence: input.evidence,
            actor: input.actor,
            recorded_at: input.recorded_at,
        })
    }

    pub const fn trace(&self) -> TraceSequenceV2 {
        self.trace
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub const fn session_revision(&self) -> Revision {
        self.session_revision
    }

    pub fn procedure_snapshot_id(&self) -> &ProcedureSnapshotId {
        &self.procedure_snapshot_id
    }

    pub fn procedure_digest(&self) -> &Sha256Digest {
        &self.procedure_digest
    }

    pub fn graph_node_id(&self) -> &GraphNodeId {
        &self.graph_node_id
    }

    pub fn node_definition_id(&self) -> &NodeDefinitionId {
        &self.node_definition_id
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub const fn attempt_number(&self) -> AttemptNumberV2 {
        self.attempt_number
    }

    pub fn goal_revision(&self) -> Option<GoalRevisionNumberV2> {
        self.goal_revision
    }

    pub fn selected_option(&self) -> &OptionId {
        &self.selected_option
    }

    pub const fn route_effect(&self) -> TransitionEffectV2 {
        self.route_effect
    }

    pub fn route_target(&self) -> &GraphNodeId {
        &self.route_target
    }

    pub fn reason(&self) -> &ReasonV2 {
        &self.reason
    }

    pub fn evidence(&self) -> &ResolvedEvidenceSetV2 {
        &self.evidence
    }

    pub fn actor(&self) -> Option<&ActorAttributionV2> {
        self.actor.as_ref()
    }

    pub const fn recorded_at(&self) -> UnixMillis {
        self.recorded_at
    }
}

/// Whether a rework transition was declared by a graph-selected route or chosen manually at runtime
/// (dossier sections 9.3 and 9.5).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReworkKindV2 {
    Declared,
    Manual,
}

impl ReworkKindV2 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Manual => "manual",
        }
    }
}

impl std::str::FromStr for ReworkKindV2 {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "declared" => Ok(Self::Declared),
            "manual" => Ok(Self::Manual),
            _ => Err(invalid("unknown rework kind")),
        }
    }
}

/// Caller-supplied parts for assembling a rework record. Named fields remove the silent-transpose
/// hazard between the two `GraphNodeId` fields (`from_node` and `to_node`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReworkRecordInputV2 {
    pub trace: TraceSequenceV2,
    pub kind: ReworkKindV2,
    pub from_node: GraphNodeId,
    pub to_node: GraphNodeId,
    pub target_attempt_id: AttemptId,
    pub reason: ReasonV2,
    pub reactivated: bool,
    pub actor: Option<ActorAttributionV2>,
    pub recorded_at: UnixMillis,
}

/// An immutable rework record (dossier sections 9.3 and 9.5). Declared and manual rework both
/// create a fresh target attempt through conservative trace-suffix invalidation; the record captures
/// the source and target nodes, the fresh target attempt, the kind, the reason, and whether the
/// transition reactivated a completed session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReworkRecordV2 {
    trace: TraceSequenceV2,
    kind: ReworkKindV2,
    from_node: GraphNodeId,
    to_node: GraphNodeId,
    target_attempt_id: AttemptId,
    reason: ReasonV2,
    reactivated: bool,
    actor: Option<ActorAttributionV2>,
    recorded_at: UnixMillis,
}

impl ReworkRecordV2 {
    pub fn new(input: ReworkRecordInputV2) -> Result<Self, DomainError> {
        if input.trace < TraceSequenceV2::FIRST {
            return Err(invalid("rework record trace sequence must be nonzero"));
        }
        Ok(Self {
            trace: input.trace,
            kind: input.kind,
            from_node: input.from_node,
            to_node: input.to_node,
            target_attempt_id: input.target_attempt_id,
            reason: input.reason,
            reactivated: input.reactivated,
            actor: input.actor,
            recorded_at: input.recorded_at,
        })
    }

    pub const fn trace(&self) -> TraceSequenceV2 {
        self.trace
    }

    pub const fn kind(&self) -> ReworkKindV2 {
        self.kind
    }

    pub fn from_node(&self) -> &GraphNodeId {
        &self.from_node
    }

    pub fn to_node(&self) -> &GraphNodeId {
        &self.to_node
    }

    pub fn target_attempt_id(&self) -> &AttemptId {
        &self.target_attempt_id
    }

    pub fn reason(&self) -> &ReasonV2 {
        &self.reason
    }

    pub const fn reactivated(&self) -> bool {
        self.reactivated
    }

    pub fn actor(&self) -> Option<&ActorAttributionV2> {
        self.actor.as_ref()
    }

    pub const fn recorded_at(&self) -> UnixMillis {
        self.recorded_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::ArtifactValueV1;
    use crate::procedure::ItemTypeV1;
    use crate::procedure_v2::TransitionEffectV2;
    use crate::session_v2::{AttemptNumberV2, GoalRevisionNumberV2, TraceSequenceV2};
    use crate::{
        AttemptId, GraphNodeId, ItemId, NodeDefinitionId, OptionId, ProcedureSnapshotId, Revision,
        SessionId, Sha256Digest, UnixMillis,
    };

    fn digest(seed: &str) -> Sha256Digest {
        let mut hex = String::new();
        while hex.len() < 64 {
            hex.push_str(seed);
        }
        Sha256Digest::new(format!("sha256:{}", &hex[..64])).unwrap()
    }

    fn local_artifact() -> ArtifactValueV1 {
        ArtifactValueV1::local_path("reports/run.log", digest("a1b2c3d4"), 42, "text/plain")
            .unwrap()
    }

    fn node(value: &str) -> GraphNodeId {
        GraphNodeId::new(value).unwrap()
    }

    fn attempt_id(n: u64) -> AttemptId {
        AttemptId::new(format!("00000000-0000-0000-0000-{n:012x}")).unwrap()
    }

    fn snapshot(source: &str) -> EvidenceReferenceSnapshotV2 {
        EvidenceReferenceSnapshotV2::new(
            node(source),
            attempt_id(1),
            AttemptNumberV2::FIRST,
            digest("a1b2c3d4e5f60718293a4b5c6d7e8f90"),
            UnixMillis::new(5),
        )
        .unwrap()
    }

    fn resolved_set() -> ResolvedEvidenceSetV2 {
        ResolvedEvidenceSetV2::new(vec![
            ResolvedEvidenceReferenceV2::resolved(snapshot("source-a")),
            ResolvedEvidenceReferenceV2::unresolved(node("source-b")),
        ])
        .unwrap()
    }

    fn decision_input() -> DecisionRecordInputV2 {
        DecisionRecordInputV2 {
            trace: TraceSequenceV2::FIRST,
            session_id: SessionId::new("00000000-0000-0000-0000-000000000001").unwrap(),
            session_revision: Revision::new(1),
            procedure_snapshot_id: ProcedureSnapshotId::new("00000000-0000-0000-0000-000000000002")
                .unwrap(),
            procedure_digest: digest("a1b2c3d4e5f60718293a4b5c6d7e8f90"),
            graph_node_id: node("decide"),
            node_definition_id: NodeDefinitionId::new("dec").unwrap(),
            attempt_id: attempt_id(3),
            attempt_number: AttemptNumberV2::FIRST,
            goal_revision: Some(GoalRevisionNumberV2::FIRST),
            selected_option: OptionId::new("passed").unwrap(),
            route_effect: TransitionEffectV2::Advance,
            route_target: node("review"),
            reason: ReasonV2::new("Tests pass.").unwrap(),
            evidence: resolved_set(),
            actor: Some(ActorAttributionV2::new("reviewer").unwrap()),
            recorded_at: UnixMillis::new(9),
        }
    }

    #[test]
    fn reason_and_actor_enforce_non_blank_character_bounds() {
        assert_eq!(ReasonV2::new(""), Err(invalid("record reason")));
        assert_eq!(ReasonV2::new("   "), Err(invalid("record reason")));
        let at_limit = ReasonV2::new("r".repeat(2_000)).unwrap();
        assert_eq!(at_limit.as_str().chars().count(), 2_000);
        assert_eq!(
            ReasonV2::new("r".repeat(2_001)),
            Err(invalid("record reason"))
        );
        assert_eq!(
            ActorAttributionV2::new("").unwrap_err(),
            invalid("actor attribution")
        );
        assert_eq!(
            ActorAttributionV2::new("a".repeat(257)).unwrap_err(),
            invalid("actor attribution")
        );
        assert_eq!(
            ActorAttributionV2::new("a".repeat(256))
                .unwrap()
                .as_str()
                .chars()
                .count(),
            256
        );
    }

    #[test]
    fn reason_and_actor_count_unicode_scalars_not_bytes() {
        // Each emoji is one scalar but four UTF-8 bytes; 500 emojis hit the 2000-character limit.
        let reason = ReasonV2::new("😀".repeat(2_000)).unwrap();
        assert_eq!(reason.as_str().chars().count(), 2_000);
        assert_eq!(reason.as_str().len(), 8_000);
        assert_eq!(
            ReasonV2::new("😀".repeat(2_001)),
            Err(invalid("record reason"))
        );
    }

    #[test]
    fn recorded_item_value_round_trips_each_typed_variant() {
        let confirm = RecordedItemValueV2::confirm();
        assert_eq!(confirm.item_type(), ItemTypeV1::Confirm);

        let text = RecordedItemValueV2::text("summary").unwrap();
        assert_eq!(text.as_text(), Some("summary"));
        assert_eq!(text.item_type(), ItemTypeV1::Text);

        let choice = RecordedItemValueV2::choice("green").unwrap();
        assert_eq!(choice.as_choice(), Some("green"));
        assert_eq!(choice.item_type(), ItemTypeV1::Choice);

        let integer = RecordedItemValueV2::integer(-7);
        assert_eq!(integer.as_integer(), Some(-7));
        assert_eq!(integer.item_type(), ItemTypeV1::Integer);

        let list = RecordedItemValueV2::list(vec!["a".to_owned(), "b".to_owned()]).unwrap();
        assert_eq!(
            list.as_list(),
            Some(["a".to_owned(), "b".to_owned()].as_slice())
        );
        assert_eq!(list.item_type(), ItemTypeV1::List);

        let artifact_value = RecordedItemValueV2::artifact(local_artifact());
        assert_eq!(artifact_value.item_type(), ItemTypeV1::Artifact);
        assert!(artifact_value.as_artifact().is_some());
    }

    #[test]
    fn recorded_text_accepts_empty_and_at_limit_but_not_over() {
        assert!(RecordedItemValueV2::text("").is_ok());
        assert!(RecordedItemValueV2::text("t".repeat(16_384)).is_ok());
        assert_eq!(
            RecordedItemValueV2::text("t".repeat(16_385)).unwrap_err(),
            invalid("recorded text value")
        );
    }

    #[test]
    fn recorded_choice_and_list_enforce_non_empty_and_count_bounds() {
        assert_eq!(
            RecordedItemValueV2::choice("").unwrap_err(),
            invalid("recorded choice value")
        );
        assert!(RecordedItemValueV2::choice("c".repeat(120)).is_ok());
        assert_eq!(
            RecordedItemValueV2::choice("c".repeat(121)).unwrap_err(),
            invalid("recorded choice value")
        );
        assert!(RecordedItemValueV2::list(Vec::new()).is_ok());
        assert!(RecordedItemValueV2::list(vec!["x".to_owned(); 200]).is_ok());
        assert_eq!(
            RecordedItemValueV2::list(vec!["x".to_owned(); 201]).unwrap_err(),
            invalid("recorded list value exceeds the v2 entry maximum")
        );
        assert_eq!(
            RecordedItemValueV2::list(vec!["".to_owned()]).unwrap_err(),
            invalid("recorded list entry")
        );
        assert!(RecordedItemValueV2::list(vec!["x".repeat(1_000)]).is_ok());
        assert_eq!(
            RecordedItemValueV2::list(vec!["x".repeat(1_001)]).unwrap_err(),
            invalid("recorded list entry")
        );
    }

    #[test]
    fn recorded_item_set_preserves_order_and_rejects_duplicates_and_overflow() {
        let make = |id: &str| {
            RecordedItemV2::new(ItemId::new(id).unwrap(), RecordedItemValueV2::confirm())
        };
        let set = RecordedItemSetV2::new(vec![make("first"), make("second")]).unwrap();
        assert_eq!(
            set.items()
                .iter()
                .map(RecordedItemV2::id)
                .collect::<Vec<_>>(),
            vec![
                &ItemId::new("first").unwrap(),
                &ItemId::new("second").unwrap()
            ]
        );
        assert_eq!(
            RecordedItemSetV2::new(vec![make("dup"), make("dup")]).unwrap_err(),
            invalid("recorded item identifiers must be unique")
        );
        let too_many: Vec<RecordedItemV2> = (0..65).map(|i| make(&format!("i-{i}"))).collect();
        assert_eq!(
            RecordedItemSetV2::new(too_many).unwrap_err(),
            invalid("an attempt holds at most 64 recorded items")
        );
    }

    #[test]
    fn resolved_reference_states_carry_the_exact_snapshot_identity() {
        let snap = snapshot("source-a");
        let resolved = ResolvedEvidenceReferenceV2::resolved(snap.clone());
        assert_eq!(resolved.source_node(), &node("source-a"));
        assert_eq!(resolved.snapshot(), Some(&snap));
        assert!(!resolved.is_unresolved());

        let skipped = ResolvedEvidenceReferenceV2::skipped(snap.clone());
        assert_eq!(skipped.snapshot(), Some(&snap));

        let unresolved = ResolvedEvidenceReferenceV2::unresolved(node("source-b"));
        assert_eq!(unresolved.source_node(), &node("source-b"));
        assert!(unresolved.snapshot().is_none());
        assert!(unresolved.is_unresolved());
    }

    #[test]
    fn resolved_snapshot_rejects_zero_attempt_number() {
        assert_eq!(
            EvidenceReferenceSnapshotV2::new(
                node("source-a"),
                attempt_id(1),
                AttemptNumberV2::ZERO,
                digest("a1b2c3d4e5f60718293a4b5c6d7e8f90"),
                UnixMillis::new(5),
            )
            .unwrap_err(),
            invalid("resolved reference source attempt number must be nonzero")
        );
    }

    #[test]
    fn resolved_evidence_set_bounds_the_ordered_collection() {
        assert!(ResolvedEvidenceSetV2::new(Vec::new()).is_ok());
        let eight: Vec<ResolvedEvidenceReferenceV2> = (0..8)
            .map(|i| ResolvedEvidenceReferenceV2::unresolved(node(&format!("n-{i}"))))
            .collect();
        assert!(ResolvedEvidenceSetV2::new(eight).is_ok());
        let nine: Vec<ResolvedEvidenceReferenceV2> = (0..9)
            .map(|i| ResolvedEvidenceReferenceV2::unresolved(node(&format!("n-{i}"))))
            .collect();
        assert_eq!(
            ResolvedEvidenceSetV2::new(nine).unwrap_err(),
            invalid("an attempt holds at most eight resolved references")
        );
    }

    #[test]
    fn decision_record_round_trips_every_field_and_keeps_route_target_distinct() {
        let record = DecisionRecordV2::new(decision_input()).unwrap();
        assert_eq!(record.trace(), TraceSequenceV2::FIRST);
        assert_eq!(record.attempt_number(), AttemptNumberV2::FIRST);
        assert_eq!(record.goal_revision(), Some(GoalRevisionNumberV2::FIRST));
        assert_eq!(record.graph_node_id(), &node("decide"));
        assert_eq!(record.route_target(), &node("review"));
        assert_ne!(record.graph_node_id(), record.route_target());
        assert_eq!(record.route_effect(), TransitionEffectV2::Advance);
        assert_eq!(record.selected_option().as_str(), "passed");
        assert_eq!(record.reason().as_str(), "Tests pass.");
        assert_eq!(record.evidence().references().len(), 2);
        assert_eq!(record.actor().unwrap().as_str(), "reviewer");
        assert_eq!(record.recorded_at(), UnixMillis::new(9));
    }

    #[test]
    fn decision_record_rejects_zero_identity_fields_and_accepts_null_goal_revision() {
        let mut input = decision_input();
        input.trace = TraceSequenceV2::ZERO;
        assert_eq!(
            DecisionRecordV2::new(input).unwrap_err(),
            invalid("decision record trace sequence must be nonzero")
        );

        let mut input = decision_input();
        input.session_revision = Revision::ZERO;
        assert_eq!(
            DecisionRecordV2::new(input).unwrap_err(),
            invalid("decision record session revision must be nonzero")
        );

        let mut input = decision_input();
        input.attempt_number = AttemptNumberV2::ZERO;
        assert_eq!(
            DecisionRecordV2::new(input).unwrap_err(),
            invalid("decision record attempt number must be nonzero")
        );

        let mut input = decision_input();
        input.goal_revision = Some(GoalRevisionNumberV2::ZERO);
        assert_eq!(
            DecisionRecordV2::new(input).unwrap_err(),
            invalid("decision record goal revision must be nonzero")
        );

        let mut input = decision_input();
        input.goal_revision = None;
        assert!(DecisionRecordV2::new(input).is_ok());
    }

    #[test]
    fn rework_kind_round_trips_strings() {
        assert_eq!(ReworkKindV2::Declared.as_str(), "declared");
        assert_eq!(ReworkKindV2::Manual.as_str(), "manual");
        assert_eq!(
            "manual".parse::<ReworkKindV2>().unwrap(),
            ReworkKindV2::Manual
        );
        assert_eq!(
            "forced".parse::<ReworkKindV2>().unwrap_err(),
            invalid("unknown rework kind")
        );
    }

    #[test]
    fn rework_record_round_trips_and_distinguishes_from_to_nodes() {
        let record = ReworkRecordV2::new(ReworkRecordInputV2 {
            trace: TraceSequenceV2::new(7),
            kind: ReworkKindV2::Declared,
            from_node: node("decide"),
            to_node: node("implement"),
            target_attempt_id: attempt_id(4),
            reason: ReasonV2::new("Verification failed; return to implementation.").unwrap(),
            reactivated: false,
            actor: None,
            recorded_at: UnixMillis::new(12),
        })
        .unwrap();
        assert_eq!(record.kind(), ReworkKindV2::Declared);
        assert_eq!(record.from_node(), &node("decide"));
        assert_eq!(record.to_node(), &node("implement"));
        assert_ne!(record.from_node(), record.to_node());
        assert_eq!(record.target_attempt_id(), &attempt_id(4));
        assert!(!record.reactivated());
        assert!(record.actor().is_none());

        let manual = ReworkRecordV2::new(ReworkRecordInputV2 {
            trace: TraceSequenceV2::ZERO,
            kind: ReworkKindV2::Manual,
            from_node: node("decide"),
            to_node: node("implement"),
            target_attempt_id: attempt_id(4),
            reason: ReasonV2::new("x").unwrap(),
            reactivated: true,
            actor: Some(ActorAttributionV2::new("lead").unwrap()),
            recorded_at: UnixMillis::new(12),
        })
        .unwrap_err();
        assert_eq!(
            manual,
            invalid("rework record trace sequence must be nonzero")
        );
    }
}
