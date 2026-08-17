//! Deterministic, bounded Procedure v2 status and next projections.
//!
//! Projection consumes one coherent Store observation and the immutable canonical snapshot
//! captured at session start. It never resolves the Procedure source or mutates runtime state.

use std::{error::Error, fmt};

use podway_config::{
    ParsedNodeDefinition, ParsedProcedure, ParsedProcedureV2, ProcedureDocumentFormat,
    goal_revision_safe_targets_v2, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{
    ArtifactLocationKindV1, AttemptLifecycle, AttemptValidityV2, BlockerState, CriterionCitationV2,
    CriterionStatusV2, GoalOutcome, GraphPlacementV2, ItemSpecV2, ItemTypeV1, RecordedItemValueV2,
    ResolvedEvidenceReferenceV2, SessionAttemptV2, SessionLifecycle, TraceSequenceV2, UnixMillis,
    canonicalize_json_v1,
};
use podway_store::{
    AttemptWorkflowMemoryV2, EvidenceReadbackV2, EvidenceResolutionStateV2, GraphSessionStateV2,
    GraphWorkspaceViewV2,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const ITEM_DISPLAY_CHARS_MAX: usize = 2_048;
const STATUS_VALUES_BYTES_MAX: usize = 262_144;
const BLOCKER_WINDOW_BYTES_MAX: usize = 49_152;
const HISTORY_WINDOW_BYTES_MAX: usize = 65_536;
const OBSERVATION_ITEM_VALUE_CHARS_MAX: usize = 128;
const OBSERVATION_ITEM_VALUE_BYTES_MAX: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphStatusTierV2 {
    Compact,
    Standard,
    Verbose,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphViewErrorV2 {
    MissingGraphSession,
    PendingMutationsForCompact,
    TerminalSessionHasNoNext,
    InvalidSnapshot,
    InconsistentState(&'static str),
    TimestampOutOfRange,
}

impl fmt::Display for GraphViewErrorV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingGraphSession => formatter.write_str("no Procedure v2 session exists"),
            Self::PendingMutationsForCompact => {
                formatter.write_str("compact status requires an idle workspace queue")
            }
            Self::TerminalSessionHasNoNext => {
                formatter.write_str("a terminal Procedure v2 session has no next action")
            }
            Self::InvalidSnapshot => {
                formatter.write_str("the stored Procedure v2 snapshot cannot be rehydrated")
            }
            Self::InconsistentState(reason) => formatter.write_str(reason),
            Self::TimestampOutOfRange => formatter.write_str("timestamp is outside RFC 3339 range"),
        }
    }
}

impl Error for GraphViewErrorV2 {}

pub fn project_graph_status_v2(
    view: &GraphWorkspaceViewV2,
    tier: GraphStatusTierV2,
    history_before: Option<TraceSequenceV2>,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let state = graph_state(view)?;
    if tier == GraphStatusTierV2::Compact
        && (view.queued_job_count() != 0 || view.running_job_id().is_some())
    {
        return Err(GraphViewErrorV2::PendingMutationsForCompact);
    }
    if tier != GraphStatusTierV2::Verbose && history_before.is_some() {
        return Err(GraphViewErrorV2::InconsistentState(
            "a history cursor is valid only for verbose status",
        ));
    }
    let procedure = rehydrate_snapshot(state)?;
    let current = CurrentProjection::derive(state, &procedure)?;

    let mut result = Map::new();
    let schema = match tier {
        GraphStatusTierV2::Compact => "podway.compact-status-result/v3",
        GraphStatusTierV2::Standard | GraphStatusTierV2::Verbose => "podway.status-result/v3",
    };
    result.insert("schema".to_owned(), json!(schema));
    if tier != GraphStatusTierV2::Compact {
        result.insert(
            "tier".to_owned(),
            json!(if tier == GraphStatusTierV2::Standard {
                "standard"
            } else {
                "verbose"
            }),
        );
    }
    result.insert("procedure".to_owned(), procedure_identity(state));
    result.insert("session".to_owned(), session_identity(state));
    result.insert(
        "current".to_owned(),
        current
            .as_ref()
            .map(CurrentProjection::identity)
            .unwrap_or(Value::Null),
    );
    result.insert(
        "goal_tracking".to_owned(),
        json!(state.snapshot().goal_tracking()),
    );
    result.insert(
        "goal_defined".to_owned(),
        json!(state.goal_state().current_revision().is_some()),
    );
    add_goal_summary(&mut result, state);
    result.insert(
        "trace_length".to_owned(),
        json!(state.trace().attempts().len()),
    );
    result.insert("counters".to_owned(), counters(state));
    result.insert(
        "items".to_owned(),
        Value::Array(
            current
                .as_ref()
                .map(CurrentProjection::compact_items)
                .unwrap_or_default(),
        ),
    );
    result.insert("queue".to_owned(), queue(view));
    if tier == GraphStatusTierV2::Compact {
        return Ok(result);
    }

    result.insert("purpose".to_owned(), json!(state.snapshot().purpose()));
    result.insert(
        "missing_required_item_ids".to_owned(),
        Value::Array(
            current
                .as_ref()
                .map(CurrentProjection::missing_ids)
                .unwrap_or_default(),
        ),
    );
    let (blockers, blockers_truncated) = current
        .as_ref()
        .map(CurrentProjection::blocker_window)
        .transpose()?
        .unwrap_or_default();
    result.insert("blocker_window".to_owned(), Value::Array(blockers));
    result.insert("blockers_truncated".to_owned(), json!(blockers_truncated));
    let (item_values, items_total, items_truncated) = current
        .as_ref()
        .map(CurrentProjection::item_values)
        .unwrap_or_default();
    result.insert("items_total".to_owned(), json!(items_total));
    result.insert("items_truncated".to_owned(), Value::Bool(items_truncated));
    result.insert("item_values".to_owned(), Value::Array(item_values));
    result.insert(
        "references".to_owned(),
        Value::Array(
            current
                .as_ref()
                .map(|current| current.reference_metadata(&procedure))
                .transpose()?
                .unwrap_or_default(),
        ),
    );
    result.insert(
        "allowed_option_ids".to_owned(),
        Value::Array(
            current
                .as_ref()
                .map(CurrentProjection::allowed_option_ids)
                .unwrap_or_default(),
        ),
    );
    result.insert(
        "allowed_manual_rework_targets".to_owned(),
        manual_rework_targets(state, &procedure),
    );
    add_static_disposition(&mut result, current.as_ref());
    if let Some(goal) = goal_display(state, current.as_ref()) {
        result.insert("goal".to_owned(), goal);
    }
    if tier == GraphStatusTierV2::Verbose {
        add_histories(&mut result, state, &procedure, history_before)?;
    }
    Ok(result)
}

pub fn project_graph_next_v2(
    view: &GraphWorkspaceViewV2,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let state = graph_state(view)?;
    if state.trace().lifecycle() == SessionLifecycle::Prepared {
        return Ok(project_prepared_next_v1(view, state));
    }
    let procedure = rehydrate_snapshot(state)?;
    let current = CurrentProjection::derive(state, &procedure)?
        .ok_or(GraphViewErrorV2::TerminalSessionHasNoNext)?;
    let mut result = Map::new();
    result.insert("schema".to_owned(), json!("podway.next-result/v2"));
    result.insert("procedure_schema".to_owned(), json!("podway.procedure/v2"));
    result.insert(
        "procedure_digest".to_owned(),
        json!(state.snapshot().digest().as_str()),
    );
    result.insert(
        "goal_tracking".to_owned(),
        json!(state.snapshot().goal_tracking()),
    );
    result.insert(
        "goal_defined".to_owned(),
        json!(state.goal_state().current_revision().is_some()),
    );
    add_goal_summary(&mut result, state);
    if let Some(goal) = goal_display(state, Some(&current)) {
        result.insert("goal".to_owned(), goal);
    }
    result.insert("node".to_owned(), current.node_identity());
    result.insert("attempt".to_owned(), current.attempt_identity());
    result.insert(
        "trace_length".to_owned(),
        json!(state.trace().attempts().len()),
    );
    result.insert("counters".to_owned(), counters(state));
    result.insert("queue".to_owned(), queue(view));
    result.insert("revision".to_owned(), json!(state.trace().revision().get()));
    result.insert("readiness".to_owned(), current.readiness_value());
    result.insert(
        "missing_required_item_count".to_owned(),
        json!(current.missing_required.len()),
    );
    result.insert(
        "missing_required_items".to_owned(),
        Value::Array(current.missing_items()),
    );
    result.insert(
        "blockers_total".to_owned(),
        json!(current.open_blockers.len()),
    );
    let (blockers, blockers_truncated) = current.blocker_window()?;
    result.insert("blockers".to_owned(), Value::Array(blockers));
    result.insert("blockers_truncated".to_owned(), json!(blockers_truncated));
    result.insert(
        "references".to_owned(),
        Value::Array(current.reference_metadata(&procedure)?),
    );
    result.insert(
        "readback".to_owned(),
        Value::Array(readback(state, &current, &procedure)?),
    );
    result.insert(
        "allowed_manual_rework_targets".to_owned(),
        manual_rework_targets(state, &procedure),
    );

    match current.definition {
        ParsedNodeDefinition::Action(definition) => {
            result.insert("title".to_owned(), json!(definition.title()));
            result.insert("intent".to_owned(), json!(definition.intent()));
            if let Some(description) = definition.description() {
                result.insert("description".to_owned(), json!(description));
            }
            result.insert("instructions".to_owned(), json!(definition.instructions()));
            add_static_disposition(&mut result, Some(&current));
        }
        ParsedNodeDefinition::Decision(definition) => {
            result.insert("title".to_owned(), json!(definition.title()));
            result.insert("objective".to_owned(), json!(definition.objective()));
            result.insert("prompt".to_owned(), json!(definition.prompt()));
            if let Some(description) = definition.description() {
                result.insert("description".to_owned(), json!(description));
            }
            let allowed_options = current.allowed_option_ids();
            result.insert(
                "options".to_owned(),
                Value::Array(
                    definition
                        .options()
                        .iter()
                        .filter(|option| {
                            definition.assessment().is_none()
                                || allowed_options.is_empty()
                                || allowed_options.contains(&json!(option.id().as_str()))
                        })
                        .map(|option| {
                            let mut value = Map::new();
                            value.insert("option_id".to_owned(), json!(option.id().as_str()));
                            value.insert("label".to_owned(), json!(option.label()));
                            if let Some(criteria) = option.criteria() {
                                value.insert("criteria".to_owned(), json!(criteria));
                            }
                            Value::Object(value)
                        })
                        .collect(),
                ),
            );
            let mut policy = Map::new();
            policy.insert("required".to_owned(), Value::Bool(true));
            if let Some(prompt) = definition.reason().prompt() {
                policy.insert("prompt".to_owned(), json!(prompt));
            }
            result.insert("reason_policy".to_owned(), Value::Object(policy));
            if !definition.evidence_guidance().is_empty() {
                result.insert(
                    "evidence_guidance".to_owned(),
                    json!(definition.evidence_guidance()),
                );
            }
        }
    }
    let actions = current.allowed_actions(state, &procedure);
    result.insert("allowed_actions".to_owned(), json!(actions));
    result.insert(
        "suggestions".to_owned(),
        Value::Array(current.suggestions(state, &actions)),
    );
    Ok(result)
}

/// Project one self-contained automation observation from a single coherent Store view.
pub fn project_graph_observation_v1(
    view: &GraphWorkspaceViewV2,
) -> Result<Map<String, Value>, GraphViewErrorV2> {
    let state = graph_state(view)?;
    let procedure = rehydrate_snapshot(state)?;
    let current = CurrentProjection::derive(state, &procedure)?;
    let status = project_graph_status_v2(view, GraphStatusTierV2::Standard, None)?;
    let guidance = if current.is_some() || state.trace().lifecycle() == SessionLifecycle::Prepared {
        Value::Object(project_graph_next_v2(view)?)
    } else {
        Value::Null
    };

    let mut result = Map::new();
    result.insert("schema".to_owned(), json!("podway.observation-result/v2"));
    result.insert("status".to_owned(), Value::Object(status));
    result.insert("guidance".to_owned(), guidance);
    result.insert(
        "active_items".to_owned(),
        Value::Array(
            current
                .as_ref()
                .map(CurrentProjection::active_item_descriptors)
                .unwrap_or_default(),
        ),
    );
    let mutation_templates = if state.trace().lifecycle() == SessionLifecycle::Prepared {
        lifecycle_mutation_templates(view, state, true)
    } else if matches!(
        state.trace().lifecycle(),
        SessionLifecycle::Completed | SessionLifecycle::Cancelled
    ) {
        lifecycle_mutation_templates(view, state, false)
    } else {
        current
            .as_ref()
            .map(|current| current.mutation_templates(view, state, &procedure))
            .unwrap_or_default()
    };
    result.insert(
        "mutation_templates".to_owned(),
        Value::Array(mutation_templates),
    );
    Ok(result)
}

fn project_prepared_next_v1(
    view: &GraphWorkspaceViewV2,
    state: &GraphSessionStateV2,
) -> Map<String, Value> {
    json!({
        "schema": "podway.prepared-next-result/v1",
        "procedure_schema": "podway.procedure/v2",
        "procedure_digest": state.snapshot().digest().as_str(),
        "session_id": state.trace().session_id(),
        "session_state": "prepared",
        "revision": 0,
        "goal_tracking": state.snapshot().goal_tracking(),
        "goal_defined": false,
        "trace_length": 0,
        "queue": queue(view),
        "allowed_actions": ["session.begin", "session.reset", "session.start_replace"],
        "suggestions": [
            {"command":"session.begin","argv":["podway","begin"]},
            {"command":"session.reset","argv":["podway","reset"]},
            {"command":"session.start_replace","argv":["podway","start","--replace-eligible"]},
        ],
    })
    .as_object()
    .expect("prepared next is an object")
    .clone()
}

fn lifecycle_mutation_templates(
    view: &GraphWorkspaceViewV2,
    state: &GraphSessionStateV2,
    prepared: bool,
) -> Vec<Value> {
    let template = |command: &str, argv: Vec<&str>, explicit: bool| {
        json!({
            "command": command,
            "argv": argv,
            "preconditions": {
                "workspace_uuid": view.identity().workspace_uuid(),
                "session_id": state.trace().session_id(),
                "session_revision": state.trace().revision(),
            },
            "authority": "optimistic_concurrency_only",
            "idempotency_key_required": true,
            "requires_explicit_authorization": explicit,
        })
    };
    if prepared {
        vec![
            template("session.begin", vec!["podway", "begin"], false),
            template("session.reset", vec!["podway", "reset"], false),
            template(
                "session.start_replace",
                vec!["podway", "start", "--replace-eligible"],
                false,
            ),
        ]
    } else {
        vec![
            template(
                "session.terminal_disposition",
                vec!["podway", "disposition"],
                false,
            ),
            template("session.reset", vec!["podway", "reset"], false),
            template(
                "session.start_replace",
                vec!["podway", "start", "--replace-eligible"],
                false,
            ),
        ]
    }
}

fn graph_state(view: &GraphWorkspaceViewV2) -> Result<&GraphSessionStateV2, GraphViewErrorV2> {
    view.graph_state()
        .ok_or(GraphViewErrorV2::MissingGraphSession)
}

fn rehydrate_snapshot(state: &GraphSessionStateV2) -> Result<ParsedProcedureV2, GraphViewErrorV2> {
    let parsed = parse_procedure_document(
        state.snapshot().canonical_json().as_str().as_bytes(),
        ProcedureDocumentFormat::Json,
    )
    .map_err(|_| GraphViewErrorV2::InvalidSnapshot)?;
    let ParsedProcedure::V2(parsed) = parsed;
    let validated = validate_procedure_v2(parsed).map_err(|_| GraphViewErrorV2::InvalidSnapshot)?;
    if validated.digest() != state.snapshot().digest()
        || validated.canonical_json().as_str() != state.snapshot().canonical_json().as_str()
    {
        return Err(GraphViewErrorV2::InvalidSnapshot);
    }
    Ok(validated.parsed().clone())
}

struct CurrentProjection<'a> {
    state: &'a GraphSessionStateV2,
    attempt: &'a SessionAttemptV2,
    memory: &'a AttemptWorkflowMemoryV2,
    placement: &'a GraphPlacementV2,
    definition: &'a ParsedNodeDefinition,
    item_specs: &'a [ItemSpecV2],
    missing_required: Vec<&'a ItemSpecV2>,
    open_blockers: Vec<&'a podway_store::BlockerStateV2>,
    items_satisfied: bool,
    goal_ready: bool,
    evidence_ready: bool,
}

