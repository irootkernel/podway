//! Current successor results use the identical cursor under both presentation selectors.

use super::*;
use podway_core::{BlockerState, GoalOutcome, ItemTypeV1, SessionLifecycle};
use podway_protocol::{
    ContractGenerationV1, GROUP_ACTIVATE_INPUT_SCHEMA, GROUP_REPLAN_INPUT_SCHEMA,
    SCOPED_CRITERION_INPUT_SCHEMA, SCOPED_ITEM_RECORD_MANY_INPUT_SCHEMA,
};

pub(crate) const OBSERVATION_V7: &str = "podway.observation-result/v7";
pub(crate) const OBSERVATION_V8: &str = "podway.observation-result/v8";
pub(crate) const MUTATION_OBSERVATION_V2: &str = "podway.mutation-observation-result/v2";

pub(super) fn public_mutation_result(
    operation: &str,
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
    revision_before: Revision,
    mut fixture: Value,
) -> Result<Value> {
    let schema = match operation {
        "group.activate" | "group.return" => "podway.successor-group-boundary-result/v1",
        "group.replan" => "podway.successor-group-replan-result/v1",
        _ => {
            let cursor = SuccessorCursor::from_state(state);
            let mut result = json!({
                "schema": "podway.successor-cursor-mutation-result/v1",
                "operation": operation,
                "revision_before": revision_before,
                "cursor": cursor_json(&cursor),
                "lifecycle": lifecycle(state.lifecycle()),
                "changed": cursor.execution.revision != revision_before,
            });
            if operation.starts_with("item.") {
                result["items"] = std::mem::take(&mut fixture["items"]);
            }
            if operation.starts_with("goal.")
                || (operation == "session.begin" && state.current_goal().is_some())
            {
                result["goal"] = goal_summary(state);
            }
            return Ok(result);
        }
    };
    fixture["schema"] = json!(schema);
    fixture["operation"] = json!(operation);
    fixture["guidance"] = public_guidance(state, sources, active_target(state)?)?;
    Ok(fixture)
}

fn lifecycle(value: SessionLifecycle) -> &'static str {
    match value {
        SessionLifecycle::Prepared => "prepared",
        SessionLifecycle::Running => "running",
        SessionLifecycle::Completed => "completed",
        SessionLifecycle::Cancelled => "cancelled",
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ObservationActivityV1 {
    Current { terminal_disposition: bool },
    Archived,
}

impl ObservationActivityV1 {
    fn lifecycle_operations(self, lifecycle: SessionLifecycle) -> &'static [(&'static str, bool)] {
        match (self, lifecycle) {
            (Self::Current { .. }, SessionLifecycle::Prepared) => &[
                ("session.begin", false),
                ("session.reset", true),
                ("session.start_replace", true),
            ],
            (
                Self::Current {
                    terminal_disposition: true,
                },
                SessionLifecycle::Completed | SessionLifecycle::Cancelled,
            ) => &[
                ("session.archive", true),
                ("session.reset", true),
                ("session.start", true),
            ],
            (
                Self::Current {
                    terminal_disposition: false,
                },
                SessionLifecycle::Completed | SessionLifecycle::Cancelled,
            ) => &[("session.terminal_disposition", false)],
            _ => &[],
        }
    }

    fn guidance(self, lifecycle: SessionLifecycle) -> Value {
        let (activity, current_terminal_disposition) = match self {
            Self::Current {
                terminal_disposition,
            } => ("current", terminal_disposition),
            Self::Archived => ("archived", true),
        };
        json!({
            "activity": activity,
            "current_terminal_disposition": current_terminal_disposition,
            "operations": self.lifecycle_operations(lifecycle).iter()
                .map(|(command, explicit)| json!({
                    "command": command,
                    "requires_explicit_authorization": explicit,
                }))
                .collect::<Vec<_>>(),
        })
    }
}

