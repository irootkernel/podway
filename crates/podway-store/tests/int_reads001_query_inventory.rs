//! READS-001 frozen recipes with READS-002 acquisition/retention assertions.
//!
//! The qualification guide retains the original per-owner query counts.
//! Candidate cold loads assert the predeclared family bounds, identical
//! delivered rows and payload retention, and exact variable-length bytes
//! with the fixture-root input accounted separately. Prepared reloads keep
//! independent header, changed-payload and pointer-reuse measurements.

use podway_core::{
    ActorAttributionV2, AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2, BlockerId,
    BlockerState, CanonicalProcedureJsonV1, CriterionAssessmentReasonV2,
    CriterionAssessmentResultV2, CriterionCitationV2, CriterionId, CriterionStatusV2,
    GoalCriterionV2, GoalDefinitionV2, GoalRevisionNumberV2, GoalRevisionReasonV2,
    GoalRevisionRecordV2, GoalStateV2, GoalStatementV2, GraphNodeId, ItemId, ItemTypeV1,
    ProcedureSnapshotId, ProcedureSourceLabelV1, RecordedItemValueV2, Revision, SessionAttemptV2,
    SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2, UnixMillis,
    WorkspaceId, canonicalize_json_v1, recorded_value_encoded_size_v2,
};
use podway_store::test_support::query_account::{
    QueryAccountant, clear_payload_registry, declare_cached_input_bytes, payload_bytes_of,
    payload_pointer,
};
use podway_store::{
    AttemptCriterionAssessmentStateV2, AttemptMetadataV2, AttemptWorkflowMemoryV2, BlockerStateV2,
    CriterionAssessmentStateV2, DurableWorktreeIdentityV1, EvidenceResolutionStateV2,
    GraphNodeCounterV2, GraphSessionStateV2, ItemSlotStateV2, ProcedureSnapshotV2,
    WorkflowMemoryStateV2,
};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::int_v2dfc005_direct_store::{deadline, digest, request, store, success};
use super::int_v2dfc008_persistence::{archive, fixture, seed};

/// Frozen fixture recipe dimensions. S/M/grown-L/1024 are newly chosen for
/// READS-001 Store-level registration; CLI qualification keeps its own
/// 300-node G7/G8 recipes.
pub(super) struct Recipe {
    pub(super) attempts: usize,
    pub(super) items: usize,
    pub(super) value_chars: usize,
    pub(super) filled_values: bool,
    pub(super) blocker_attempts: usize,
    pub(super) goal_revisions: usize,
    pub(super) criteria: usize,
    pub(super) citations: usize,
}

pub(super) fn recipe_small() -> Recipe {
    Recipe {
        attempts: 5,
        items: 2,
        value_chars: 40,
        filled_values: true,
        blocker_attempts: 1,
        goal_revisions: 2,
        criteria: 2,
        citations: 2,
    }
}

pub(super) fn recipe_medium() -> Recipe {
    Recipe {
        attempts: 24,
        items: 8,
        value_chars: 1_000,
        filled_values: true,
        blocker_attempts: 6,
        goal_revisions: 2,
        criteria: 4,
        citations: 3,
    }
}

fn recipe_grown_large() -> Recipe {
    Recipe {
        attempts: 40,
        items: 16,
        value_chars: 55_000,
        filled_values: true,
        blocker_attempts: 4,
        goal_revisions: 2,
        criteria: 3,
        citations: 2,
    }
}

fn recipe_maximum_nodes() -> Recipe {
    Recipe {
        attempts: 1_024,
        items: 1,
        value_chars: 24,
        filled_values: true,
        blocker_attempts: 8,
        goal_revisions: 2,
        criteria: 2,
        citations: 2,
    }
}

fn recipe_empty() -> Recipe {
    Recipe {
        attempts: 1,
        items: 1,
        value_chars: 0,
        filled_values: false,
        blocker_attempts: 0,
        goal_revisions: 1,
        criteria: 1,
        citations: 0,
    }
}

fn recipe_single() -> Recipe {
    Recipe {
        attempts: 1,
        items: 2,
        value_chars: 10,
        filled_values: true,
        blocker_attempts: 0,
        goal_revisions: 1,
        criteria: 1,
        citations: 1,
    }
}

/// Attempt identities embed their fixture session number so several seeded
/// sessions can coexist in one database without colliding rows.
pub(super) fn attempt_uuid(session_number: u64, index: usize) -> AttemptId {
    let value = (session_number << 24) | (index as u64 & 0xff_ffff);
    AttemptId::new(format!("00000000-0000-4000-8000-{value:012x}")).unwrap()
}

fn session_uuid(number: u64) -> SessionId {
    SessionId::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap()
}

fn node_id(index: usize) -> GraphNodeId {
    GraphNodeId::new(format!("n{index}")).unwrap()
}

fn item_id(index: usize) -> ItemId {
    ItemId::new(format!("item-{index}")).unwrap()
}

fn criterion_id(index: usize) -> CriterionId {
    CriterionId::new(format!("crit-{index}")).unwrap()
}

fn text_payload(prefix: &str, chars: usize) -> String {
    let base = prefix.repeat(chars / prefix.len().max(1) + 1);
    base.chars().take(chars).collect()
}