impl<'a> CurrentProjection<'a> {
    fn derive(
        state: &'a GraphSessionStateV2,
        procedure: &'a ParsedProcedureV2,
    ) -> Result<Option<Self>, GraphViewErrorV2> {
        if state.trace().lifecycle() != SessionLifecycle::Running {
            return Ok(None);
        }
        let attempt = state
            .trace()
            .active_attempt()
            .ok_or(GraphViewErrorV2::InconsistentState(
                "running graph session has no active attempt",
            ))?;
        let memory = state
            .workflow_memory()
            .attempts()
            .iter()
            .find(|memory| memory.attempt_id() == attempt.attempt_id())
            .ok_or(GraphViewErrorV2::InconsistentState(
                "active attempt workflow memory is absent",
            ))?;
        let placement = procedure
            .graph()
            .placement(attempt.graph_node_id())
            .ok_or(GraphViewErrorV2::InvalidSnapshot)?;
        let definition = procedure
            .node_definitions()
            .iter()
            .find(|definition| match (definition, placement) {
                (ParsedNodeDefinition::Action(definition), GraphPlacementV2::Action(placement)) => {
                    definition.id() == placement.definition()
                }
                (
                    ParsedNodeDefinition::Decision(definition),
                    GraphPlacementV2::Decision(placement),
                ) => definition.id() == placement.definition(),
                _ => false,
            })
            .ok_or(GraphViewErrorV2::InvalidSnapshot)?;
        let item_specs = match definition {
            ParsedNodeDefinition::Action(definition) => definition.items(),
            ParsedNodeDefinition::Decision(definition) => definition.items(),
        };
        if item_specs.len() != memory.item_slots().len()
            || item_specs
                .iter()
                .zip(memory.item_slots())
                .any(|(spec, slot)| {
                    spec.id() != slot.item_id() || spec.item_type() != slot.item_type()
                })
        {
            return Err(GraphViewErrorV2::InconsistentState(
                "active item slots disagree with the immutable Procedure snapshot",
            ));
        }
        let missing_required = item_specs
            .iter()
            .zip(memory.item_slots())
            .filter_map(|(spec, slot)| {
                (spec.common().required()
                    && !slot
                        .value()
                        .is_some_and(|value| spec.admits_recorded_value(value)))
                .then_some(spec)
            })
            .collect::<Vec<_>>();
        let items_satisfied = item_specs
            .iter()
            .zip(memory.item_slots())
            .all(|(spec, slot)| {
                !spec.common().required()
                    || slot
                        .value()
                        .is_some_and(|value| spec.admits_recorded_value(value))
            });
        let open_blockers = memory
            .blockers()
            .iter()
            .filter(|blocker| blocker.state() == BlockerState::Open)
            .collect::<Vec<_>>();
        let terminal_action =
            matches!(placement, GraphPlacementV2::Action(action) if action.outcome().is_terminal());
        let goal_ready = match definition {
            ParsedNodeDefinition::Decision(definition) if definition.assessment().is_some() => {
                determined_goal_outcome(state, attempt).is_some()
            }
            _ => {
                !terminal_action
                    || !state.snapshot().goal_tracking()
                    || state
                        .goal_state()
                        .latest_fresh_assessment(state.trace())
                        .is_some()
            }
        };
        let evidence_ready = match placement {
            GraphPlacementV2::Action(_) => true,
            GraphPlacementV2::Decision(_) => state
                .selected_evidence_readback(attempt.attempt_id())
                .map_err(|_| {
                    GraphViewErrorV2::InconsistentState("selected evidence readback is invalid")
                })?
                .iter()
                .all(|readback| {
                    !readback.stale()
                        && (!readback.reference().required()
                            || matches!(
                                readback.reference().resolution(),
                                ResolvedEvidenceReferenceV2::Resolved(_)
                            ))
                }),
        };
        Ok(Some(Self {
            state,
            attempt,
            memory,
            placement,
            definition,
            item_specs,
            missing_required,
            open_blockers,
            items_satisfied,
            goal_ready,
            evidence_ready,
        }))
    }