pub(crate) fn public_observation(
    workspace: &WorkspaceId,
    title: &str,
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
    sequence: u64,
    generation: ContractGenerationV1,
    activity: ObservationActivityV1,
) -> Result<Value> {
    let mut view = current::observation(workspace, state, sources, sequence)?;
    view["schema"] = json!(match generation {
        ContractGenerationV1::Generation1 => OBSERVATION_V7,
        ContractGenerationV1::Generation2 => OBSERVATION_V8,
    });
    let base = state.base_definition();
    view["title"] = json!(title);
    view["base"] = json!({
        "id": base.header().id(),
        "version": base.header().version(),
        "digest": state.base_digest(),
        "goal_tracking": base.goal_tracking().is_some(),
    });
    view["goal"] = goal_summary(state);
    view["lifecycle_guidance"] = activity.guidance(state.lifecycle());
    view["selection"] = json!(state.selected().map(|group| json!({
        "execution": group.identity().execution(),
        "definition_digest": group.identity().definition_digest(),
        "outcome": group.outcome(),
        "terminal_attempt": group.terminal_attempt(),
    })));
    if let Some(active) = state.active_attempt() {
        view["guidance"] = public_guidance(state, sources, active.target())?;
    }
    let plan = state
        .active_attempt()
        .filter(|active| {
            active.target().scope() == &ExecutionScopeV1::Base
                && active.target().node() == base.boundary().anchor()
                && view["guidance"]["ready"] == true
                && (base.goal_tracking().is_none() || state.current_goal().is_some())
        })
        .map(|_| state.plan_binding())
        .transpose()?;
    view["plan"] = plan.as_ref().map(plan_json).unwrap_or(Value::Null);
    if generation == ContractGenerationV1::Generation2 {
        view["mutation_templates"] = json!(mutation_templates(workspace, state, &view, activity)?);
    }
    Ok(view)
}

fn goal_summary(state: &ExecutionSessionV3) -> Value {
    let Some(goal) = state.current_goal() else {
        return Value::Null;
    };
    let revision = state.fence().goal_revision;
    let assessments = state
        .active_attempt()
        .and_then(GroupAttemptStateV1::criterion_assessments)
        .filter(|set| Some(set.goal_revision) == revision);
    json!({
        "statement": goal.statement.as_str(),
        "actor": state.current_goal_actor().map(|actor| actor.as_str()),
        "revision": revision.map(GoalRevisionNumberV2::get),
        "criteria": goal.criteria.criteria().iter().map(|criterion| json!({
            "id": criterion.id(),
            "statement": criterion.statement(),
            "status": assessments.and_then(|set| {
                set.results.iter().find(|result| &result.criterion == criterion.id())
            }).map(|result| result.status.as_str()),
        })).collect::<Vec<_>>(),
        "outcome": state.determined_goal_outcome().ok().flatten()
            .or_else(|| state.current_goal_outcome()).map(GoalOutcome::as_str),
    })
}

fn plan_json(plan: &PlanBindingV1) -> Value {
    json!({
        "design_attempt": plan.design_attempt(),
        "item_set_digest": plan.item_set_digest(),
        "items": plan.items().iter().map(|item| json!({
            "item": item.item(),
            "revision": item.revision(),
            "value_digest": item.value_digest(),
        })).collect::<Vec<_>>(),
    })
}