pub(super) fn reads001_snapshot(recipe: &Recipe, snapshot_number: u64) -> ProcedureSnapshotV2 {
    let mut items = Vec::new();
    for index in 0..recipe.items {
        items.push(json!({
            "id": format!("item-{index}"),
            "type": "text",
            "prompt": format!("Record item {index}."),
            "required": false,
            "min_length": 1,
            "max_length": 65_536,
            "multiline": false
        }));
    }
    let assess = |index: usize, next: usize| {
        let mut node = json!({
            "id": format!("n{index}"),
            "use": "assess-def",
            "routes": {
                "continue": {"to": format!("n{next}"), "effect": "advance"},
                "hold": {"to": format!("n{next}"), "effect": "advance"},
                "supersede": {"to": format!("n{next}"), "effect": "advance"}
            }
        });
        // Only the second assessment node consumes evidence, from the work
        // attempt before it. The first assessment node stays evidence-free so
        // the goal rework repeat attempt targets a node without an evidence
        // placement.
        if index == 3 {
            node["evidence_from"] = json!([
                {"node": "n2", "required": false, "items": ["item-0"]}
            ]);
        }
        node
    };
    let mut nodes = Vec::new();
    let mut assess_traces = Vec::new();
    for index in 0..recipe.attempts {
        let is_last = index + 1 == recipe.attempts;
        let is_assess = index % 2 == 1 && !is_last;
        if is_assess {
            nodes.push(assess(index, index + 1));
            // Attempt trace numbers are 1-based; this node hosts attempt
            // trace index+1, a candidate later-revision binding.
            assess_traces.push((index + 1) as u64);
        } else {
            let mut node = json!({"id": format!("n{index}"), "use": "work-def"});
            if is_last {
                node["terminal"] = json!(true);
            } else {
                node["next"] = json!(format!("n{}", index + 1));
            }
            nodes.push(node);
        }
    }
    let document = json!({
        "schema": "podway.procedure/v2",
        "id": "reads001-inventory",
        "version": "1",
        "name": "READS-001 inventory",
        "purpose": "Registered query inventory fixture with repeated families.",
        "goal_tracking": true,
        "node_definitions": {
            "work-def": {
                "type": "action",
                "title": "Work",
                "intent": "Record bounded values for read characterization.",
                "items": items
            },
            "assess-def": {
                "type": "decision",
                "title": "Assess",
                "objective": "Assess the registered session goal.",
                "prompt": "Continue or hold?",
                "items": items,
                "options": [
                    {"id": "continue", "label": "Continue"},
                    {"id": "hold", "label": "Hold"},
                    {"id": "supersede", "label": "Supersede"}
                ],
                "assessment": {
                    "target": "session_goal",
                    "outcomes": {
                        "continue": "achieved",
                        "hold": "not_achieved",
                        "supersede": "superseded"
                    }
                },
                "reason": {"required": true}
            }
        },
        "graph": {"entry": "n0", "nodes": nodes},
        "manual_rework": {
            // Only the rework target the recipe actually uses: the first
            // assessment node for two-revision goal histories, or the entry
            // work node when no rework is exercised.
            "allowed_targets": [if reads001_binding_traces(recipe).len() > 1 {
                "n1"
            } else {
                "n0"
            }]
        }
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let procedure_digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    podway_store::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new(format!("00000000-0000-4000-8000-{snapshot_number:012x}"))
            .unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        procedure_digest,
        ProcedureSourceLabelV1::file("reads001.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap()
}

/// Binding traces: the initial revision binds trace 1; each later revision
/// binds the next assessment node's attempt trace, whose node is a manual
/// rework target and revision-safe.
fn reads001_binding_traces(recipe: &Recipe) -> Vec<u64> {
    // A second revision reworks from the terminal cursor back to the first
    // assessment node: its binding attempt is a repeat attempt appended after
    // the base chain, and the revision is a terminal reactivation.
    match recipe.goal_revisions {
        0 | 1 => vec![1],
        2 => vec![1, recipe.attempts as u64 + 1],
        other => panic!("registered recipes use at most two goal revisions: {other}"),
    }
}

/// Total attempts including the goal-rework repeat attempt.
pub(super) fn attempt_total(recipe: &Recipe) -> u64 {
    recipe.attempts as u64 + u64::from(recipe.goal_revisions >= 2)
}

/// Builds a domain-valid inventory session. `changed_active_items` bumps the
/// active attempt's first N slot revisions and payloads, producing the
/// header/point selective-miss shape a compatible writer produces.
pub(super) fn reads001_session(
    recipe: &Recipe,
    snapshot: &ProcedureSnapshotV2,
    session_number: u64,
    changed_active_items: usize,
    bump_revisions: bool,
) -> GraphSessionStateV2 {
    let session_id = session_uuid(session_number);
    let changed = changed_active_items.min(recipe.items);
    let binding_traces = reads001_binding_traces(recipe);
    assert_eq!(
        binding_traces.len(),
        recipe.goal_revisions,
        "recipe must provide one binding trace per goal revision"
    );
    // revision index -> (binding trace, reason text)
    let revision_meta: Vec<(u64, Option<&'static str>)> = binding_traces
        .iter()
        .enumerate()
        .map(|(index, trace)| {
            if index == 0 {
                (*trace, None)
            } else {
                (*trace, Some("Explicit registered revision."))
            }
        })
        .collect();
    let revision_of_attempt = |trace: u64| -> u64 {
        let mut current = 1;
        for (index, (binding_trace, _)) in revision_meta.iter().enumerate() {
            if *binding_trace <= trace {
                current = index as u64 + 1;
            }
        }
        current
    };
    let mut attempts = Vec::new();
    let mut metadata = Vec::new();
    let mut memories = Vec::new();
    let mut counters = Vec::new();
    let repeat_revision = recipe.goal_revisions >= 2;
    for index in 0..recipe.attempts {
        // With a second goal revision the whole base chain completes and the
        // rework repeat attempt carries the active cursor; otherwise the
        // final base attempt stays active.
        let active = !repeat_revision && index + 1 == recipe.attempts;
        let lifecycle = if active {
            AttemptLifecycle::Active
        } else {
            AttemptLifecycle::Completed
        };
        // Attempt i runs under the newest revision whose binding trace is
        // not after its own trace.
        let goal_binding = if recipe.goal_revisions > 0 {
            Some(GoalRevisionNumberV2::new(revision_of_attempt(
                (index + 1) as u64,
            )))
        } else {
            None
        };
        attempts.push(
            SessionAttemptV2::new(
                attempt_uuid(session_number, index),
                node_id(index),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::new((index + 1) as u64),
                lifecycle,
                if repeat_revision && index > 0 {
                    // The goal rework invalidates everything from the target
                    // node's first attempt onward; the initial work attempt
                    // stays valid.
                    AttemptValidityV2::Stale
                } else {
                    AttemptValidityV2::Valid
                },
                goal_binding,
            )
            .unwrap(),
        );
        metadata.push(
            AttemptMetadataV2::new(
                attempt_uuid(session_number, index),
                UnixMillis::new(100 + index as u64),
                (!active).then_some(UnixMillis::new(100 + index as u64 + 1)),
                (!active).then(|| "advance".to_owned()),
            )
            .unwrap(),
        );
        counters.push(GraphNodeCounterV2::new(
            node_id(index),
            1 + u64::from(repeat_revision && index == 1),
            u64::from(repeat_revision && index == 1),
        ));
        let mut slots = Vec::new();
        for item in 0..recipe.items {
            let changed_slot = active && item < changed;
            let value = if recipe.filled_values && (recipe.value_chars > 0) {
                let prefix = if changed_slot {
                    format!("c{index}i{item}y")
                } else {
                    format!("a{index}i{item}x")
                };
                Some(RecordedItemValueV2::text(text_payload(&prefix, recipe.value_chars)).unwrap())
            } else {
                None
            };
            let created = UnixMillis::new(100 + index as u64);
            let updated = if value.is_some() {
                UnixMillis::new(101 + index as u64 + u64::from(changed_slot))
            } else {
                created
            };
            slots.push(
                ItemSlotStateV2::new(
                    attempt_uuid(session_number, index),
                    item_id(item),
                    ItemTypeV1::Text,
                    if value.is_some() {
                        Revision::new(if changed_slot { 2 } else { 1 })
                    } else {
                        Revision::ZERO
                    },
                    value,
                    created,
                    updated,
                )
                .unwrap(),
            );
        }
        let blockers = if index < recipe.blocker_attempts {
            vec![
                BlockerStateV2::new(
                    BlockerId::new(format!(
                        "00000000-0000-4000-8000-{:012x}",
                        (session_number << 24) | (index as u64 & 0xff_ffff)
                    ))
                    .unwrap(),
                    attempt_uuid(session_number, index),
                    "Registered inventory blocker.",
                    BlockerState::Resolved,
                    UnixMillis::new(100 + index as u64),
                    Some(UnixMillis::new(101 + index as u64)),
                )
                .unwrap(),
            ]
        } else {
            Vec::new()
        };
        let evidence = if index == 3 {
            let source: &AttemptWorkflowMemoryV2 = &memories[2];
            vec![
                EvidenceResolutionStateV2::new(
                    0,
                    false,
                    vec![item_id(0)],
                    podway_core::ResolvedEvidenceReferenceV2::resolved(
                        podway_core::EvidenceReferenceSnapshotV2::new(
                            node_id(index - 1),
                            attempt_uuid(session_number, index - 1),
                            AttemptNumberV2::FIRST,
                            source.recorded_items_digest().unwrap(),
                            // Evidence resolves when its consumer attempt starts.
                            UnixMillis::new(100 + index as u64),
                        )
                        .unwrap(),
                    ),
                )
                .unwrap(),
            ]
        } else {
            Vec::new()
        };
        memories.push(
            AttemptWorkflowMemoryV2::new(
                attempt_uuid(session_number, index),
                slots,
                blockers,
                evidence,
            )
            .unwrap(),
        );
    }
    // The goal-rework repeat attempt on the first assessment node carries the
    // active cursor for two-revision recipes; changed active items apply to
    // its slots.
    if repeat_revision {
        let index = recipe.attempts;
        attempts.push(
            SessionAttemptV2::new(
                attempt_uuid(session_number, index),
                node_id(1),
                AttemptNumberV2::new(2),
                TraceSequenceV2::new((index + 1) as u64),
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
                Some(GoalRevisionNumberV2::new(2)),
            )
            .unwrap(),
        );
        metadata.push(
            AttemptMetadataV2::new(
                attempt_uuid(session_number, index),
                UnixMillis::new(100 + index as u64),
                None,
                None,
            )
            .unwrap(),
        );
        let mut slots = Vec::new();
        for item in 0..recipe.items {
            let changed_slot = item < changed;
            let value = if recipe.filled_values && (recipe.value_chars > 0) {
                let prefix = if changed_slot {
                    format!("c{index}i{item}y")
                } else {
                    format!("a{index}i{item}x")
                };
                Some(RecordedItemValueV2::text(text_payload(&prefix, recipe.value_chars)).unwrap())
            } else {
                None
            };
            let created = UnixMillis::new(100 + index as u64);
            let updated = if value.is_some() {
                UnixMillis::new(101 + index as u64 + u64::from(changed_slot))
            } else {
                created
            };
            slots.push(
                ItemSlotStateV2::new(
                    attempt_uuid(session_number, index),
                    item_id(item),
                    ItemTypeV1::Text,
                    if value.is_some() {
                        Revision::new(if changed_slot { 2 } else { 1 })
                    } else {
                        Revision::ZERO
                    },
                    value,
                    created,
                    updated,
                )
                .unwrap(),
            );
        }
        memories.push(
            AttemptWorkflowMemoryV2::new(
                attempt_uuid(session_number, index),
                slots,
                Vec::new(),
                Vec::new(),
            )
            .unwrap(),
        );
    }
    let mut revisions = Vec::new();
    for revision in 1..=recipe.goal_revisions {
        let criteria: Vec<GoalCriterionV2> = (0..recipe.criteria)
            .map(|criterion| {
                GoalCriterionV2::new(
                    criterion_id(criterion),
                    format!("Registered criterion {criterion} for revision {revision}."),
                )
                .unwrap()
            })
            .collect();
        let (binding_trace, reason) = revision_meta[revision - 1];
        // The revision is created when its binding attempt starts: trace 1's
        // attempt for the initial revision, the rework repeat attempt for a
        // terminal reactivation.
        let created_at = 100 + binding_trace - 1;
        revisions.push(
            GoalRevisionRecordV2::new(
                GoalRevisionNumberV2::new(revision as u64),
                (revision > 1).then(|| GoalRevisionNumberV2::new(revision as u64 - 1)),
                GoalStatementV2::new(format!("Registered goal revision {revision}.")).unwrap(),
                GoalDefinitionV2::new(criteria).unwrap(),
                reason
                    .is_some()
                    .then(|| GoalRevisionReasonV2::new("Explicit registered revision.").unwrap()),
                // The second revision reworks from the terminal cursor back
                // to the first assessment node and reactivates it.
                (binding_trace > 1).then_some(node_id(1)),
                binding_trace > 1,
                Some(ActorAttributionV2::new("planner").unwrap()),
                TraceSequenceV2::new(binding_trace),
                UnixMillis::new(created_at),
            )
            .unwrap(),
        );
    }
    let mut assessments = Vec::new();
    // Criterion assessments belong to every completed goal-decision attempt;
    // each of them also records a decision and a goal assessment.
    let assessed: Vec<usize> = (0..recipe.attempts.saturating_sub(1))
        .filter(|index| index % 2 == 1)
        .collect();
    let mut decisions = Vec::new();
    let mut goal_assessments = Vec::new();
    for &attempt in &assessed {
        let memory = &memories[attempt];
        let evidence_set = podway_core::ResolvedEvidenceSetV2::new(
            memory
                .evidence()
                .iter()
                .map(|reference| reference.resolution().clone())
                .collect(),
        )
        .unwrap();
        let has_evidence = !memory.evidence().is_empty();
        let attempt_goal_revision =
            GoalRevisionNumberV2::new(revision_of_attempt((attempt + 1) as u64));
        let results: Vec<podway_core::CriterionAssessmentResultV2> = (0..recipe.criteria)
            .map(|criterion| {
                // Citations must be unique per result: at most one evidence
                // citation (only owners that consume evidence) plus one item
                // citation.
                let mut citations = Vec::new();
                if has_evidence {
                    citations.push(CriterionCitationV2::Evidence(node_id(2)));
                }
                citations.push(CriterionCitationV2::Item(item_id(0)));
                CriterionAssessmentResultV2::new(
                    criterion_id(criterion),
                    CriterionStatusV2::Satisfied,
                    CriterionAssessmentReasonV2::new("Registered assessment reason for inventory.")
                        .unwrap(),
                    citations,
                )
                .unwrap()
            })
            .collect();
        decisions.push(
            podway_core::DecisionRecordV2::new(podway_core::DecisionRecordInputV2 {
                trace: TraceSequenceV2::new((attempt + 1) as u64),
                session_id: session_id.clone(),
                session_revision: Revision::new((attempt + 1) as u64),
                procedure_snapshot_id: snapshot.snapshot_id().clone(),
                procedure_digest: snapshot.digest().clone(),
                graph_node_id: node_id(attempt),
                node_definition_id: podway_core::NodeDefinitionId::new("assess-def").unwrap(),
                attempt_id: attempt_uuid(session_number, attempt),
                attempt_number: AttemptNumberV2::FIRST,
                goal_revision: Some(attempt_goal_revision.clone()),
                selected_option: podway_core::OptionId::new("continue").unwrap(),
                route_effect: podway_core::TransitionEffectV2::Advance,
                route_target: node_id(attempt + 1),
                reason: podway_core::ReasonV2::new("Continue the registered chain.").unwrap(),
                evidence: evidence_set.clone(),
                actor: Some(ActorAttributionV2::new("reviewer").unwrap()),
                recorded_at: UnixMillis::new(101 + attempt as u64),
            })
            .unwrap(),
        );
        let attempt_revision = GoalRevisionNumberV2::new(revision_of_attempt((attempt + 1) as u64));
        assessments.push(
            AttemptCriterionAssessmentStateV2::new(
                attempt_uuid(session_number, attempt),
                attempt_revision.clone(),
                results
                    .iter()
                    .cloned()
                    .map(|result| {
                        CriterionAssessmentStateV2::new(
                            result,
                            Some(ActorAttributionV2::new("reviewer").unwrap()),
                            UnixMillis::new(101 + attempt as u64),
                        )
                    })
                    .collect(),
            )
            .unwrap(),
        );
        goal_assessments.push(
            podway_core::GoalAssessmentRecordV2::new(
                attempt_revision,
                podway_core::GoalOutcome::Achieved,
                results,
                evidence_set,
                Some(ActorAttributionV2::new("reviewer").unwrap()),
                attempt_uuid(session_number, attempt),
                node_id(attempt),
                TraceSequenceV2::new((attempt + 1) as u64),
                UnixMillis::new(101 + attempt as u64),
            )
            .unwrap(),
        );
    }
    let workflow = WorkflowMemoryStateV2::new(memories, decisions, Vec::new()).unwrap();
    let goal = GoalStateV2::new(
        Some(GoalRevisionNumberV2::new(recipe.goal_revisions as u64)),
        revisions,
        assessments,
        goal_assessments,
    )
    .unwrap();
    let bump = u64::from(bump_revisions);
    GraphSessionStateV2::new_with_goal_state(
        Revision::new(recipe.attempts as u64 + bump),
        "READS-001 inventory",
        snapshot.clone(),
        SessionTraceV2::from_parts(
            session_id,
            SessionLifecycle::Running,
            Revision::new(recipe.attempts as u64 + bump),
            attempts,
        )
        .unwrap(),
        counters,
        metadata,
        workflow,
        goal,
        UnixMillis::new(90),
        None,
        None,
        None,
    )
    .unwrap()
}

pub(super) fn reads001_fixture(
    recipe: &Recipe,
    session_number: u64,
    snapshot_number: u64,
) -> (
    tempfile::TempDir,
    podway_store::direct_store::DirectStoreV1,
    GraphSessionStateV2,
) {
    let identity = DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new(format!("00000000-0000-4000-8000-{session_number:012x}")).unwrap(),
        digest('b'),
    );
    let (root, direct) = fixture(&identity);
    let snapshot = reads001_snapshot(recipe, snapshot_number);
    let state = reads001_session(recipe, &snapshot, session_number, 0, false);
    seed(&direct, &state);
    (root, direct, state)
}

fn cold_current_graph(
    direct: &podway_store::direct_store::DirectStoreV1,
) -> Result<Option<GraphSessionStateV2>, podway_store::direct_store::DirectStoreErrorV1> {
    direct.read(deadline(), |snapshot| snapshot.current_graph())
}

/// One registered inventory expectation: a unique needle identifying the SQL
/// text plus its exact (prepare, execute, row) triple.
struct Expected {
    needle: &'static str,
    prepares: u64,
    executes: u64,
    rows: u64,
}

/// Criterion-assessment owner attempts: every completed assessment-node
/// attempt records criterion results, a decision and a goal assessment.
pub(super) fn assessed_owner_attempts(recipe: &Recipe) -> u64 {
    (0..recipe.attempts.saturating_sub(1))
        .filter(|index| index % 2 == 1)
        .count() as u64
}

/// Resolved evidence-reference rows: only the second assessment attempt
/// consumes evidence.
fn evidence_rows(recipe: &Recipe) -> u64 {
    (recipe.attempts >= 5).then_some(1).unwrap_or(0)
}

fn expected(needle: &'static str, prepares: u64, executes: u64, rows: u64) -> Expected {
    Expected {
        needle,
        prepares,
        executes,
        rows,
    }
}

/// Exact per-SQL (prepare, execute, row) expectations for one cold
/// `current_graph` load, asserted by unique SQL needles, plus the needles
/// that must not appear at all on the cold path.
fn expected_cold_inventory(recipe: &Recipe) -> (Vec<Expected>, Vec<&'static str>) {
    let attempts = attempt_total(recipe);
    let items = recipe.items as u64;
    let revisions = recipe.goal_revisions as u64;
    let criteria = recipe.criteria as u64;
    let assessed = assessed_owner_attempts(recipe);
    let _citations = recipe.citations as u64;
    let mut all = vec![
        expected("value_json IS NOT NULL", 1, 1, attempts * items),
        expected("FROM v2_blockers", 1, 1, recipe.blocker_attempts as u64),
        expected(
            "FROM v2_resolved_evidence_references",
            1,
            1,
            evidence_rows(recipe),
        ),
        expected(
            "FROM v2_decision_records",
            1,
            1,
            assessed_owner_attempts(recipe),
        ),
        expected("FROM v2_rework_records", 1, 1, 0),
        expected("predecessor_revision", 1, 1, revisions),
        expected(
            "criterion_ordinal, statement FROM v2_goal_criteria",
            1,
            1,
            revisions * criteria,
        ),
        expected(
            "v2_criterion_assessment_results r JOIN",
            1,
            1,
            assessed * criteria,
        ),
        expected(
            "record_digest FROM v2_goal_assessments",
            1,
            1,
            assessed_owner_attempts(recipe),
        ),
        expected("AS citation", 1, 1, 1),
        expected("PRAGMA user_version", 2, 2, 2),
        expected("FROM v2_terminal_dispositions", 1, 1, 0),
        expected(
            "canonical_placement_json FROM v2_graph_nodes",
            1,
            1,
            recipe.attempts as u64,
        ),
        expected("validity, goal_revision FROM v2_attempts", 1, 1, attempts),
        expected("terminal_reason FROM v2_attempts", 1, 1, attempts),
        expected("rework_traversal_count", 1, 1, recipe.attempts as u64),
        expected("canonical_json, source_kind", 1, 1, 1),
        expected("SELECT source_label", 1, 1, 1),
        expected(
            "COUNT(*) FROM v2_task_sessions WHERE activity = 'current'",
            1,
            1,
            1,
        ),
        expected("SELECT COUNT(*) FROM v2_workspace_state", 1, 1, 1),
        expected("SELECT w.workspace_revision", 1, 1, 1),
    ];
    if assessed * criteria > 0 {
        // One item citation per result plus one evidence citation for the
        // single evidence-consuming assessment owner.
        let evidence_bonus = if evidence_rows(recipe) > 0 {
            criteria
        } else {
            0
        };
        all.push(expected(
            "citation_ordinal, citation_kind",
            1,
            1,
            assessed * criteria + evidence_bonus,
        ));
    }
    let mut absent = vec![
        "typeof(value_json) != 'null'",
        "attempt_id=?1 AND item_id=?2",
        "activity = 'inactive'",
        "SELECT digest FROM v2_procedure_snapshots WHERE snapshot_id=?1",
    ];
    if assessed * criteria == 0 {
        absent.push("citation_ordinal, citation_kind");
    }
    (all, absent)
}

fn assert_inventory(
    recipe: &Recipe,
    report_sql: &std::collections::BTreeMap<
        String,
        podway_store::test_support::query_account::SqlCounts,
    >,
) {
    let (mut expected_all, absent) = expected_cold_inventory(recipe);
    // The snapshot adapter's binding read is part of the registered read path
    // whenever the unchanged implementation reads it eagerly for this
    // command; pin its counts when observed.
    if report_sql
        .keys()
        .any(|key| key.contains("last_validated_root FROM workspace_state"))
    {
        expected_all.push(expected(
            "last_validated_root FROM workspace_state",
            1,
            1,
            1,
        ));
    }
    for item in &expected_all {
        let matches: Vec<&String> = report_sql
            .keys()
            .filter(|key| key.contains(item.needle))
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "needle {:?} must identify exactly one SQL text, found {:?}",
            item.needle,
            matches
        );
        let counts = &report_sql[matches[0]];
        assert_eq!(
            (counts.prepares, counts.executes, counts.rows),
            (item.prepares, item.executes, item.rows),
            "needle {:?} on {:?}",
            item.needle,
            matches[0]
        );
    }
    for needle in absent {
        assert!(
            report_sql.keys().all(|key| !key.contains(needle)),
            "needle {needle:?} must be absent from the cold inventory"
        );
    }
    for key in report_sql.keys() {
        assert!(
            expected_all.iter().any(|item| key.contains(item.needle)),
            "every observed SQL text must be covered by the registered inventory: {key:?}"
        );
    }
}

fn filled_slot_values(state: &GraphSessionStateV2) -> u64 {
    state
        .workflow_memory()
        .attempts()
        .iter()
        .flat_map(|attempt| attempt.item_slots())
        .filter(|slot| slot.value().is_some())
        .count() as u64
}

pub(super) fn encoded_payload_bytes(state: &GraphSessionStateV2) -> u64 {
    state
        .workflow_memory()
        .attempts()
        .iter()
        .flat_map(|attempt| attempt.item_slots())
        .filter_map(|slot| slot.value().map(recorded_value_encoded_size_v2))
        .map(|size| size.map(|size| size as u64).unwrap_or_default())
        .sum()
}

/// Largest single-attempt raw payload total: the slot loader acquires all of
/// one attempt's rows before decoding, so the raw window peak is the maximum
/// per-attempt sum.
fn max_attempt_payload_bytes(state: &GraphSessionStateV2) -> u64 {
    state
        .workflow_memory()
        .attempts()
        .iter()
        .map(|attempt| {
            attempt
                .item_slots()
                .iter()
                .filter_map(|slot| slot.value().map(recorded_value_encoded_size_v2))
                .map(|size| size.map(|size| size as u64).unwrap_or_default())
                .sum::<u64>()
        })
        .max()
        .unwrap_or(0)
}

fn measure_cold(recipe: &Recipe, tag: &str, session_number: u64, snapshot_number: u64) {
    let (root, _direct, state) = reads001_fixture(recipe, session_number, snapshot_number);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let loaded = cold_current_graph(&cold).unwrap();
    let report = accountant.finish();
    assert_eq!(loaded.as_ref(), Some(&state));
    assert_inventory(recipe, &report.sql);
    // Frozen READS-001 actuals, before any query rewrite. These fixtures
    // keep identical rows, variable-length columns and payload retention.
    let (old_statements, old_rows, old_column_bytes, old_combined_peak) = match tag {
        "S" => (43, 65, 6_819, 858),
        "M" => (140, 438, 231_250, 206_226),
        "grown-L" => (201, 995, 36_141_030, 36_152_082),
        "N1024" => (4_118, 8_215, 822_518, 51_300),
        "empty" => (23, 16, 2_089, 0),
        "single" => (23, 17, 2_421, 108),
        _ => unreachable!("registered cold fixture"),
    };
    let target = if assessed_owner_attempts(recipe) > 0 {
        24
    } else {
        23
    };
    assert_eq!(
        (report.totals.prepares, report.totals.executes),
        (target, target)
    );
    if old_statements > 23 {
        assert!(report.totals.executes < old_statements);
    } else {
        assert!(report.totals.executes <= old_statements);
    }
    assert_eq!(report.totals.rows, old_rows);
    // The frozen run's hex-encoded fixture root occupied 154 bytes. The
    // owned TMPDIR changes this input, not table-family acquisition. Keep
    // the observer's actual bytes and account for the exact root field in
    // the expected total instead of hiding it in the measurement.
    let current_root_bytes = cold.layout().root().as_encoded().len() as u64;
    assert_eq!(
        report.totals.column_bytes,
        old_column_bytes - 154 + current_root_bytes
    );
    assert_eq!(report.retention.combined_peak, old_combined_peak);
    let filled = filled_slot_values(&state);
    let expected_bytes = encoded_payload_bytes(&state);
    assert_eq!(report.retention.fresh_payloads, filled);
    assert_eq!(report.retention.fresh_bytes_cumulative, expected_bytes);
    assert_eq!(report.retention.decoded_retained_in_results, expected_bytes);
    assert_eq!(report.retention.decoded_released_at_scope_end, 0);
    assert_eq!(report.retention.raw_released_at_scope_end, 0);
    let attempt_peak = max_attempt_payload_bytes(&state);
    assert_eq!(report.retention.raw_window_peak, attempt_peak);
    assert!(report.retention.combined_peak >= attempt_peak);
    assert!(report.retention.decoded_live_peak >= attempt_peak);
    let slots = report
        .sql
        .iter()
        .find(|(key, _)| key.contains("value_json IS NOT NULL"))
        .map(|(_, counts)| counts)
        .unwrap();
    assert!(slots.column_bytes >= expected_bytes);
    println!(
        "READS002_CANDIDATE|store-cold|fixture={tag}|prepares={}|executes={}|rows={}|column_bytes={}|slot_payload_fresh_bytes={}|raw_window_peak={}|combined_peak={}",
        report.totals.prepares,
        report.totals.executes,
        report.totals.rows,
        report.totals.column_bytes,
        report.retention.fresh_bytes_cumulative,
        report.retention.raw_window_peak,
        report.retention.combined_peak
    );
}

#[test]
fn cold_read_inventory_small_medium_maximum_and_grown_large() {
    measure_cold(&recipe_small(), "S", 101, 201);
    measure_cold(&recipe_medium(), "M", 102, 202);
    measure_cold(&recipe_maximum_nodes(), "N1024", 103, 203);
    let grown = recipe_grown_large();
    let (root, direct, _state) = reads001_fixture(&grown, 104, 204);
    let database_bytes = std::fs::metadata(direct.layout().database()).unwrap().len();
    println!("READS001_BASELINE|grown-L-database-bytes={database_bytes}");
    assert!(
        database_bytes >= 16 * 1024 * 1024,
        "grown-L recipe must keep its registered database floor"
    );
    drop(direct);
    measure_cold(&grown, "grown-L", 104, 204);
    drop(root);
}

#[test]
fn cold_read_inventory_empty_and_single_entry_controls() {
    measure_cold(&recipe_empty(), "empty", 111, 211);
    measure_cold(&recipe_single(), "single", 112, 212);
}

#[test]
fn observers_are_neutral_for_paired_observed_and_unobserved_reads() {
    let recipe = recipe_medium();
    let (root, _direct, state) = reads001_fixture(&recipe, 105, 205);
    let cold = store(root.path());
    let unobserved = cold_current_graph(&cold).unwrap();
    let accountant = QueryAccountant::start();
    let observed = cold_current_graph(&cold).unwrap();
    let report = accountant.finish();
    assert_eq!(unobserved, observed);
    assert_eq!(observed.as_ref(), Some(&state));
    assert_inventory(&recipe, &report.sql);
}

/// Characterizes today's selective prepared-graph reload: header-only slot
/// queries per attempt plus one point reload per mismatched slot. The
/// 128-pair chunk bound is a future-code predicate (READS-002), not an
/// assertion about this unchanged path.
#[test]
fn prepared_reload_counts_unchanged_sparse_and_dense_selective_misses() {
    let recipe = recipe_medium();
    let total_slots = attempt_total(&recipe) * recipe.items as u64;
    for (tag, changed) in [
        ("unchanged", 0usize),
        ("sparse", 1usize),
        ("dense", recipe.items),
    ] {
        let (root, direct, state) = reads001_fixture(&recipe, 106, 206);
        // Independent scenarios start from a cleared payload registry; the
        // prepared input graph stays alive through both windows so its
        // registered pointer identities remain valid.
        clear_payload_registry();
        let prepare_accountant = QueryAccountant::start();
        let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
        let prepare_report = prepare_accountant.finish();
        assert_eq!(
            prepare_report.retention.fresh_payloads,
            filled_slot_values(&state)
        );
        // Measured live prepared-cache baseline: every payload of the
        // prepared graph must be registered (identity proof); their logical
        // serialized sizes are the cached input units.
        let prepared_bytes: u64 = prepared
            .graph()
            .workflow_memory()
            .attempts()
            .iter()
            .flat_map(|attempt| attempt.item_slots())
            .filter(|slot| slot.value().is_some())
            .map(|slot| {
                payload_bytes_of(slot)
                    .map(|bytes| bytes as u64)
                    .expect("prepared payload must be registered by its prepare window")
            })
            .sum();
        // Pointer identities of the prepared active attempt's slots, proven
        // shared or distinct inside the revalidation closure below.
        let prepared_active = prepared
            .graph()
            .trace()
            .active_attempt()
            .unwrap()
            .attempt_id()
            .clone();
        let prepared_memory = prepared
            .graph()
            .workflow_memory()
            .attempts()
            .iter()
            .find(|memory| memory.attempt_id() == &prepared_active)
            .unwrap();
        let prepared_pointers: Vec<Option<usize>> = prepared_memory
            .item_slots()
            .iter()
            .map(payload_pointer)
            .collect();
        if changed > 0 {
            // Advance persisted state once without the prepared graph so the
            // next prepared validation takes the stale-prepared cached path.
            let snapshot = state.snapshot().clone();
            let next = reads001_session(&recipe, &snapshot, 106, changed, true);
            let advance = request(&format!("advance-{tag}"));
            direct
                .mutate(
                    &advance,
                    deadline(),
                    || UnixMillis::new(900),
                    |transaction| {
                        transaction.now();
                        transaction.replace_graph(
                            state.workspace_revision(),
                            state.trace().revision(),
                            &next,
                        )?;
                        Ok(success(
                            &advance,
                            serde_json::json!({"advanced": true}),
                            transaction.now(),
                        ))
                    },
                )
                .unwrap();
        }
        // The stale prepared graph now misses every changed slot; with no
        // interim change the stamps still match and the prepared graph is
        // reused directly without any slot statement.
        let accountant = QueryAccountant::start();
        // The live prepared cache participates in the combined retention
        // peak of the measured window.
        declare_cached_input_bytes(prepared_bytes);
        let revalidate = request(&format!("revalidate-{tag}"));
        direct
            .mutate(
                &revalidate,
                deadline(),
                || UnixMillis::new(950),
                |transaction| {
                    let current = transaction
                        .use_prepared_graph(Some(&prepared))?
                        .expect("current graph");
                    // Pointer-identity proof inside the closure: unchanged slots
                    // share the prepared payload object; changed slots do not.
                    let memory = current
                        .workflow_memory()
                        .attempts()
                        .iter()
                        .find(|memory| memory.attempt_id() == &prepared_active)
                        .unwrap();
                    for (index, slot) in memory.item_slots().iter().enumerate() {
                        let pointer = payload_pointer(slot);
                        if index < changed {
                            assert_ne!(
                                pointer, prepared_pointers[index],
                                "a reloaded changed slot must not share the prepared payload"
                            );
                        } else {
                            assert_eq!(
                                pointer, prepared_pointers[index],
                                "an unchanged slot must reuse the prepared payload object"
                            );
                        }
                    }
                    Ok(success(
                        &revalidate,
                        serde_json::json!({"revalidated": true}),
                        transaction.now(),
                    ))
                },
            )
            .unwrap();
        let report = accountant.finish();
        assert_eq!(report.retention.cached_input_bytes, prepared_bytes);
        let headers: Vec<&String> = report
            .sql
            .keys()
            .filter(|key| key.contains("typeof(value_json) != 'null'"))
            .collect();
        let header_counts = headers
            .first()
            .map(|key| report.sql[*key])
            .unwrap_or_default();
        if changed == 0 {
            assert_eq!(header_counts.prepares, 0);
            assert_eq!(header_counts.executes, 0);
            assert_eq!(header_counts.rows, 0);
        } else {
            // READS-002 mixed path: one session-wide header execution
            // replaces the per-attempt header statements.
            assert_eq!(header_counts.prepares, 1);
            assert_eq!(header_counts.executes, 1);
            assert_eq!(header_counts.rows, total_slots);
        }
        let reloads: Vec<&String> = report
            .sql
            .keys()
            .filter(|key| key.contains("attempt_id=?1 AND item_id=?2"))
            .collect();
        let reload_executes = reloads
            .first()
            .map(|key| report.sql[*key].executes)
            .unwrap_or(0);
        // READS-002 mixed path: changed payloads arrive through bounded
        // global chunks (ceil(changed/128) executions, prepares asserted
        // separately) instead of one point reload per changed slot.
        assert_eq!(reload_executes, 0);
        let chunk_executes: u64 = report
            .sql
            .iter()
            .filter(|(key, _)| key.contains("WITH wanted"))
            .map(|(_, counts)| counts.executes)
            .sum();
        let chunk_prepares: u64 = report
            .sql
            .iter()
            .filter(|(key, _)| key.contains("WITH wanted"))
            .map(|(_, counts)| counts.prepares)
            .sum();
        let expected_chunks = u64::try_from(changed.div_ceil(128)).unwrap();
        assert_eq!(chunk_executes, expected_chunks);
        assert_eq!(chunk_prepares, expected_chunks);
        assert_eq!(report.retention.fresh_payloads, changed as u64);
        assert_eq!(report.retention.unknown_reuse_payloads, 0);
        let expected_copies = if changed == 0 {
            0
        } else {
            total_slots - changed as u64
        };
        assert_eq!(report.retention.container_copies, expected_copies);
        // The three retention units coexist at the changed item's decode:
        // cached input + held raw + decoded-so-far fresh bytes.
        assert!(
            report.retention.combined_peak
                >= report.retention.cached_input_bytes
                    + report.retention.raw_window_peak
                    + report.retention.fresh_bytes_cumulative,
            "combined peak must cover cached input, raw and fresh units for {tag}"
        );
        if changed == 0 {
            assert_eq!(report.retention.combined_peak, prepared_bytes);
        }
        println!(
            "READS001_BASELINE|prepared-reload|fixture={tag}|header_executes={}|point_reloads={reload_executes}|reused_payloads={}|fresh_payloads={}|cached_input_bytes={}|raw_window_peak={}|combined_peak={}",
            header_counts.executes,
            report.retention.reused_payloads,
            report.retention.fresh_payloads,
            report.retention.cached_input_bytes,
            report.retention.raw_window_peak,
            report.retention.combined_peak
        );
        drop(direct);
        drop(root);
    }
}

/// Error-path retention honesty: a malformed first payload decodes to
/// nothing, every raw byte is still released, and no fresh decoded payload
/// is claimed (READS negative observer assertion).
#[test]
fn failed_first_payload_decode_reports_zero_fresh_and_exact_raw_release() {
    let recipe = recipe_single();
    let (root, direct, _state) = reads001_fixture(&recipe, 107, 207);
    let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
    let changed = connection
        .execute(
            "UPDATE v2_item_slots SET value_json='{\"kind\":\"mystery\"}' WHERE attempt_id=?1 AND item_id='item-0'",
            [attempt_uuid(107, 0).as_str()],
        )
        .unwrap();
    assert_eq!(changed, 1, "exactly one recorded slot value changes");
    drop(connection);
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let refused = cold_current_graph(&cold);
    let report = accountant.finish();
    assert!(refused.is_err(), "cold read must refuse malformed payload");
    assert_eq!(report.retention.decode_attempts, 1);
    assert_eq!(report.retention.fresh_payloads, 0);
    assert_eq!(report.retention.fresh_bytes_cumulative, 0);
    assert_eq!(report.retention.decoded_retained_in_results, 0);
    assert!(report.retention.raw_released_at_scope_end > 0);
    println!(
        "READS001_BASELINE|failed-decode|decode_attempts={}|fresh=0|raw_released_at_scope_end={}",
        report.retention.decode_attempts, report.retention.raw_released_at_scope_end
    );
}

/// Sums registered statement executions whose SQL text contains the needle.
fn state002_executes(
    sql: &std::collections::BTreeMap<String, podway_store::test_support::query_account::SqlCounts>,
    needle: &str,
) -> u64 {
    sql.iter()
        .filter(|(key, _)| key.contains(needle))
        .map(|(_, counts)| counts.executes)
        .sum()
}

/// STATE-002: one ordinary view admits the current graph exactly once and
/// keeps the loader's admitted dispositions instead of reloading them.
#[test]
fn state002_view_admits_graph_and_dispositions_once() {
    let identity = DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000141").unwrap(),
        digest('b'),
    );
    let (root, direct) = fixture(&identity);
    let running = podway_store::test_support::running_archive_fixture_session_v2(2110, 2210, 10);
    let completed = running
        .complete_active_action_v2(
            running.trace().revision(),
            running.trace().active_attempt().unwrap().attempt_id(),
            None,
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    seed(&direct, &completed);
    let dispose = request("state002-dispose-current");
    direct
        .mutate(
            &dispose,
            deadline(),
            || UnixMillis::new(2111),
            |transaction| {
                transaction.now();
                transaction.record_disposition(
                    &podway_core::TerminalDispositionV2::not_required(
                        completed.trace().session_id().clone(),
                        completed.trace().revision(),
                        "No handoff required",
                        None,
                        UnixMillis::new(2111),
                    )
                    .unwrap(),
                )?;
                Ok(success(
                    &dispose,
                    serde_json::json!({"disposed":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let observed = cold
        .read(deadline(), |snapshot| {
            let view = snapshot.view(UnixMillis::new(2112))?;
            Ok((
                view.graph().map(|graph| graph.trace().session_id().clone()),
                view.terminal_dispositions().to_vec(),
            ))
        })
        .unwrap();
    let report = accountant.finish();
    assert_eq!(
        observed.0.as_ref(),
        Some(completed.trace().session_id()),
        "the admitted graph is the view's graph"
    );
    assert_eq!(
        observed.1.len(),
        1,
        "the retained disposition reaches the view"
    );
    assert_eq!(
        observed.1[0].terminal_session_revision(),
        completed.trace().revision()
    );
    assert_eq!(
        state002_executes(&report.sql, "FROM v2_terminal_dispositions"),
        1,
        "the view must load dispositions exactly once"
    );
    assert_eq!(
        state002_executes(
            &report.sql,
            "SELECT COUNT(*) FROM v2_task_sessions WHERE activity = 'current'"
        ),
        1,
        "the view must admit the current graph exactly once"
    );
}

/// STATE-002: one archived view admits the archived graph exactly once and
/// keeps the loader's admitted dispositions instead of reloading them.
#[test]
fn state002_archived_view_admits_dispositions_once() {
    let identity = DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000142").unwrap(),
        digest('b'),
    );
    let (root, direct) = fixture(&identity);
    let running = podway_store::test_support::running_archive_fixture_session_v2(2120, 2220, 10);
    let completed = running
        .complete_active_action_v2(
            running.trace().revision(),
            running.trace().active_attempt().unwrap().attempt_id(),
            None,
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    seed(&direct, &completed);
    archive(&direct, &completed, UnixMillis::new(2121));
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let observed = cold
        .read(deadline(), |snapshot| {
            let view = snapshot
                .archived_view(completed.trace().session_id(), UnixMillis::new(2122))?
                .expect("archived session admits");
            Ok((
                view.graph().map(|graph| graph.trace().session_id().clone()),
                view.terminal_dispositions().to_vec(),
            ))
        })
        .unwrap();
    let report = accountant.finish();
    assert_eq!(
        observed.0.as_ref(),
        Some(completed.trace().session_id()),
        "the admitted archived graph is the view's graph"
    );
    assert_eq!(observed.1.len(), 1);
    assert_eq!(
        observed.1[0].terminal_session_revision(),
        completed.trace().revision()
    );
    assert_eq!(
        state002_executes(&report.sql, "FROM v2_terminal_dispositions"),
        1,
        "the archived view must load dispositions exactly once"
    );
    assert_eq!(
        state002_executes(&report.sql, "activity = 'inactive'"),
        1,
        "the archived view must admit the archived graph exactly once"
    );
}