    fn identity(&self) -> Value {
        json!({
            "node": self.node_identity(),
            "attempt": self.attempt_identity(),
            "readiness": self.readiness_value(),
            "missing_required_item_count": self.missing_required.len(),
            "blockers_total": self.open_blockers.len(),
        })
    }

    fn node_identity(&self) -> Value {
        let definition = match self.definition {
            ParsedNodeDefinition::Action(definition) => definition.id(),
            ParsedNodeDefinition::Decision(definition) => definition.id(),
        };
        json!({
            "node_definition_id": definition.as_str(),
            "graph_node_id": self.attempt.graph_node_id().as_str(),
            "node_type": self.placement.node_kind().as_str(),
        })
    }

    fn attempt_identity(&self) -> Value {
        json!({
            "attempt_id": self.attempt.attempt_id().as_str(),
            "attempt_number": self.attempt.number().get(),
        })
    }

    fn can_advance(&self) -> bool {
        self.items_satisfied
            && self.open_blockers.is_empty()
            && self.goal_ready
            && self.evidence_ready
    }

    fn readiness_value(&self) -> Value {
        json!({
            "items_satisfied": self.items_satisfied,
            "unblocked": self.open_blockers.is_empty(),
            "goal_ready": self.goal_ready,
            "can_advance": self.can_advance(),
        })
    }

    fn compact_items(&self) -> Vec<Value> {
        self.item_specs
            .iter()
            .zip(self.memory.item_slots())
            .map(|(spec, slot)| {
                json!({
                    "item_id": spec.id().as_str(),
                    "type": item_type(spec.item_type()),
                    "required": spec.common().required(),
                    "satisfied": slot.value().is_some_and(|value| spec.admits_recorded_value(value)),
                    "revision": slot.revision().get(),
                })
            })
            .collect()
    }

    fn missing_ids(&self) -> Vec<Value> {
        self.missing_required
            .iter()
            .map(|item| json!(item.id().as_str()))
            .collect()
    }

    fn missing_items(&self) -> Vec<Value> {
        self.missing_required
            .iter()
            .map(|item| {
                json!({
                    "item_id": item.id().as_str(),
                    "prompt": item.common().prompt(),
                })
            })
            .collect()
    }

    fn item_values(&self) -> (Vec<Value>, usize, bool) {
        let all = self
            .memory
            .item_slots()
            .iter()
            .filter_map(|slot| {
                slot.value().map(|value| {
                    let display = display_item_value(value);
                    let (display, value_truncated) =
                        truncate_chars(&display, ITEM_DISPLAY_CHARS_MAX);
                    json!({
                        "item_id": slot.item_id().as_str(),
                        "value": display,
                        "value_truncated": value_truncated,
                    })
                })
            })
            .collect::<Vec<_>>();
        let total = all.len();
        let mut used = 2_usize;
        let mut values = Vec::new();
        for value in all {
            let bytes = serde_json::to_vec(&value)
                .map_or(STATUS_VALUES_BYTES_MAX, |json| json.len())
                + usize::from(!values.is_empty());
            if used.saturating_add(bytes) > STATUS_VALUES_BYTES_MAX {
                break;
            }
            used += bytes;
            values.push(value);
        }
        let truncated = values.len() < total;
        (values, total, truncated)
    }

    fn active_item_descriptors(&self) -> Vec<Value> {
        self.item_specs
            .iter()
            .zip(self.memory.item_slots())
            .map(|(spec, slot)| {
                let mut descriptor = Map::new();
                descriptor.insert("item_id".to_owned(), json!(spec.id().as_str()));
                descriptor.insert("type".to_owned(), json!(item_type(spec.item_type())));
                descriptor.insert("prompt".to_owned(), json!(spec.common().prompt()));
                if let Some(help) = spec.common().help() {
                    descriptor.insert("help".to_owned(), json!(help));
                }
                descriptor.insert("required".to_owned(), json!(spec.common().required()));
                descriptor.insert(
                    "satisfied".to_owned(),
                    json!(
                        slot.value()
                            .is_some_and(|value| spec.admits_recorded_value(value))
                    ),
                );
                descriptor.insert("revision".to_owned(), json!(slot.revision().get()));
                descriptor.insert("constraints".to_owned(), item_constraints(spec));
                let (value, value_truncated) = slot
                    .value()
                    .map(project_observation_item_value)
                    .unwrap_or((Value::Null, false));
                descriptor.insert("value".to_owned(), value);
                descriptor.insert("value_truncated".to_owned(), json!(value_truncated));
                Value::Object(descriptor)
            })
            .collect()
    }

    fn mutation_templates(
        &self,
        view: &GraphWorkspaceViewV2,
        state: &GraphSessionStateV2,
        procedure: &ParsedProcedureV2,
    ) -> Vec<Value> {
        let actions = self.allowed_actions(state, procedure);
        let mut recipes = self.suggestions(state, &actions);
        for (spec, slot) in self.item_specs.iter().zip(self.memory.item_slots()) {
            let item_id = spec.id().as_str();
            let set_placeholder = match spec.item_type() {
                ItemTypeV1::Text => Some(("item.set", "set", "<text>")),
                ItemTypeV1::Choice => Some(("item.set", "set", "<choice>")),
                ItemTypeV1::Integer => Some(("item.set", "set", "<integer>")),
                ItemTypeV1::Artifact => Some(("item.attach", "attach", "<path>")),
                ItemTypeV1::Confirm | ItemTypeV1::List => None,
            };
            if let Some((command, verb, placeholder)) = set_placeholder {
                push_recipe_unique(
                    &mut recipes,
                    json!({
                        "command": command,
                        "argv": ["podway", verb, item_id, placeholder],
                        "item_id": item_id,
                    }),
                );
            }
            if spec.item_type() == ItemTypeV1::Confirm {
                push_recipe_unique(
                    &mut recipes,
                    json!({"command":"item.check","argv":["podway","check",item_id],"item_id":item_id}),
                );
            }
            if let ItemSpecV2::List(list) = spec {
                let count = slot
                    .value()
                    .and_then(RecordedItemValueV2::as_list)
                    .map_or(0, <[String]>::len);
                if count < usize::from(list.max_items()) {
                    push_recipe_unique(
                        &mut recipes,
                        json!({"command":"item.add","argv":["podway","add",item_id,"<value>"],"item_id":item_id}),
                    );
                }
                if count != 0 {
                    push_recipe_unique(
                        &mut recipes,
                        json!({"command":"item.remove","argv":["podway","remove",item_id,"<value>"],"item_id":item_id}),
                    );
                }
            }
            if slot.value().is_some() {
                if spec.item_type() == ItemTypeV1::Confirm {
                    push_recipe_unique(
                        &mut recipes,
                        json!({"command":"item.uncheck","argv":["podway","uncheck",item_id],"item_id":item_id}),
                    );
                }
                push_recipe_unique(
                    &mut recipes,
                    json!({"command":"item.clear","argv":["podway","clear",item_id],"item_id":item_id}),
                );
            }
        }
        if actions.contains(&"session.block") {
            push_recipe_unique(
                &mut recipes,
                json!({"command":"session.block","argv":["podway","block","--reason","<reason>"]}),
            );
        }
        if actions.contains(&"session.unblock") {
            push_recipe_unique(
                &mut recipes,
                json!({"command":"session.unblock","argv":["podway","unblock","<blocker-id>"]}),
            );
        }
        push_recipe_unique(
            &mut recipes,
            json!({"command":"session.cancel","argv":["podway","cancel","--reason","<reason>"]}),
        );
        push_recipe_unique(
            &mut recipes,
            json!({"command":"session.reset","argv":["podway","reset","--yes"]}),
        );
        if actions.contains(&"session.rework") {
            push_recipe_unique(
                &mut recipes,
                json!({"command":"session.rework","argv":["podway","rework","--to","<graph-node-id>","--reason","<reason>"]}),
            );
        }
        if actions.contains(&"goal.revise") {
            push_recipe_unique(
                &mut recipes,
                json!({
                    "command":"goal.revise",
                    "argv":["podway","goal","revise","--goal","<goal>","--criterion","<criterion>","--rework-to","<graph-node-id>","--reason","<reason>"]
                }),
            );
        }
        recipes
            .into_iter()
            .filter_map(|suggestion| {
                let object = suggestion.as_object()?;
                let command = object.get("command")?.as_str()?;
                let mut argv = object.get("argv")?.as_array()?.clone();
                append_flag(
                    &mut argv,
                    "--if-workspace-uuid",
                    view.identity().workspace_uuid().as_str(),
                );
                append_flag(
                    &mut argv,
                    "--if-session-id",
                    state.trace().session_id().as_str(),
                );

                let mut fences = Map::new();
                fences.insert(
                    "workspace_uuid".to_owned(),
                    json!(view.identity().workspace_uuid().as_str()),
                );
                fences.insert(
                    "session_id".to_owned(),
                    json!(state.trace().session_id().as_str()),
                );
                if command.starts_with("item.") {
                    let item_id = object.get("item_id")?.as_str()?;
                    let slot = self
                        .memory
                        .item_slots()
                        .iter()
                        .find(|slot| slot.item_id().as_str() == item_id)?;
                    append_flag(
                        &mut argv,
                        "--if-attempt",
                        self.attempt.attempt_id().as_str(),
                    );
                    append_flag(
                        &mut argv,
                        "--if-item-revision",
                        &slot.revision().get().to_string(),
                    );
                    fences.insert(
                        "attempt_id".to_owned(),
                        json!(self.attempt.attempt_id().as_str()),
                    );
                    fences.insert("item_revision".to_owned(), json!(slot.revision().get()));
                } else {
                    append_flag(
                        &mut argv,
                        "--if-session-revision",
                        &state.trace().revision().get().to_string(),
                    );
                    fences.insert(
                        "session_revision".to_owned(),
                        json!(state.trace().revision().get()),
                    );
                    if matches!(
                        command,
                        "session.complete"
                            | "session.decide"
                            | "session.retry"
                            | "session.skip"
                            | "session.rework"
                            | "session.block"
                            | "session.unblock"
                            | "session.cancel"
                            | "goal.revise"
                            | "goal.assess_criterion"
                    ) {
                        append_flag(
                            &mut argv,
                            "--if-attempt",
                            self.attempt.attempt_id().as_str(),
                        );
                        fences.insert(
                            "attempt_id".to_owned(),
                            json!(self.attempt.attempt_id().as_str()),
                        );
                    }
                    if matches!(
                        command,
                        "session.decide" | "goal.revise" | "goal.assess_criterion"
                    ) && let Some(revision) = state.goal_state().current_revision()
                    {
                        append_flag(&mut argv, "--if-goal-revision", &revision.get().to_string());
                        fences.insert("goal_revision".to_owned(), json!(revision.get()));
                    }
                }
                append_flag(&mut argv, "--idempotency-key", "<idempotency-key>");
                Some(json!({
                    "command": command,
                    "argv": argv,
                    "preconditions": fences,
                    "authority": "optimistic_concurrency_only",
                    "idempotency_key_required": true,
                    "requires_explicit_authorization": matches!(
                        command,
                        "goal.define" | "goal.revise" | "session.cancel" | "session.reset"
                    ),
                }))
            })
            .collect()
    }