pub(super) fn public_guidance(
    state: &ExecutionSessionV3,
    sources: &CandidateSources,
    attempt: &ScopedAttemptV1,
) -> Result<Value> {
    let mut guidance = current_guidance(state, sources, attempt)?;
    let (Some(active), Some(placement), Some(declarations)) = (
        state.active_attempt(),
        state.current_placement(),
        state.current_item_declarations(),
    ) else {
        return Err(SuccessorExecutionError::Invalid(
            "successor guidance requires a current placement and its declarations",
        ));
    };
    let values = active
        .slots()
        .iter()
        .map(|slot| slot.value())
        .collect::<Vec<_>>();
    let mut ready = state.current_readiness().is_ok();
    guidance["items"] = json!(
        declarations
            .iter()
            .zip(&values)
            .map(|(declaration, value)| {
                let required = declaration.required_now(declarations, &values);
                let satisfied = value.is_some_and(|value| declaration.is_satisfied_by(value));
                json!({"item": declaration.id(), "required": required, "satisfied": satisfied})
            })
            .collect::<Vec<_>>()
    );
    let evidence = placement
        .evidence()
        .iter()
        .map(|reference| {
            state
                .current_evidence_source(reference.source())
                .map(GroupAttemptStateV1::target)
        })
        .collect::<Vec<_>>();
    let declared = guidance["placement"]["evidence_from"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if declared.len() != evidence.len() {
        return Err(SuccessorExecutionError::Invalid(
            "successor guidance evidence differs from its admitted placement",
        ));
    }
    guidance["evidence"] = json!(
        declared
            .iter()
            .zip(evidence)
            .map(|(declaration, source)| { json!({"declaration": declaration, "attempt": source}) })
            .collect::<Vec<_>>()
    );

    let manual_rework = state.current_manual_rework_targets().collect::<Vec<_>>();
    let mut commands = vec!["session.block", "session.cancel", "session.retry"];
    if !manual_rework.is_empty() {
        commands.push("session.rework");
    }
    guidance["manual_rework_targets"] = json!(manual_rework);
    if active
        .blockers()
        .iter()
        .any(|blocker| blocker.state() == BlockerState::Open)
    {
        commands.push("session.unblock");
    }
    if state.base_definition().goal_tracking().is_some() && state.current_goal().is_none() {
        commands.push("goal.define");
    }
    if !declarations.is_empty() {
        commands.extend(["item.clear", "item.record_many"]);
        for item in declarations {
            let options: &[&str] = match item.item_type() {
                ItemTypeV1::Confirm => &["item.check", "item.uncheck"],
                ItemTypeV1::Text | ItemTypeV1::Choice | ItemTypeV1::Integer => &["item.set"],
                ItemTypeV1::List => &["item.add", "item.remove"],
                ItemTypeV1::Artifact => &["item.attach"],
                ItemTypeV1::CheckResult => &[],
            };
            for command in options {
                if !commands.contains(command) {
                    commands.push(command);
                }
            }
        }
    }
    match placement.kind() {
        PlacementKindV3::Action {
            outcome,
            delegation,
            ..
        } => {
            if delegation.is_some() {
                commands.push("delegation.accept");
                ready &= active.delegation().is_some();
            }
            if attempt.scope() == &ExecutionScopeV1::Base
                && attempt.node() == state.base_definition().boundary().anchor()
            {
                commands.push("group.activate");
            } else if attempt.scope() != &ExecutionScopeV1::Base
                && matches!(outcome, ActionOutcomeV2::Terminal)
            {
                commands.extend(["session.complete", "group.return"]);
            } else {
                commands.push("session.complete");
            }
            if state.current_skip_policy().is_some() {
                commands.push("session.skip");
            }
        }
        PlacementKindV3::Decision { .. } => {
            let decision =
                state
                    .current_decision_guidance()?
                    .ok_or(SuccessorExecutionError::Invalid(
                        "current Decision omitted its choice guidance",
                    ))?;
            if !decision.allowed_option_ids.is_empty() {
                commands.push("session.decide");
            }
            if !decision.replan_option_ids.is_empty() {
                commands.push("group.replan");
            }
            guidance["allowed_option_ids"] = json!(decision.allowed_option_ids);
            guidance["replan_option_ids"] = json!(decision.replan_option_ids);
            if let Some(statuses) = decision.guard_statuses {
                guidance["option_guard_statuses"] = json!(
                    statuses
                        .into_iter()
                        .map(|status| json!({
                            "option_id": status.option,
                            "state": status.state.as_str(),
                            "predicates": status.predicates.into_iter().map(|predicate| json!({
                                "source": predicate.source,
                                "item": predicate.item,
                                "state": predicate.state.as_str(),
                            })).collect::<Vec<_>>(),
                        }))
                        .collect::<Vec<_>>()
                );
            }
            if decision.goal_assessment_available {
                commands.extend(["goal.assess_criterion", "goal.assess_criteria"]);
            }
        }
    }
    guidance["ready"] = json!(ready);
    guidance["available_operations"] = json!(commands);
    Ok(guidance)
}

fn mutation_templates(
    workspace: &WorkspaceId,
    state: &ExecutionSessionV3,
    view: &Value,
    activity: ObservationActivityV1,
) -> Result<Vec<Value>> {
    let cursor = SuccessorCursor::from_state(state);
    let attempt = cursor.execution.attempt.as_ref();
    let preconditions = json!({
        "attempt": attempt,
        "session_revision": cursor.execution.revision,
        "goal_revision": cursor.execution.goal_revision.map(GoalRevisionNumberV2::get),
        "selected_group": cursor.selected_group,
    });
    let context = json!({
        "workspace_uuid": workspace,
        "idempotency_key": "<idempotency-key>",
        "cursor": preconditions,
    });
    let mut recipes = Vec::new();
    let key = "<idempotency-key>";
    let cursor_argument = serde_json::to_string(&cursor_json(&cursor))
        .map_err(|_| SuccessorExecutionError::ResponseBudget)?;
    let revision = cursor.execution.revision.get().to_string();
    let mut add_argv = |command: &str, arguments: &[&str], explicit_authority| {
        let mut argv = vec!["podway".to_owned(), "--json".to_owned()];
        argv.extend(arguments.iter().map(|value| (*value).to_owned()));
        argv.extend([
            "--if-workspace-uuid".to_owned(),
            workspace.as_str().to_owned(),
        ]);
        if matches!(
            command,
            "session.reset"
                | "session.start_replace"
                | "session.start"
                | "session.terminal_disposition"
                | "session.archive"
        ) {
            argv.extend([
                "--if-session-id".to_owned(),
                cursor.execution.session.as_str().to_owned(),
                "--if-session-revision".to_owned(),
                revision.clone(),
            ]);
        } else {
            argv.extend(["--if-cursor".to_owned(), cursor_argument.clone()]);
        }
        argv.extend(["--idempotency-key".to_owned(), key.to_owned()]);
        recipes.push(json!({
            "command": command,
            "argv": argv,
            "preconditions": preconditions,
            "authority": "optimistic_concurrency_only",
            "idempotency_key_required": true,
            "requires_explicit_authorization": explicit_authority,
            "stdin_template": null,
        }));
    };
    for &(command, explicit) in activity.lifecycle_operations(state.lifecycle()) {
        match command {
            "session.begin" if state.base_definition().goal_tracking().is_some() => add_argv(
                command,
                &[
                    "begin",
                    "--goal",
                    "<goal>",
                    "--criterion",
                    "<criterion-id>=<statement>",
                ],
                explicit,
            ),
            "session.begin" => add_argv(command, &["begin"], explicit),
            "session.reset" => add_argv(command, &["reset"], explicit),
            "session.archive" => add_argv(command, &["archive"], explicit),
            "session.start" | "session.start_replace" => {
                let mut start = vec!["start"];
                if command == "session.start_replace" {
                    start.extend(["--on-existing", "delete"]);
                }
                let mut preset = start.clone();
                preset.extend(["--preset", "<preset>", "--task", "<task>"]);
                add_argv(command, &preset, explicit);
                start.extend([
                    "--procedure",
                    "<procedure-file>",
                    "--expect-procedure-digest",
                    "<procedure-digest>",
                    "--task",
                    "<task>",
                ]);
                add_argv(command, &start, explicit);
            }
            "session.terminal_disposition" => {
                add_argv(
                    command,
                    &[
                        "disposition",
                        "handed-off",
                        "--summary",
                        "<summary>",
                        "--reference",
                        "<reference>",
                    ],
                    explicit,
                );
                add_argv(
                    command,
                    &["disposition", "not-required", "--reason", "<reason>"],
                    explicit,
                );
            }
            _ => unreachable!("lifecycle operation list names implemented commands"),
        }
    }
    if state.lifecycle() != SessionLifecycle::Running {
        return Ok(recipes);
    }
    let available = view["guidance"]["available_operations"].as_array().ok_or(
        SuccessorExecutionError::Invalid("running guidance omitted available operations"),
    )?;
    let allowed = |command: &str| available.iter().any(|operation| operation == command);
    let skip_requires_reason = view["guidance"]["placement"]["skip"]["reason_required"] == true;
    for command in available.iter().filter_map(Value::as_str) {
        if let Some(arguments) =
            podway_protocol::scoped_running_template_arguments_v1(command, skip_requires_reason)
        {
            add_argv(command, arguments, false);
        }
    }
    if attempt.is_none() {
        return Ok(recipes);
    }
    let mut add = |command, argv: &[&str], stdin: Value| -> Result<()> {
        recipes.push(json!({
            "command": command,
            "argv": argv,
            "preconditions": preconditions,
            "authority": "optimistic_concurrency_only",
            "idempotency_key_required": true,
            "requires_explicit_authorization": false,
            "stdin_template": serde_json::to_string(&stdin).map_err(|_| SuccessorExecutionError::ResponseBudget)?,
        }));
        Ok(())
    };
    if allowed("item.record_many") {
        add(
            "item.record_many",
            &["podway", "--json", "record", "--stdin"],
            json!({
                "schema": SCOPED_ITEM_RECORD_MANY_INPUT_SCHEMA,
                "context": context,
                "operations": "<typed-operations-array>",
            }),
        )?;
    }
    if !view["plan"].is_null() {
        let catalog = podway_presets::list_v3()
            .iter()
            .find(|preset| preset.base_digest == state.base_digest().as_str());
        let (source, expected_definition) = match catalog {
            Some(preset) => (json!({"kind": "default"}), preset.group_digest),
            None => (
                json!({"kind": "document", "format": "yaml", "source": "<group source>"}),
                "<group digest>",
            ),
        };
        let predecessor = state
            .group_history()
            .last()
            .map(|group| group.identity().execution());
        let prior_work = if predecessor.is_some() {
            json!("<prior-work-dispositions-array>")
        } else {
            json!([])
        };
        add(
            "group.activate",
            &["podway", "--json", "group", "activate", "--stdin"],
            json!({
                "schema": GROUP_ACTIVATE_INPUT_SCHEMA,
                "context": context,
                "source": source,
                "expected_definition": expected_definition,
                "plan": view["plan"],
                "predecessor": predecessor,
                "working_copy_basis": {"descriptor": "<working copy basis>", "digest": "<basis digest>"},
                "prior_work": prior_work,
            }),
        )?;
    }
    if let Some(option) = view["guidance"]["replan_option_ids"][0].as_str() {
        add(
            "group.replan",
            &["podway", "--json", "group", "replan", "--stdin"],
            json!({
                "schema": GROUP_REPLAN_INPUT_SCHEMA,
                "context": context,
                "option": option,
                "reason": "<reason>",
                "working_copy_basis": {"descriptor": "<working copy basis>", "digest": "<basis digest>"},
                "feedback": "<replan feedback>",
            }),
        )?;
    }
    if allowed("goal.assess_criteria") {
        add(
            "goal.assess_criteria",
            &["podway", "--json", "goal", "assess-criteria", "--stdin"],
            json!({
                "schema": SCOPED_CRITERION_INPUT_SCHEMA,
                "context": context,
                "actor": null,
                "assessments": [{
                    "criterion": "<criterion id>",
                    "status": "<status>",
                    "reason": "<reason>",
                    "citations": [],
                }],
            }),
        )?;
    }
    Ok(recipes)
}

pub(crate) const COMPACT_STATUS_V1: &str = "podway.scoped-compact-status-result/v1";
pub(crate) const VERBOSE_STATUS_V1: &str = "podway.scoped-verbose-status-result/v1";

pub(super) fn compact_status(
    workspace: &WorkspaceId,
    state: &ExecutionSessionV3,
    sequence: u64,
) -> Value {
    let base = state.base_definition();
    let active = state.active_attempt();
    json!({
        "schema": COMPACT_STATUS_V1,
        "workspace": workspace,
        "workspace_sequence": sequence,
        "lifecycle": lifecycle(state.lifecycle()),
        "cursor": cursor_json(&SuccessorCursor::from_state(state)),
        "base": {
            "id": base.header().id(),
            "version": base.header().version(),
            "digest": state.base_digest(),
            "goal_tracking": base.goal_tracking().is_some(),
        },
        "items": active.map(|attempt| attempt.slots().iter().map(|item| json!({
            "item": item.item_id(), "revision": item.revision(), "recorded": item.value().is_some(),
        })).collect::<Vec<_>>()),
        "trace_length": state.attempt_history().len(),
        "retained_group_count": state.group_history().len(),
    })
}

pub(super) fn verbose_status(
    observation: Value,
    state: &ExecutionSessionV3,
    history_before: Option<podway_core::TraceSequenceV2>,
) -> Result<Value> {
    const PAGE_LIMIT: usize = 16;
    // Leave enough room for the Direct envelope, including the workspace path.
    const ENVELOPE_HEADROOM: usize = 16 * 1024;
    let trace = state.attempt_history();
    let mut end = history_before
        .map(|before| usize::try_from(before.get().saturating_sub(1)).unwrap_or(usize::MAX))
        .unwrap_or(trace.len())
        .min(trace.len());
    let mut result = json!({
        "schema": VERBOSE_STATUS_V1,
        "observation": observation,
        "attempts": [],
        "trace_length": trace.len(),
        "history_before": null,
    });
    for _ in 0..PAGE_LIMIT {
        let Some(index) = end.checked_sub(1) else {
            break;
        };
        let entry = super::history::attempt_history_row(index, &trace[index]);
        result["attempts"].as_array_mut().unwrap().push(entry);
        result["history_before"] = if index == 0 {
            Value::Null
        } else {
            json!(index + 1)
        };
        if podway_protocol::serialized_json_len_v1(&result)
            .map_err(|_| SuccessorExecutionError::ResponseBudget)?
            > podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1 - ENVELOPE_HEADROOM
        {
            result["attempts"].as_array_mut().unwrap().pop();
            result["history_before"] = if end == 0 {
                Value::Null
            } else {
                json!(end + 1)
            };
            if result["attempts"].as_array().unwrap().is_empty() {
                return Err(SuccessorExecutionError::ResponseBudget);
            }
            break;
        }
        end = index;
    }
    Ok(result)
}