    fn blocker_window(&self) -> Result<(Vec<Value>, bool), GraphViewErrorV2> {
        let mut blockers = self.open_blockers.clone();
        blockers.sort_by_key(|blocker| std::cmp::Reverse(blocker.created_at()));
        let mut used = 2_usize;
        let mut projected = Vec::new();
        for blocker in blockers {
            let value = json!({
                "blocker_id": blocker.blocker_id().as_str(),
                "reason": blocker.reason(),
                "created_at": timestamp(blocker.created_at())?,
            });
            let bytes = serde_json::to_vec(&value)
                .map_err(|_| GraphViewErrorV2::InconsistentState("blocker view is not JSON"))?
                .len()
                + usize::from(!projected.is_empty());
            if used.saturating_add(bytes) > BLOCKER_WINDOW_BYTES_MAX {
                break;
            }
            used += bytes;
            projected.push(value);
        }
        let truncated = projected.len() < self.open_blockers.len();
        Ok((projected, truncated))
    }

    fn reference_metadata(
        &self,
        procedure: &ParsedProcedureV2,
    ) -> Result<Vec<Value>, GraphViewErrorV2> {
        self.memory
            .evidence()
            .iter()
            .map(|reference| reference_metadata(reference, procedure, false))
            .collect()
    }

    fn allowed_option_ids(&self) -> Vec<Value> {
        if !self.can_advance() {
            return Vec::new();
        }
        match self.definition {
            ParsedNodeDefinition::Decision(definition) => {
                if let Some(assessment) = definition.assessment() {
                    let Some(outcome) = determined_goal_outcome_for_current(self) else {
                        return Vec::new();
                    };
                    definition
                        .options()
                        .iter()
                        .filter(|option| {
                            assessment.outcomes().iter().any(|mapping| {
                                mapping.option_id() == option.id() && mapping.outcome() == outcome
                            })
                        })
                        .map(|option| json!(option.id().as_str()))
                        .collect()
                } else {
                    definition
                        .options()
                        .iter()
                        .map(|option| json!(option.id().as_str()))
                        .collect()
                }
            }
            ParsedNodeDefinition::Action(_) => Vec::new(),
        }
    }

    fn allowed_actions(
        &self,
        state: &GraphSessionStateV2,
        procedure: &ParsedProcedureV2,
    ) -> Vec<&'static str> {
        let mut actions = Vec::new();
        for (spec, slot) in self.item_specs.iter().zip(self.memory.item_slots()) {
            match spec {
                ItemSpecV2::Confirm(_) => push_unique(&mut actions, "item.check"),
                ItemSpecV2::Text(_) | ItemSpecV2::Choice(_) | ItemSpecV2::Integer(_) => {
                    push_unique(&mut actions, "item.set");
                }
                ItemSpecV2::List(list) => {
                    let item_count = slot
                        .value()
                        .and_then(RecordedItemValueV2::as_list)
                        .map_or(0, <[String]>::len);
                    if item_count < usize::from(list.max_items()) {
                        push_unique(&mut actions, "item.add");
                    }
                    if item_count != 0 {
                        push_unique(&mut actions, "item.remove");
                    }
                }
                ItemSpecV2::Artifact(_) => push_unique(&mut actions, "item.attach"),
            }
            if slot.value().is_some() {
                if spec.item_type() == ItemTypeV1::Confirm {
                    push_unique(&mut actions, "item.uncheck");
                }
                push_unique(&mut actions, "item.clear");
            }
        }
        if self.can_advance() {
            match self.placement {
                GraphPlacementV2::Action(_) => push_unique(&mut actions, "session.complete"),
                GraphPlacementV2::Decision(_) => push_unique(&mut actions, "session.decide"),
            }
        }
        if matches!(self.placement, GraphPlacementV2::Action(action) if action.skip().is_some() && (!action.outcome().is_terminal() || self.goal_ready))
        {
            push_unique(&mut actions, "session.skip");
        }
        push_unique(&mut actions, "session.retry");
        if self.open_blockers.len() < 64 {
            push_unique(&mut actions, "session.block");
        }
        if !self.open_blockers.is_empty() {
            push_unique(&mut actions, "session.unblock");
        }
        push_unique(&mut actions, "session.cancel");
        push_unique(&mut actions, "session.reset");
        if manual_rework_targets(state, procedure)
            .as_array()
            .is_some_and(|targets| !targets.is_empty())
        {
            push_unique(&mut actions, "session.rework");
        }
        if state.snapshot().goal_tracking() {
            if state.goal_state().current_revision().is_some() {
                if goal_revision_rework_targets(state, procedure)
                    .as_array()
                    .is_some_and(|targets| !targets.is_empty())
                {
                    push_unique(&mut actions, "goal.revise");
                }
            } else {
                push_unique(&mut actions, "goal.define");
            }
            if matches!(self.definition, ParsedNodeDefinition::Decision(definition) if definition.assessment().is_some())
                && unassessed_goal_criteria(state, self.attempt)
                    .is_some_and(|criteria| !criteria.is_empty())
            {
                push_unique(&mut actions, "goal.assess_criterion");
            }
        }
        actions
    }

    fn suggestions(&self, state: &GraphSessionStateV2, actions: &[&str]) -> Vec<Value> {
        let mut suggestions = Vec::new();
        for item in &self.missing_required {
            let (command, verb, placeholder) = match item.item_type() {
                ItemTypeV1::Confirm => ("item.check", "check", None),
                ItemTypeV1::Text => ("item.set", "set", Some("<text>")),
                ItemTypeV1::Choice => ("item.set", "set", Some("<choice>")),
                ItemTypeV1::Integer => ("item.set", "set", Some("<integer>")),
                ItemTypeV1::List => ("item.add", "add", Some("<value>")),
                ItemTypeV1::Artifact => ("item.attach", "attach", Some("<path>")),
            };
            let mut argv = vec![json!("podway"), json!(verb), json!(item.id().as_str())];
            if let Some(placeholder) = placeholder {
                argv.push(json!(placeholder));
            }
            suggestions.push(json!({
                "command": command,
                "argv": argv,
                "item_id": item.id().as_str(),
            }));
        }
        if actions.contains(&"session.complete") {
            suggestions
                .push(json!({"command": "session.complete", "argv": ["podway", "complete"]}));
        }
        if actions.contains(&"session.decide")
            && let ParsedNodeDefinition::Decision(definition) = self.definition
        {
            let allowed_options = self.allowed_option_ids();
            for option in definition
                .options()
                .iter()
                .filter(|option| allowed_options.contains(&json!(option.id().as_str())))
            {
                suggestions.push(json!({
                    "command": "session.decide",
                    "argv": ["podway", "decide", "--option", option.id().as_str(), "--reason", "<reason>"],
                }));
            }
        }
        suggestions.push(json!({
            "command": "session.retry",
            "argv": ["podway", "retry", "--reason", "<reason>"],
        }));
        if actions.contains(&"session.skip") {
            let reason_required = matches!(self.placement, GraphPlacementV2::Action(action) if action.skip().is_some_and(|skip| skip.reason_required()));
            suggestions.push(if reason_required {
                json!({"command": "session.skip", "argv": ["podway", "skip", "--reason", "<text>"]})
            } else {
                json!({"command": "session.skip", "argv": ["podway", "skip"]})
            });
        }
        if actions.contains(&"goal.define") {
            suggestions.push(json!({
                "command": "goal.define",
                "argv": ["podway", "goal", "define", "--goal", "<goal>", "--criterion", "<criterion>"],
            }));
        }
        if actions.contains(&"goal.assess_criterion")
            && let Some(criteria) = unassessed_goal_criteria(state, self.attempt)
        {
            for criterion in criteria {
                suggestions.push(json!({
                    "command": "goal.assess_criterion",
                    "argv": ["podway", "goal", "assess-criterion", criterion, "--status", "<status>", "--reason", "<reason>"],
                }));
            }
        }
        suggestions.truncate(128);
        suggestions
    }
}

fn append_flag(argv: &mut Vec<Value>, flag: &str, value: &str) {
    argv.push(json!(flag));
    argv.push(json!(value));
}

fn push_recipe_unique(recipes: &mut Vec<Value>, recipe: Value) {
    if !recipes.contains(&recipe) {
        recipes.push(recipe);
    }
}

fn item_constraints(spec: &ItemSpecV2) -> Value {
    match spec {
        ItemSpecV2::Confirm(_) => json!({}),
        ItemSpecV2::Text(spec) => json!({
            "min_length": spec.min_length(),
            "max_length": spec.max_length(),
            "multiline": spec.multiline(),
        }),
        ItemSpecV2::Choice(spec) => json!({"choices": spec.choices()}),
        ItemSpecV2::Integer(spec) => {
            let mut constraints = Map::new();
            if let Some(minimum) = spec.minimum() {
                constraints.insert("minimum".to_owned(), json!(minimum));
            }
            if let Some(maximum) = spec.maximum() {
                constraints.insert("maximum".to_owned(), json!(maximum));
            }
            Value::Object(constraints)
        }
        ItemSpecV2::List(spec) => json!({
            "min_items": spec.min_items(),
            "max_items": spec.max_items(),
            "max_item_length": spec.max_item_length(),
            "unique": spec.unique(),
        }),
        ItemSpecV2::Artifact(spec) => {
            json!({"allowed_media_types": spec.allowed_media_types()})
        }
    }
}

fn project_observation_item_value(value: &RecordedItemValueV2) -> (Value, bool) {
    if value.item_type() == ItemTypeV1::Confirm {
        return (Value::Bool(true), false);
    }
    if let Some(value) = value.as_text() {
        let (value, truncated) = truncate_chars(value, OBSERVATION_ITEM_VALUE_CHARS_MAX);
        return (json!(value), truncated);
    }
    if let Some(value) = value.as_choice() {
        return (json!(value), false);
    }
    if let Some(value) = value.as_integer() {
        return (json!(value), false);
    }
    if let Some(value) = value.as_list() {
        let mut projected = Vec::new();
        let mut truncated = false;
        for entry in value {
            let (entry, entry_truncated) = truncate_chars(entry, OBSERVATION_ITEM_VALUE_CHARS_MAX);
            let mut candidate = projected.clone();
            candidate.push(entry);
            if serde_json::to_vec(&candidate).map_or(true, |encoded| {
                encoded.len() > OBSERVATION_ITEM_VALUE_BYTES_MAX
            }) {
                truncated = true;
                break;
            }
            projected = candidate;
            truncated |= entry_truncated;
        }
        truncated |= projected.len() < value.len();
        return (json!(projected), truncated);
    }
    let artifact = value
        .as_artifact()
        .expect("the item type guarantees an artifact value");
    let (location, location_truncated) =
        truncate_chars(artifact.location(), OBSERVATION_ITEM_VALUE_CHARS_MAX);
    (
        json!({
            "location_type": match artifact.location_kind() {
                ArtifactLocationKindV1::LocalPath => "path",
                ArtifactLocationKindV1::ExternalReference => "reference",
            },
            "location": location,
            "sha256_digest": artifact.digest().as_str(),
            "size_bytes": artifact.size_bytes(),
            "media_type": artifact.media_type(),
        }),
        location_truncated,
    )
}

fn procedure_identity(state: &GraphSessionStateV2) -> Value {
    json!({
        "schema": "podway.procedure/v2",
        "id": state.snapshot().procedure_id(),
        "version": state.snapshot().procedure_version(),
        "digest": state.snapshot().digest().as_str(),
    })
}

fn session_identity(state: &GraphSessionStateV2) -> Value {
    json!({
        "id": state.trace().session_id().as_str(),
        "lifecycle": lifecycle(state.trace().lifecycle()),
        "revision": state.trace().revision().get(),
    })
}

fn counters(state: &GraphSessionStateV2) -> Value {
    if state.trace().lifecycle() == SessionLifecycle::Prepared {
        return Value::Array(Vec::new());
    }
    Value::Array(
        state
            .counters()
            .iter()
            .map(|counter| {
                json!({
                    "graph_node_id": counter.graph_node_id().as_str(),
                    "attempt_count": counter.attempt_count(),
                    "rework_traversal_count": counter.rework_traversal_count(),
                })
            })
            .collect(),
    )
}

fn queue(view: &GraphWorkspaceViewV2) -> Value {
    json!({
        "pending_mutations": view.queued_job_count() != 0 || view.running_job_id().is_some(),
        "queued_count": view.queued_job_count(),
        "running_job_id": view.running_job_id().map(|job| job.as_str()),
        "latest_workspace_sequence": view.latest_workspace_sequence(),
    })
}

fn add_static_disposition(
    result: &mut Map<String, Value>,
    current: Option<&CurrentProjection<'_>>,
) {
    let Some(current) = current else {
        return;
    };
    if let GraphPlacementV2::Action(action) = current.placement {
        if let Some(target) = action.outcome().next_target() {
            result.insert("next_graph_node_id".to_owned(), json!(target.as_str()));
        } else {
            result.insert("terminal".to_owned(), Value::Bool(true));
        }
        if let Some(skip) = action.skip() {
            result.insert(
                "skip".to_owned(),
                json!({"allowed": true, "reason_required": skip.reason_required()}),
            );
        }
    }
}

fn manual_rework_targets(state: &GraphSessionStateV2, procedure: &ParsedProcedureV2) -> Value {
    let valid_nodes = state
        .trace()
        .attempts()
        .iter()
        .filter(|attempt| attempt.validity() == AttemptValidityV2::Valid)
        .map(|attempt| attempt.graph_node_id())
        .collect::<Vec<_>>();
    Value::Array(
        procedure
            .graph()
            .manual_rework()
            .map(|manual| {
                manual
                    .targets()
                    .iter()
                    .filter(|target| valid_nodes.contains(target))
                    .map(|target| json!(target.as_str()))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn goal_revision_rework_targets(
    state: &GraphSessionStateV2,
    procedure: &ParsedProcedureV2,
) -> Value {
    let safe = goal_revision_safe_targets_v2(procedure);
    let valid_nodes = state
        .trace()
        .attempts()
        .iter()
        .filter(|attempt| attempt.validity() == AttemptValidityV2::Valid)
        .map(|attempt| attempt.graph_node_id())
        .collect::<Vec<_>>();
    Value::Array(
        procedure
            .graph()
            .manual_rework()
            .map(|manual| {
                manual
                    .targets()
                    .iter()
                    .filter(|target| valid_nodes.contains(target) && safe.contains(target))
                    .map(|target| json!(target.as_str()))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn add_goal_summary(result: &mut Map<String, Value>, state: &GraphSessionStateV2) {
    if let Some(revision) = state.goal_state().current_revision() {
        result.insert("goal_revision".to_owned(), json!(revision.get()));
        if let Some(assessment) = state.goal_state().latest_fresh_assessment(state.trace()) {
            result.insert(
                "latest_goal_outcome".to_owned(),
                json!(assessment.outcome().as_str()),
            );
        }
    }
}

fn goal_display(
    state: &GraphSessionStateV2,
    current: Option<&CurrentProjection<'_>>,
) -> Option<Value> {
    let revision_number = state.goal_state().current_revision()?;
    let revision = state
        .goal_state()
        .revisions()
        .iter()
        .find(|record| record.revision() == revision_number)?;
    let attempt_results = current.and_then(|current| {
        state
            .goal_state()
            .attempt_assessments()
            .iter()
            .find(|assessment| {
                assessment.attempt_id() == current.attempt.attempt_id()
                    && assessment.goal_revision() == revision_number
            })
    });
    let latest_assessment = state
        .goal_state()
        .latest_fresh_assessment(state.trace())
        .filter(|assessment| assessment.goal_revision() == revision_number);
    let current_is_goal_assessment = current.is_some_and(|current| {
        matches!(
            current.definition,
            ParsedNodeDefinition::Decision(definition) if definition.assessment().is_some()
        )
    });
    let displayed_historical_assessment = (!current_is_goal_assessment)
        .then_some(latest_assessment)
        .flatten();
    let criteria = revision
        .criteria()
        .criteria()
        .iter()
        .map(|criterion| {
            let status = attempt_results
                .and_then(|assessment| {
                    assessment
                        .results()
                        .iter()
                        .find(|result| result.result().criterion_id() == criterion.id())
                })
                .map(|result| result.result().status().as_str())
                .or_else(|| {
                    displayed_historical_assessment.and_then(|assessment| {
                        assessment
                            .criterion_results()
                            .iter()
                            .find(|result| result.criterion_id() == criterion.id())
                            .map(|result| result.status().as_str())
                    })
                })
                .unwrap_or("unassessed");
            json!({
                "criterion_id": criterion.id().as_str(),
                "statement": criterion.statement(),
                "status": status,
            })
        })
        .collect::<Vec<_>>();
    let mut goal = Map::new();
    goal.insert("revision".to_owned(), json!(revision_number.get()));
    goal.insert("statement".to_owned(), json!(revision.statement().as_str()));
    goal.insert("criteria".to_owned(), Value::Array(criteria));
    if let Some(results) = attempt_results
        && let Some(first) = results.results().first()
    {
        goal.insert(
            "assessment_mode".to_owned(),
            json!(first.result().mode().as_str()),
        );
    } else if let Some(assessment) = displayed_historical_assessment {
        goal.insert(
            "assessment_mode".to_owned(),
            json!(assessment.mode().as_str()),
        );
    }
    let determined = current
        .and_then(|current| determined_goal_outcome(state, current.attempt))
        .or_else(|| {
            (!current_is_goal_assessment).then_some(())?;
            state
                .goal_state()
                .latest_fresh_assessment(state.trace())
                .map(|assessment| assessment.outcome())
        });
    if let Some(outcome) = determined {
        goal.insert("determined_outcome".to_owned(), json!(outcome.as_str()));
    }
    Some(Value::Object(goal))
}

fn determined_goal_outcome_for_current(current: &CurrentProjection<'_>) -> Option<GoalOutcome> {
    determined_goal_outcome(current.state, current.attempt)
}

fn determined_goal_outcome(
    state: &GraphSessionStateV2,
    attempt: &SessionAttemptV2,
) -> Option<GoalOutcome> {
    let revision = state.goal_state().current_revision()?;
    if attempt.goal_revision() != Some(revision) {
        return None;
    }
    let goal = state
        .goal_state()
        .revisions()
        .iter()
        .find(|record| record.revision() == revision)?;
    let assessment = state
        .goal_state()
        .attempt_assessments()
        .iter()
        .find(|assessment| {
            assessment.attempt_id() == attempt.attempt_id()
                && assessment.goal_revision() == revision
        })?;
    if assessment.results().len() != goal.criteria().criteria().len()
        || goal.criteria().criteria().iter().any(|criterion| {
            !assessment
                .results()
                .iter()
                .any(|state| state.result().criterion_id() == criterion.id())
        })
    {
        return None;
    }
    if assessment
        .results()
        .iter()
        .all(|state| state.result().status() == CriterionStatusV2::NotApplicable)
    {
        Some(GoalOutcome::Superseded)
    } else if assessment
        .results()
        .iter()
        .all(|state| state.result().status() == CriterionStatusV2::Satisfied)
    {
        Some(GoalOutcome::Achieved)
    } else if assessment
        .results()
        .iter()
        .any(|state| state.result().status() == CriterionStatusV2::Unsatisfied)
    {
        Some(GoalOutcome::NotAchieved)
    } else {
        None
    }
}

fn unassessed_goal_criteria<'a>(
    state: &'a GraphSessionStateV2,
    attempt: &SessionAttemptV2,
) -> Option<Vec<&'a str>> {
    let revision = state.goal_state().current_revision()?;
    if attempt.goal_revision() != Some(revision) {
        return None;
    }
    let goal = state
        .goal_state()
        .revisions()
        .iter()
        .find(|record| record.revision() == revision)?;
    let results = state
        .goal_state()
        .attempt_assessments()
        .iter()
        .find(|assessment| {
            assessment.attempt_id() == attempt.attempt_id()
                && assessment.goal_revision() == revision
        });
    Some(
        goal.criteria()
            .criteria()
            .iter()
            .filter(|criterion| {
                results.is_none_or(|assessment| {
                    !assessment
                        .results()
                        .iter()
                        .any(|state| state.result().criterion_id() == criterion.id())
                })
            })
            .map(|criterion| criterion.id().as_str())
            .collect(),
    )
}

fn reference_metadata(
    reference: &EvidenceResolutionStateV2,
    procedure: &ParsedProcedureV2,
    stale: bool,
) -> Result<Value, GraphViewErrorV2> {
    let resolution = reference.resolution();
    let source = resolution.source_node();
    let mut value = Map::new();
    value.insert("source_graph_node_id".to_owned(), json!(source.as_str()));
    if let Some(title) = definition_title_for_node(procedure, source.as_str()) {
        value.insert("source_title".to_owned(), json!(title));
    }
    let state = if stale {
        "stale"
    } else {
        match resolution {
            ResolvedEvidenceReferenceV2::Resolved(_) => "resolved",
            ResolvedEvidenceReferenceV2::Skipped(_) => "skipped",
            ResolvedEvidenceReferenceV2::Unresolved { .. } => "unresolved",
        }
    };
    value.insert("state".to_owned(), json!(state));
    if let Some(snapshot) = resolution.snapshot() {
        value.insert(
            "source_attempt_id".to_owned(),
            json!(snapshot.source_attempt_id().as_str()),
        );
        value.insert(
            "source_attempt_number".to_owned(),
            json!(snapshot.source_attempt_number().get()),
        );
        value.insert(
            "items_digest".to_owned(),
            json!(snapshot.items_digest().as_str()),
        );
    } else if stale {
        return Err(GraphViewErrorV2::InconsistentState(
            "a stale reference must retain resolved snapshot metadata",
        ));
    }
    Ok(Value::Object(value))
}

fn readback(
    state: &GraphSessionStateV2,
    current: &CurrentProjection<'_>,
    procedure: &ParsedProcedureV2,
) -> Result<Vec<Value>, GraphViewErrorV2> {
    state
        .selected_evidence_readback(current.attempt.attempt_id())
        .map_err(|_| GraphViewErrorV2::InconsistentState("selected evidence readback is invalid"))?
        .iter()
        .map(|readback| readback_value(state, readback, procedure))
        .collect()
}

fn readback_value(
    state: &GraphSessionStateV2,
    readback: &EvidenceReadbackV2,
    procedure: &ParsedProcedureV2,
) -> Result<Value, GraphViewErrorV2> {
    let resolution = readback.reference().resolution();
    let source = resolution.source_node();
    let source_title = definition_title_for_node(procedure, source.as_str())
        .ok_or(GraphViewErrorV2::InvalidSnapshot)?;
    let reference_state = match resolution {
        ResolvedEvidenceReferenceV2::Resolved(_) => "resolved",
        ResolvedEvidenceReferenceV2::Skipped(_) => "skipped",
        ResolvedEvidenceReferenceV2::Unresolved { .. } => "unresolved",
    };
    let mut value = Map::new();
    value.insert("source_graph_node_id".to_owned(), json!(source.as_str()));
    value.insert("source_title".to_owned(), json!(source_title));
    value.insert("state".to_owned(), json!(reference_state));
    value.insert(
        "items".to_owned(),
        Value::Array(
            readback
                .items()
                .items()
                .iter()
                .map(|item| {
                    json!({
                        "item_id": item.id().as_str(),
                        "type": item_type(item.value().item_type()),
                        "value": typed_item_value(item.value()),
                    })
                })
                .collect(),
        ),
    );
    if let Some(snapshot) = resolution.snapshot() {
        value.insert(
            "source_attempt_id".to_owned(),
            json!(snapshot.source_attempt_id().as_str()),
        );
        value.insert(
            "source_attempt_number".to_owned(),
            json!(snapshot.source_attempt_number().get()),
        );
        value.insert(
            "items_digest".to_owned(),
            json!(snapshot.items_digest().as_str()),
        );
    }
    if let Some(decision) = readback.decision() {
        value.insert(
            "decision_record".to_owned(),
            decision_readback_projection(state, decision)?,
        );
    }
    Ok(Value::Object(value))
}

fn definition_title_for_node<'a>(
    procedure: &'a ParsedProcedureV2,
    node_id: &str,
) -> Option<&'a str> {
    let placement = procedure
        .graph()
        .placements()
        .iter()
        .find(|placement| placement.id().as_str() == node_id)?;
    let definition_id = match placement {
        GraphPlacementV2::Action(placement) => placement.definition(),
        GraphPlacementV2::Decision(placement) => placement.definition(),
    };
    procedure
        .node_definitions()
        .iter()
        .find_map(|definition| match definition {
            ParsedNodeDefinition::Action(value) if value.id() == definition_id => {
                Some(value.title())
            }
            ParsedNodeDefinition::Decision(value) if value.id() == definition_id => {
                Some(value.title())
            }
            _ => None,
        })
}

fn add_histories(
    result: &mut Map<String, Value>,
    state: &GraphSessionStateV2,
    procedure: &ParsedProcedureV2,
    cursor: Option<TraceSequenceV2>,
) -> Result<(), GraphViewErrorV2> {
    let trace = state.trace().attempts();
    let entries = trace
        .iter()
        .rev()
        .filter(|attempt| attempt.validity() == AttemptValidityV2::Valid)
        .filter(|attempt| before_cursor(attempt.trace(), cursor))
        .map(|attempt| trace_entry(state, procedure, attempt));
    result.insert(
        "current_trace_history".to_owned(),
        history_window(entries, 32)?,
    );

    let entries = trace
        .iter()
        .rev()
        .filter(|attempt| attempt.validity() == AttemptValidityV2::Stale)
        .filter(|attempt| before_cursor(attempt.trace(), cursor))
        .map(|attempt| stale_attempt_value(state, procedure, attempt));
    result.insert(
        "stale_attempt_history".to_owned(),
        history_window(entries, 1)?,
    );

    let entries = state
        .workflow_memory()
        .decisions()
        .iter()
        .rev()
        .filter(|record| before_cursor(record.trace(), cursor))
        .map(decision_value);
    result.insert("decision_history".to_owned(), history_window(entries, 1)?);

    let entries = state
        .workflow_memory()
        .reworks()
        .iter()
        .rev()
        .filter(|record| before_cursor(record.trace(), cursor))
        .map(|record| {
            let mut value = json!({
                "trace_sequence": record.trace().get(),
                "kind": record.kind().as_str(),
                "from_graph_node_id": record.from_node().as_str(),
                "to_graph_node_id": record.to_node().as_str(),
                "target_attempt_id": record.target_attempt_id().as_str(),
                "reason": record.reason().as_str(),
                "reactivated": record.reactivated(),
                "recorded_at": timestamp(record.recorded_at())?,
            });
            if let Some(actor) = record.actor()
                && let Some(object) = value.as_object_mut()
            {
                object.insert("actor".to_owned(), json!(actor.as_str()));
            }
            Ok(value)
        });
    result.insert("rework_history".to_owned(), history_window(entries, 6)?);

    let current_revision = state.goal_state().current_revision();
    let entries = state
        .goal_state()
        .revisions()
        .iter()
        .rev()
        .filter(|record| Some(record.revision()) != current_revision)
        .filter(|record| before_cursor(record.binding_trace(), cursor))
        .map(|record| stale_goal_revision_value(state, record));
    result.insert(
        "stale_goal_revision_history".to_owned(),
        history_window(entries, 1)?,
    );

    let entries = state
        .goal_state()
        .assessments()
        .iter()
        .rev()
        .filter(|assessment| {
            current_revision != Some(assessment.goal_revision())
                || !state.trace().attempts().iter().any(|attempt| {
                    attempt.attempt_id() == assessment.decision_attempt_id()
                        && attempt.validity() == AttemptValidityV2::Valid
                })
        })
        .filter(|assessment| before_cursor(assessment.decision_trace(), cursor))
        .map(|assessment| stale_goal_assessment_value(state, assessment));
    result.insert(
        "stale_goal_assessment_history".to_owned(),
        history_window(entries, 1)?,
    );
    Ok(())
}

fn before_cursor(sequence: TraceSequenceV2, cursor: Option<TraceSequenceV2>) -> bool {
    cursor.is_none_or(|cursor| sequence < cursor)
}

fn history_window(
    entries: impl Iterator<Item = Result<Value, GraphViewErrorV2>>,
    maximum: usize,
) -> Result<Value, GraphViewErrorV2> {
    let mut entries = entries;
    let mut bounded = Vec::new();
    let mut truncated = false;
    while bounded.len() < maximum {
        let Some(entry) = entries.next() else {
            break;
        };
        let entry = entry?;
        let mut candidate = bounded.clone();
        candidate.push(entry);
        // Budget with the longer `false` spelling so the final marker cannot add one byte when
        // this candidate happens to consume the iterator exactly.
        let candidate_window = history_window_value(candidate.clone(), false);
        let encoded_len = serde_json::to_vec(&candidate_window)
            .map_err(|_| GraphViewErrorV2::InconsistentState("history view is not JSON"))?
            .len();
        if encoded_len > HISTORY_WINDOW_BYTES_MAX {
            truncated = true;
            break;
        }
        bounded = candidate;
    }
    if !truncated && entries.next().is_some() {
        truncated = true;
    }
    Ok(history_window_value(bounded, truncated))
}

fn history_window_value(entries: Vec<Value>, trace_truncated: bool) -> Value {
    let sequences = entries
        .iter()
        .filter_map(|entry| entry.get("trace_sequence").and_then(Value::as_u64))
        .collect::<Vec<_>>();
    let trace_window = if sequences.is_empty() {
        Value::Null
    } else {
        json!({
            "first_sequence": sequences.iter().copied().min().unwrap_or(1),
            "last_sequence": sequences.iter().copied().max().unwrap_or(1),
        })
    };
    json!({
        "entries": entries,
        "trace_truncated": trace_truncated,
        "trace_window": trace_window,
    })
}

fn trace_entry(
    state: &GraphSessionStateV2,
    procedure: &ParsedProcedureV2,
    attempt: &SessionAttemptV2,
) -> Result<Value, GraphViewErrorV2> {
    let metadata = attempt_metadata(state, attempt)?;
    let definition_id = definition_id_for_node(procedure, attempt.graph_node_id().as_str())?;
    let mut value = Map::new();
    value.insert("trace_sequence".to_owned(), json!(attempt.trace().get()));
    value.insert(
        "graph_node_id".to_owned(),
        json!(attempt.graph_node_id().as_str()),
    );
    value.insert("node_definition_id".to_owned(), json!(definition_id));
    value.insert(
        "attempt_id".to_owned(),
        json!(attempt.attempt_id().as_str()),
    );
    value.insert("attempt_number".to_owned(), json!(attempt.number().get()));
    value.insert(
        "goal_revision".to_owned(),
        attempt
            .goal_revision()
            .map_or(Value::Null, |revision| json!(revision.get())),
    );
    value.insert(
        "lifecycle".to_owned(),
        json!(attempt_lifecycle(attempt.lifecycle())),
    );
    value.insert(
        "validity".to_owned(),
        json!(attempt_validity(attempt.validity())),
    );
    value.insert(
        "started_at".to_owned(),
        json!(timestamp(metadata.started_at())?),
    );
    if let Some(finished) = metadata.ended_at() {
        value.insert("finished_at".to_owned(), json!(timestamp(finished)?));
    }
    Ok(Value::Object(value))
}

fn stale_attempt_value(
    state: &GraphSessionStateV2,
    procedure: &ParsedProcedureV2,
    attempt: &SessionAttemptV2,
) -> Result<Value, GraphViewErrorV2> {
    let metadata = attempt_metadata(state, attempt)?;
    let memory = state
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == attempt.attempt_id())
        .ok_or(GraphViewErrorV2::InconsistentState(
            "stale attempt memory is absent",
        ))?;
    let definition_id = definition_id_for_node(procedure, attempt.graph_node_id().as_str())?;
    let all_items = memory
        .item_slots()
        .iter()
        .filter_map(|slot| {
            slot.value().map(|value| {
                let display = display_item_value(value);
                let (display, truncated) = truncate_chars(&display, ITEM_DISPLAY_CHARS_MAX);
                json!({"item_id": slot.item_id().as_str(), "value": display, "value_truncated": truncated})
            })
        })
        .collect::<Vec<_>>();
    let items_total = all_items.len();
    let items = all_items.into_iter().take(3).collect::<Vec<_>>();
    Ok(json!({
        "trace_sequence": attempt.trace().get(),
        "graph_node_id": attempt.graph_node_id().as_str(),
        "node_definition_id": definition_id,
        "attempt_id": attempt.attempt_id().as_str(),
        "attempt_number": attempt.number().get(),
        "goal_revision": attempt.goal_revision().map(|revision| revision.get()),
        "lifecycle": attempt_lifecycle(attempt.lifecycle()),
        "validity": "stale",
        "started_at": timestamp(metadata.started_at())?,
        "finished_at": timestamp(metadata.ended_at().ok_or(GraphViewErrorV2::InconsistentState("stale attempt has no finish timestamp"))?)?,
        "terminal_reason": metadata.terminal_reason(),
        "items": items,
        "items_total": items_total,
        "items_truncated": items_total > 3,
        "references": memory.evidence().iter().map(|reference| {
            reference_metadata(reference, procedure, !reference.resolution().is_unresolved())
        }).collect::<Result<Vec<_>, _>>()?,
    }))
}

fn decision_value(record: &podway_core::DecisionRecordV2) -> Result<Value, GraphViewErrorV2> {
    let mut value = json!({
        "trace_sequence": record.trace().get(),
        "session_id": record.session_id().as_str(),
        "session_revision": record.session_revision().get(),
        "procedure_schema": "podway.procedure/v2",
        "procedure_snapshot_id": record.procedure_snapshot_id().as_str(),
        "procedure_digest": record.procedure_digest().as_str(),
        "graph_node_id": record.graph_node_id().as_str(),
        "node_definition_id": record.node_definition_id().as_str(),
        "attempt_id": record.attempt_id().as_str(),
        "attempt_number": record.attempt_number().get(),
        "goal_revision": record.goal_revision().map(|revision| revision.get()),
        "option_id": record.selected_option().as_str(),
        "effect": record.route_effect().as_str(),
        "target_graph_node_id": record.route_target().as_str(),
        "reason": record.reason().as_str(),
        "recorded_at": timestamp(record.recorded_at())?,
        "references": record.evidence().references().iter().map(reference_snapshot_value).collect::<Vec<_>>(),
    });
    if let Some(actor) = record.actor()
        && let Some(object) = value.as_object_mut()
    {
        object.insert("actor".to_owned(), json!(actor.as_str()));
    }
    Ok(value)
}

fn decision_readback_projection(
    state: &GraphSessionStateV2,
    decision: &podway_core::DecisionRecordV2,
) -> Result<Value, GraphViewErrorV2> {
    let mut value = decision_value(decision)?;
    let Some(assessment) = state.goal_state().assessments().iter().find(|assessment| {
        assessment.decision_attempt_id() == decision.attempt_id()
            && assessment.decision_graph_node_id() == decision.graph_node_id()
            && assessment.decision_trace() == decision.trace()
            && decision.goal_revision() == Some(assessment.goal_revision())
    }) else {
        return Ok(value);
    };
    let object = value
        .as_object_mut()
        .ok_or(GraphViewErrorV2::InconsistentState(
            "decision projection is not an object",
        ))?;
    object.insert("assessment".to_owned(), json!("session_goal"));
    object.insert(
        "assessment_mode".to_owned(),
        json!(assessment.mode().as_str()),
    );
    object.insert(
        "goal_outcome".to_owned(),
        json!(assessment.outcome().as_str()),
    );
    object.insert(
        "criterion_results".to_owned(),
        Value::Array(
            assessment
                .criterion_results()
                .iter()
                .map(|result| {
                    json!({
                        "criterion_id": result.criterion_id().as_str(),
                        "status": result.status().as_str(),
                        "reason": result.reason().as_str(),
                        "citations": result.citations().iter().map(citation_value).collect::<Vec<_>>(),
                    })
                })
                .collect(),
        ),
    );
    Ok(value)
}

fn reference_snapshot_value(reference: &ResolvedEvidenceReferenceV2) -> Value {
    let mut value = Map::new();
    value.insert(
        "source_graph_node_id".to_owned(),
        json!(reference.source_node().as_str()),
    );
    match reference {
        ResolvedEvidenceReferenceV2::Resolved(snapshot) => {
            value.insert("state".to_owned(), json!("resolved"));
            add_reference_snapshot_fields(&mut value, snapshot);
        }
        ResolvedEvidenceReferenceV2::Skipped(snapshot) => {
            value.insert("state".to_owned(), json!("skipped"));
            add_reference_snapshot_fields(&mut value, snapshot);
        }
        ResolvedEvidenceReferenceV2::Unresolved { .. } => {
            value.insert("state".to_owned(), json!("unresolved"));
        }
    }
    Value::Object(value)
}

fn add_reference_snapshot_fields(
    value: &mut Map<String, Value>,
    snapshot: &podway_core::EvidenceReferenceSnapshotV2,
) {
    value.insert(
        "source_attempt_id".to_owned(),
        json!(snapshot.source_attempt_id().as_str()),
    );
    value.insert(
        "source_attempt_number".to_owned(),
        json!(snapshot.source_attempt_number().get()),
    );
    value.insert(
        "items_digest".to_owned(),
        json!(snapshot.items_digest().as_str()),
    );
}

fn stale_goal_revision_value(
    state: &GraphSessionStateV2,
    record: &podway_core::GoalRevisionRecordV2,
) -> Result<Value, GraphViewErrorV2> {
    let assessment = state
        .goal_state()
        .assessments()
        .iter()
        .filter(|assessment| assessment.goal_revision() == record.revision())
        .max_by_key(|assessment| assessment.decision_trace());
    Ok(json!({
        "trace_sequence": record.binding_trace().get(),
        "revision": record.revision().get(),
        "predecessor_revision": record.predecessor().map(|revision| revision.get()),
        "statement": record.statement().as_str(),
        "criteria": record.criteria().criteria().iter().map(|criterion| json!({
            "criterion_id": criterion.id().as_str(),
            "statement": criterion.statement(),
            "status": assessment.and_then(|assessment| assessment.criterion_results().iter().find(|result| result.criterion_id() == criterion.id())).map_or("unassessed", |result| result.status().as_str()),
        })).collect::<Vec<_>>(),
        "actor": record.actor().map(|actor| actor.as_str()),
        "recorded_at": timestamp(record.created_at())?,
        "rework_to": record.rework_to().map(|node| node.as_str()),
        "reactivated": record.reactivated(),
    }))
}

fn stale_goal_assessment_value(
    state: &GraphSessionStateV2,
    assessment: &podway_core::GoalAssessmentRecordV2,
) -> Result<Value, GraphViewErrorV2> {
    let decision = state
        .workflow_memory()
        .decisions()
        .iter()
        .find(|record| record.attempt_id() == assessment.decision_attempt_id())
        .ok_or(GraphViewErrorV2::InconsistentState(
            "goal assessment decision record is absent",
        ))?;
    Ok(json!({
        "trace_sequence": assessment.decision_trace().get(),
        "session_id": state.trace().session_id().as_str(),
        "session_revision": decision.session_revision().get(),
        "procedure_snapshot_id": state.snapshot().snapshot_id().as_str(),
        "procedure_digest": state.snapshot().digest().as_str(),
        "graph_node_id": assessment.decision_graph_node_id().as_str(),
        "node_definition_id": decision.node_definition_id().as_str(),
        "attempt_id": assessment.decision_attempt_id().as_str(),
        "attempt_number": decision.attempt_number().get(),
        "goal_revision": assessment.goal_revision().get(),
        "assessment": "session_goal",
        "option_id": decision.selected_option().as_str(),
        "effect": decision.route_effect().as_str(),
        "target_graph_node_id": decision.route_target().as_str(),
        "mode": assessment.mode().as_str(),
        "outcome": assessment.outcome().as_str(),
        "criterion_statuses": assessment.criterion_results().iter().map(|result| json!({
            "criterion_id": result.criterion_id().as_str(),
            "status": result.status().as_str(),
            "citations": result.citations().iter().map(citation_value).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "references": assessment.evidence().references().iter().map(reference_snapshot_value).collect::<Vec<_>>(),
        "actor": assessment.actor().map(|actor| actor.as_str()),
        "recorded_at": timestamp(assessment.recorded_at())?,
        "record_digest": goal_assessment_digest(state, decision, assessment)?,
    }))
}

fn goal_assessment_digest(
    state: &GraphSessionStateV2,
    decision: &podway_core::DecisionRecordV2,
    assessment: &podway_core::GoalAssessmentRecordV2,
) -> Result<String, GraphViewErrorV2> {
    let criterion_results = assessment
        .criterion_results()
        .iter()
        .map(|result| {
            let citations = result
                .citations()
                .iter()
                .map(|citation| {
                    if let Some(source) = citation.as_evidence() {
                        json!({"kind": "evidence", "source_graph_node_id": source.as_str()})
                    } else if let Some(item) = citation.as_item() {
                        json!({"item_id": item.as_str(), "kind": "item"})
                    } else {
                        Value::Null
                    }
                })
                .collect::<Vec<_>>();
            json!({
                "citations": citations,
                "criterion_id": result.criterion_id().as_str(),
                "reason": result.reason().as_str(),
                "status": result.status().as_str(),
            })
        })
        .collect::<Vec<_>>();
    let evidence = assessment
        .evidence()
        .references()
        .iter()
        .map(|reference| match reference {
            ResolvedEvidenceReferenceV2::Resolved(value) => json!({
                "items_digest": value.items_digest().as_str(),
                "resolved_at_ms": value.resolved_at().get(),
                "source_attempt_id": value.source_attempt_id().as_str(),
                "source_attempt_number": value.source_attempt_number().get(),
                "source_graph_node_id": value.source_node().as_str(),
                "state": "resolved",
            }),
            ResolvedEvidenceReferenceV2::Skipped(value) => json!({
                "items_digest": value.items_digest().as_str(),
                "resolved_at_ms": value.resolved_at().get(),
                "source_attempt_id": value.source_attempt_id().as_str(),
                "source_attempt_number": value.source_attempt_number().get(),
                "source_graph_node_id": value.source_node().as_str(),
                "state": "skipped",
            }),
            ResolvedEvidenceReferenceV2::Unresolved { source_node } => json!({
                "source_graph_node_id": source_node.as_str(),
                "state": "unresolved",
            }),
        })
        .collect::<Vec<_>>();
    let value = json!({
        "actor": assessment.actor().map(|actor| actor.as_str()),
        "criterion_results": criterion_results,
        "decision_attempt_id": assessment.decision_attempt_id().as_str(),
        "decision_graph_node_id": assessment.decision_graph_node_id().as_str(),
        "decision_trace_sequence": assessment.decision_trace().get(),
        "evidence": evidence,
        "goal_revision": assessment.goal_revision().get(),
        "mode": assessment.mode().as_str(),
        "outcome": assessment.outcome().as_str(),
        "recorded_at_ms": assessment.recorded_at().get(),
        "route_effect": decision.route_effect().as_str(),
        "route_target_graph_node_id": decision.route_target().as_str(),
        "selected_option_id": decision.selected_option().as_str(),
        "session_id": state.trace().session_id().as_str(),
    });
    let canonical = canonicalize_json_v1(&value).map_err(|_| {
        GraphViewErrorV2::InconsistentState("goal assessment is not canonicalizable")
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
}

fn citation_value(citation: &CriterionCitationV2) -> Value {
    if let Some(node) = citation.as_evidence() {
        json!({"reference_graph_node_id": node.as_str()})
    } else if let Some(item) = citation.as_item() {
        json!({"local_item_id": item.as_str()})
    } else {
        Value::Null
    }
}

fn attempt_metadata<'a>(
    state: &'a GraphSessionStateV2,
    attempt: &SessionAttemptV2,
) -> Result<&'a podway_store::AttemptMetadataV2, GraphViewErrorV2> {
    state
        .attempt_metadata()
        .iter()
        .find(|metadata| metadata.attempt_id() == attempt.attempt_id())
        .ok_or(GraphViewErrorV2::InconsistentState(
            "attempt metadata is absent",
        ))
}

fn definition_id_for_node<'a>(
    procedure: &'a ParsedProcedureV2,
    node_id: &str,
) -> Result<&'a str, GraphViewErrorV2> {
    procedure
        .graph()
        .placements()
        .iter()
        .find(|placement| placement.id().as_str() == node_id)
        .map(|placement| match placement {
            GraphPlacementV2::Action(placement) => placement.definition().as_str(),
            GraphPlacementV2::Decision(placement) => placement.definition().as_str(),
        })
        .ok_or(GraphViewErrorV2::InvalidSnapshot)
}

fn display_item_value(value: &RecordedItemValueV2) -> String {
    match value.item_type() {
        ItemTypeV1::Confirm => "true".to_owned(),
        ItemTypeV1::Text => value.as_text().unwrap_or_default().to_owned(),
        ItemTypeV1::Choice => value.as_choice().unwrap_or_default().to_owned(),
        ItemTypeV1::Integer => value.as_integer().unwrap_or_default().to_string(),
        ItemTypeV1::List => serde_json::to_string(value.as_list().unwrap_or_default())
            .unwrap_or_else(|_| "[]".to_owned()),
        ItemTypeV1::Artifact => {
            serde_json::to_string(&typed_item_value(value)).unwrap_or_else(|_| "{}".to_owned())
        }
    }
}

fn typed_item_value(value: &RecordedItemValueV2) -> Value {
    match value.item_type() {
        ItemTypeV1::Confirm => Value::Bool(true),
        ItemTypeV1::Text => json!(value.as_text().unwrap_or_default()),
        ItemTypeV1::Choice => json!(value.as_choice().unwrap_or_default()),
        ItemTypeV1::Integer => json!(value.as_integer().unwrap_or_default()),
        ItemTypeV1::List => json!(value.as_list().unwrap_or_default()),
        ItemTypeV1::Artifact => {
            let artifact = value
                .as_artifact()
                .expect("artifact type has an artifact value");
            json!({
                "location_type": match artifact.location_kind() {
                    podway_core::ArtifactLocationKindV1::LocalPath => "path",
                    podway_core::ArtifactLocationKindV1::ExternalReference => "reference",
                },
                "location": artifact.location(),
                "sha256_digest": artifact.digest().as_str(),
                "size_bytes": artifact.size_bytes(),
                "media_type": artifact.media_type(),
            })
        }
    }
}

fn truncate_chars(value: &str, maximum: usize) -> (String, bool) {
    if value.chars().count() <= maximum {
        return (value.to_owned(), false);
    }
    (value.chars().take(maximum).collect(), true)
}

fn push_unique<'a>(values: &mut Vec<&'a str>, value: &'a str) {
    if !values.contains(&value) {
        values.push(value);
    }
}

const fn item_type(value: ItemTypeV1) -> &'static str {
    match value {
        ItemTypeV1::Confirm => "confirm",
        ItemTypeV1::Text => "text",
        ItemTypeV1::Choice => "choice",
        ItemTypeV1::Integer => "integer",
        ItemTypeV1::List => "list",
        ItemTypeV1::Artifact => "artifact",
    }
}

const fn lifecycle(value: SessionLifecycle) -> &'static str {
    match value {
        SessionLifecycle::Prepared => "prepared",
        SessionLifecycle::Running => "running",
        SessionLifecycle::Completed => "completed",
        SessionLifecycle::Cancelled => "cancelled",
    }
}

const fn attempt_lifecycle(value: AttemptLifecycle) -> &'static str {
    match value {
        AttemptLifecycle::Active => "active",
        AttemptLifecycle::Completed => "completed",
        AttemptLifecycle::Skipped => "skipped",
        AttemptLifecycle::Abandoned => "abandoned",
    }
}

const fn attempt_validity(value: AttemptValidityV2) -> &'static str {
    match value {
        AttemptValidityV2::Valid => "valid",
        AttemptValidityV2::Stale => "stale",
    }
}

fn timestamp(value: UnixMillis) -> Result<String, GraphViewErrorV2> {
    let seconds = value.get() / 1_000;
    let millis = value.get() % 1_000;
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_date_from_unix_days(days);
    if !(0..=9_999).contains(&year) {
        return Err(GraphViewErrorV2::TimestampOutOfRange);
    }
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60,
    ))
}

fn civil_date_from_unix_days(days: u64) -> (i128, i128, i128) {
    let z = i128::from(days) + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}
