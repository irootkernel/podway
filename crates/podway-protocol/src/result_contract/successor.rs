//! Cross-field invariants for the scoped result schemas.

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    ResultSchemaContractV2, SUCCESSOR_AUTHORING_RESULT_SCHEMA, SUCCESSOR_PRESET_RESULT_SCHEMA,
    result_schema_v2,
};

const START: &str = "podway.successor-session-start-result/v1";
const CURSOR: &str = "podway.successor-cursor-mutation-result/v1";
const BOUNDARY: &str = "podway.successor-group-boundary-result/v1";
const REPLAN: &str = "podway.successor-group-replan-result/v1";
const OBSERVATION_V7: &str = "podway.observation-result/v7";
const OBSERVATION_V8: &str = "podway.observation-result/v8";
const COMPACT_STATUS: &str = "podway.scoped-compact-status-result/v1";
const VERBOSE_STATUS: &str = "podway.scoped-verbose-status-result/v1";
const WRAPPER: &str = "podway.mutation-observation-result/v2";
const DELEGATION_PACKAGE: &str = "podway.delegation-package-result/v2";
const DELEGATION_RECEIPT: &str = "podway.delegation-receipt-result/v2";
const DELEGATION_ACCEPT: &str = "podway.delegation-accept-result/v3";

pub(super) const RESULT_SCHEMAS: &[ResultSchemaContractV2] = &[
    result_schema_v2(
        "podway.successor-authoring-diagnostic/v1",
        "schemas/successor-authoring-diagnostic-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.scoped-criterion-citation-invalid-details/v1",
        "schemas/scoped-criterion-citation-invalid-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.scoped-criterion-citation-invalid-details/v2",
        "schemas/scoped-criterion-citation-invalid-details-v2.schema.json",
        &[],
    ),
    result_schema_v2(
        DELEGATION_PACKAGE,
        "schemas/delegation-package-result-v2.schema.json",
        &["delegation.package"],
    ),
    result_schema_v2(
        DELEGATION_RECEIPT,
        "schemas/delegation-receipt-result-v2.schema.json",
        &["delegation.receipt"],
    ),
    result_schema_v2(
        DELEGATION_ACCEPT,
        "schemas/delegation-accept-result-v3.schema.json",
        &["delegation.accept"],
    ),
    result_schema_v2(
        "podway.successor-session-archive-show-result/v1",
        "schemas/successor-session-archive-show-result-v1.schema.json",
        &["session.archive_show"],
    ),
    result_schema_v2(
        "podway.successor-session-reset-result/v1",
        "schemas/successor-session-reset-result-v1.schema.json",
        &["session.reset"],
    ),
    result_schema_v2(
        "podway.successor-session-archive-result/v1",
        "schemas/successor-session-archive-result-v1.schema.json",
        &["session.archive"],
    ),
    result_schema_v2(
        "podway.successor-session-archive-purge-result/v1",
        "schemas/successor-session-archive-purge-result-v1.schema.json",
        &["session.archive_purge"],
    ),
    result_schema_v2(
        "podway.successor-group-list-result/v1",
        "schemas/successor-group-list-result-v1.schema.json",
        &["group.list"],
    ),
    result_schema_v2(
        "podway.successor-group-show-result/v1",
        "schemas/successor-group-show-result-v1.schema.json",
        &["group.show"],
    ),
    result_schema_v2(
        "podway.successor-history-prior-work-result/v1",
        "schemas/successor-history-prior-work-result-v1.schema.json",
        &["group.prior_work"],
    ),
    result_schema_v2(
        "podway.successor-goal-revision-history-result/v1",
        "schemas/successor-goal-revision-history-result-v1.schema.json",
        &["history.goal_revisions"],
    ),
    result_schema_v2(
        "podway.successor-history-dag-result/v1",
        "schemas/successor-history-dag-result-v1.schema.json",
        &["history.dag"],
    ),
    result_schema_v2(
        "podway.successor-history-attempts-result/v1",
        "schemas/successor-history-attempts-result-v1.schema.json",
        &["history.attempts"],
    ),
    result_schema_v2(
        "podway.successor-history-blockers-result/v1",
        "schemas/successor-history-blockers-result-v1.schema.json",
        &["history.blockers"],
    ),
    result_schema_v2(
        "podway.successor-history-item-result/v1",
        "schemas/successor-history-item-result-v1.schema.json",
        &["history.item"],
    ),
    result_schema_v2(
        "podway.scoped-evidence-read-result/v1",
        "schemas/scoped-evidence-read-result-v1.schema.json",
        &["evidence.read"],
    ),
    result_schema_v2(
        "podway.procedure-source-error-details/v1",
        "schemas/procedure-source-error-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.scoped-option-guard-unsatisfied-details/v1",
        "schemas/scoped-option-guard-unsatisfied-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.scoped-option-not-allowed-details/v1",
        "schemas/scoped-option-not-allowed-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.scoped-group-replan-required-details/v1",
        "schemas/scoped-group-replan-required-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        "podway.scoped-evidence-unavailable-details/v1",
        "schemas/scoped-evidence-unavailable-details-v1.schema.json",
        &[],
    ),
    result_schema_v2(
        START,
        "schemas/successor-session-start-result-v1.schema.json",
        &["session.start", "session.start_replace"],
    ),
    result_schema_v2(
        CURSOR,
        "schemas/successor-cursor-mutation-result-v1.schema.json",
        &[
            "session.begin",
            "session.complete",
            "session.decide",
            "session.rework",
            "session.retry",
            "session.skip",
            "session.cancel",
            "session.block",
            "session.unblock",
            "goal.define",
            "goal.revise",
            "goal.assess_criterion",
            "goal.assess_criteria",
            "item.check",
            "item.uncheck",
            "item.set",
            "item.add",
            "item.remove",
            "item.attach",
            "item.clear",
            "item.record_many",
        ],
    ),
    result_schema_v2(
        BOUNDARY,
        "schemas/successor-group-boundary-result-v1.schema.json",
        &["group.activate", "group.return"],
    ),
    result_schema_v2(
        REPLAN,
        "schemas/successor-group-replan-result-v1.schema.json",
        &["group.replan"],
    ),
    result_schema_v2(
        OBSERVATION_V7,
        "schemas/observation-result-v7.schema.json",
        &["session.observe", "session.next", "session.status"],
    ),
    result_schema_v2(
        OBSERVATION_V8,
        "schemas/observation-result-v8.schema.json",
        &["session.observe"],
    ),
    result_schema_v2(
        COMPACT_STATUS,
        "schemas/scoped-compact-status-result-v1.schema.json",
        &["session.status"],
    ),
    result_schema_v2(
        VERBOSE_STATUS,
        "schemas/scoped-verbose-status-result-v1.schema.json",
        &["session.status"],
    ),
    result_schema_v2(
        WRAPPER,
        "schemas/mutation-observation-result-v2.schema.json",
        crate::OBSERVATION_BEARING_MUTATION_COMMANDS_V1,
    ),
];

pub(super) fn command_agrees(command: &str, result: &Value) -> bool {
    match result["schema"].as_str() {
        Some(
            START
            | CURSOR
            | BOUNDARY
            | REPLAN
            | SUCCESSOR_AUTHORING_RESULT_SCHEMA
            | SUCCESSOR_PRESET_RESULT_SCHEMA,
        ) => result["operation"].as_str() == Some(command),
        Some(WRAPPER) => command_agrees(command, &result["mutation_result"]),
        _ => true,
    }
}

pub(super) fn envelope_correlations(envelope: &Value, result: &Value) -> Option<bool> {
    let command = envelope["command"].as_str()?;
    let session = &envelope["session"];
    let revision_before = &session["revision_before"];
    let revision_after = &session["revision_after"];
    match result["schema"].as_str()? {
        SUCCESSOR_AUTHORING_RESULT_SCHEMA | SUCCESSOR_PRESET_RESULT_SCHEMA => {
            Some(command_agrees(command, result))
        }
        START if result["dry_run"] == true => {
            Some(command_agrees(command, result) && session.is_null())
        }
        START => Some(
            command_agrees(command, result)
                && session["id"].is_string()
                && session["id"] == result["session_id"]
                && session["lifecycle"] == "prepared"
                && revision_before == &result["revision"]
                && revision_after == &result["revision"],
        ),
        CURSOR => Some(
            command_agrees(command, result)
                && &result["revision_before"] == revision_before
                && result["changed"] == (revision_before != revision_after)
                && result["lifecycle"] == session["lifecycle"]
                && cursor_agrees(session, &result["cursor"])
                && (!command.starts_with("item.") || item_changes_agree(result)),
        ),
        BOUNDARY | REPLAN => Some(
            command_agrees(command, result)
                && &result["revision_before"] == revision_before
                && &result["revision"] == revision_after
                && result["active_attempt"]["session"] == session["id"]
                && result["closed_attempt"]["session"] == session["id"],
        ),
        "podway.scoped-evidence-read-result/v1" => Some(
            result["workspace"] == envelope["workspace"]["uuid"]
                && result["detail"]["consumer"]["session"] == result["session"]
                && result["detail"]["source_attempt"]["session"] == result["session"],
        ),
        OBSERVATION_V7 | OBSERVATION_V8 => {
            Some(observation_agrees(envelope, result) && revision_before == revision_after)
        }
        COMPACT_STATUS => Some(
            revision_before == revision_after
                && result["workspace"] == envelope["workspace"]["uuid"]
                && result["workspace_sequence"]
                    == envelope["workspace"]["latest_workspace_sequence"]
                && result["lifecycle"] == session["lifecycle"]
                && cursor_agrees(session, &result["cursor"]),
        ),
        VERBOSE_STATUS => Some(
            revision_before == revision_after
                && observation_agrees(envelope, &result["observation"])
                && verbose_history_agrees(result, session),
        ),
        DELEGATION_PACKAGE => Some(
            revision_before == revision_after
                && cursor_agrees(session, &result["cursor"])
                && envelope["workspace"]["uuid"] == result["package"]["workspace_uuid"]
                && result["cursor"]["attempt"] == result["package"]["target"]
                && result["cursor"]["selected_group"] == result["package"]["selected_group"]
                && result["cursor"]["goal_revision"] == result["package"]["goal"]["revision"]
                && session["title"] == result["package"]["task_title"]
                && delegation_package_digest_agrees(result),
        ),
        DELEGATION_RECEIPT => Some(
            revision_before == revision_after
                && result["target"]["session"] == session["id"]
                && (result["applicability"] == "historical" || session["lifecycle"] == "running")
                && ((result["receipt"].is_null() && result["acceptance_state"] == "missing")
                    || (result["receipt"].is_object()
                        && result["acceptance_state"] != "missing"
                        && result["receipt"]["workspace_uuid"] == envelope["workspace"]["uuid"]
                        && result["receipt"]["binding"]["target"] == result["target"]
                        && serde_json::from_value::<
                            podway_core::execution_groups::ScopedDelegationResultV1,
                        >(result["receipt"]["result"].clone())
                        .is_ok_and(|result| result.validate().is_ok()))),
        ),
        DELEGATION_ACCEPT => Some(
            result["target"]["session"] == session["id"]
                && revision_after == &result["session_revision"]
                && revision_before
                    .as_u64()
                    .and_then(|prior| prior.checked_add(1))
                    == revision_after.as_u64(),
        ),
        "podway.successor-session-archive-show-result/v1" => Some(
            observation_agrees(envelope, &result["observation"])
                && revision_before == revision_after
                && result["archive"]["session_id"] == session["id"]
                && result["archive"]["session_revision"] == *revision_after
                && result["disposition"]["session_revision"] == *revision_after
                && result
                    .get("attempt_history")
                    .is_none_or(|history| verbose_history_agrees(history, session)),
        ),
        "podway.successor-group-list-result/v1"
        | "podway.successor-group-show-result/v1"
        | "podway.successor-history-prior-work-result/v1"
        | "podway.successor-goal-revision-history-result/v1"
        | "podway.successor-history-dag-result/v1"
        | "podway.successor-history-attempts-result/v1"
        | "podway.successor-history-blockers-result/v1"
        | "podway.successor-history-item-result/v1" => Some(
            session.is_null()
                && envelope["workspace"]["uuid"] == result["workspace"]
                && result["truncated"] == result["next_page_token"].is_string(),
        ),
        WRAPPER => {
            let mutation = &result["mutation_result"];
            let observation = &result["observation"];
            let cursor_agrees_with_mutation = match mutation["schema"].as_str() {
                Some(START) => {
                    mutation["dry_run"] == false && observation["cursor"]["attempt"].is_null()
                }
                Some(CURSOR) => observation["cursor"] == mutation["cursor"],
                Some(BOUNDARY | REPLAN) => {
                    observation["cursor"]["attempt"] == mutation["active_attempt"]
                }
                _ => false,
            };
            Some(
                cursor_agrees_with_mutation
                    && result["workspace_sequence"] == observation["workspace_sequence"]
                    && observation_agrees(envelope, observation)
                    && envelope_correlations(envelope, mutation) == Some(true),
            )
        }
        _ => None,
    }
}

fn cursor_agrees(session: &Value, cursor: &Value) -> bool {
    cursor["session"] == session["id"] && cursor["session_revision"] == session["revision_after"]
}

fn delegation_package_digest_agrees(result: &Value) -> bool {
    let Ok(package) = podway_core::canonicalize_json_v1(&result["package"]) else {
        return false;
    };
    if package.len() > podway_core::MAX_DELEGATION_WORK_PACKAGE_BYTES_V1 {
        return false;
    }
    let mut hasher = Sha256::new();
    hasher.update(b"podway.delegation-work-package/v2\n");
    hasher.update(package.as_bytes());
    result["package_digest"] == format!("sha256:{:x}", hasher.finalize())
}

fn item_changes_agree(result: &Value) -> bool {
    let Some(items) = result["items"].as_array() else {
        return false;
    };
    let mut prior_item = None;
    let mut any_changed = false;
    for item in items {
        let (Some(id), Some(prior), Some(revision), Some(changed)) = (
            item["item"].as_str(),
            item["previous_revision"].as_u64(),
            item["revision"].as_u64(),
            item["changed"].as_bool(),
        ) else {
            return false;
        };
        if prior_item.is_some_and(|previous| previous >= id)
            || prior.checked_add(u64::from(changed)) != Some(revision)
            || item["recorded"] != item["value_digest"].is_string()
        {
            return false;
        }
        any_changed |= changed;
        prior_item = Some(id);
    }
    result["changed"] == any_changed
}

fn observation_agrees(envelope: &Value, observation: &Value) -> bool {
    let session = &envelope["session"];
    let workspace = &envelope["workspace"];
    workspace["uuid"] == observation["workspace"]
        && workspace["latest_workspace_sequence"] == observation["workspace_sequence"]
        && session["title"] == observation["title"]
        && session["lifecycle"] == observation["lifecycle"]
        && cursor_agrees(session, &observation["cursor"])
        && observation_lifecycle_agrees(envelope, observation)
}

fn verbose_history_agrees(result: &Value, session: &Value) -> bool {
    let Some(trace_length) = result["trace_length"].as_u64() else {
        return false;
    };
    let Some(attempts) = result["attempts"].as_array() else {
        return false;
    };
    let mut previous = None;
    for row in attempts {
        let Some(sequence) = row["trace_sequence"].as_u64() else {
            return false;
        };
        if sequence > trace_length
            || row["attempt"]["session"] != session["id"]
            || previous.is_some_and(|previous| sequence.checked_add(1) != Some(previous))
        {
            return false;
        }
        previous = Some(sequence);
    }
    result["history_before"].as_u64() == previous.filter(|sequence| *sequence > 1)
}

fn observation_lifecycle_agrees(envelope: &Value, observation: &Value) -> bool {
    let guide = &observation["lifecycle_guidance"];
    let archived = envelope["command"] == "session.archive_show";
    let disposed = guide["current_terminal_disposition"] == true;
    let terminal = matches!(
        observation["lifecycle"].as_str(),
        Some("completed" | "cancelled")
    );
    if guide["activity"] != if archived { "archived" } else { "current" }
        || (archived && !(terminal && disposed))
        || (disposed && !terminal)
    {
        return false;
    }
    let expected: &[(&str, bool, usize)] =
        match (archived, observation["lifecycle"].as_str(), disposed) {
            (false, Some("prepared"), false) => &[
                ("session.begin", false, 1),
                ("session.reset", true, 1),
                ("session.start_replace", true, 2),
            ],
            (false, Some("completed" | "cancelled"), true) => &[
                ("session.archive", true, 1),
                ("session.reset", true, 1),
                ("session.start", true, 2),
            ],
            (false, Some("completed" | "cancelled"), false) => {
                &[("session.terminal_disposition", false, 2)]
            }
            _ => &[],
        };
    let Some(operations) = guide["operations"].as_array() else {
        return false;
    };
    if operations.len() != expected.len()
        || !operations
            .iter()
            .zip(expected)
            .all(|(operation, (command, explicit, _))| {
                operation["command"] == *command
                    && operation["requires_explicit_authorization"] == *explicit
            })
    {
        return false;
    }
    if observation["schema"] == OBSERVATION_V7 {
        return observation.get("mutation_templates").is_none();
    }
    let Some(templates) = observation["mutation_templates"].as_array() else {
        return false;
    };
    if observation["lifecycle"] == "running" {
        return templates.iter().all(|template| {
            !matches!(
                template["command"].as_str(),
                Some(
                    "session.begin"
                        | "session.reset"
                        | "session.start_replace"
                        | "session.start"
                        | "session.terminal_disposition"
                        | "session.archive"
                )
            ) && observation_template_agrees(observation, template)
        });
    }
    templates.len() == expected.iter().map(|(_, _, count)| *count).sum::<usize>()
        && expected.iter().all(|(command, explicit, count)| {
            templates
                .iter()
                .filter(|template| template["command"] == *command)
                .filter(|template| {
                    template["requires_explicit_authorization"] == *explicit
                        && observation_template_agrees(observation, template)
                })
                .count()
                == *count
        })
        && !templates
            .iter()
            .enumerate()
            .any(|(index, template)| templates[..index].contains(template))
}

fn observation_template_agrees(observation: &Value, template: &Value) -> bool {
    let cursor = &observation["cursor"];
    let preconditions = &template["preconditions"];
    const CURSOR_FIELDS: [&str; 4] = [
        "attempt",
        "session_revision",
        "goal_revision",
        "selected_group",
    ];
    if CURSOR_FIELDS
        .iter()
        .any(|field| cursor[*field] != preconditions[*field])
    {
        return false;
    }
    let Some(command) = template["command"].as_str() else {
        return false;
    };
    let Some(workspace) = observation["workspace"].as_str() else {
        return false;
    };
    let Some(argv) = template["argv"]
        .as_array()
        .and_then(|argv| argv.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
    else {
        return false;
    };
    let ["podway", "--json", arguments @ ..] = argv.as_slice() else {
        return false;
    };
    if let Some(input) = template["stdin_template"].as_str() {
        return observation_stdin_template_agrees(observation, template, command, arguments, input);
    }
    let [arguments @ .., "--idempotency-key", "<idempotency-key>"] = arguments else {
        return false;
    };
    let revision = cursor["session_revision"].to_string();
    let semantic = match arguments {
        [
            semantic @ ..,
            "--if-workspace-uuid",
            id,
            "--if-session-id",
            session,
            "--if-session-revision",
            rev,
        ] if *id == workspace
            && cursor["session"] == *session
            && *rev == revision
            && matches!(
                command,
                "session.reset"
                    | "session.start_replace"
                    | "session.start"
                    | "session.terminal_disposition"
                    | "session.archive"
            ) =>
        {
            semantic
        }
        [
            semantic @ ..,
            "--if-workspace-uuid",
            id,
            "--if-cursor",
            argument,
        ] if *id == workspace
            && serde_json::from_str::<Value>(argument).is_ok_and(|fence| fence == *cursor) =>
        {
            semantic
        }
        _ => return false,
    };
    match (command, semantic) {
        ("session.begin", ["begin"]) => observation["base"]["goal_tracking"] == false,
        (
            "session.begin",
            [
                "begin",
                "--goal",
                "<goal>",
                "--criterion",
                "<criterion-id>=<statement>",
            ],
        ) => observation["base"]["goal_tracking"] == true,
        ("session.reset", ["reset"]) | ("session.archive", ["archive"]) => true,
        ("session.start", ["start", source @ ..])
        | ("session.start_replace", ["start", "--on-existing", "delete", source @ ..]) => matches!(
            source,
            ["--preset", "<preset>", "--task", "<task>"]
                | [
                    "--procedure",
                    "<procedure-file>",
                    "--expect-procedure-digest",
                    "<procedure-digest>",
                    "--task",
                    "<task>"
                ]
        ),
        (
            "session.terminal_disposition",
            ["disposition", "not-required", "--reason", "<reason>"],
        )
        | (
            "session.terminal_disposition",
            [
                "disposition",
                "handed-off",
                "--summary",
                "<summary>",
                "--reference",
                "<reference>",
            ],
        ) => true,
        _ => {
            let skip_requires_reason =
                observation["guidance"]["placement"]["skip"]["reason_required"] == true;
            template["requires_explicit_authorization"] == false
                && observation_has_operation(observation, command)
                && scoped_running_template_arguments_v1(command, skip_requires_reason)
                    == Some(semantic)
        }
    }
}

/// Canonical argument body for an advertised running-session template.
/// Cursor, workspace and caller-owned key are appended by the projection.
pub fn scoped_running_template_arguments_v1(
    command: &str,
    skip_requires_reason: bool,
) -> Option<&'static [&'static str]> {
    Some(match command {
        "session.block" => &["block", "--reason", "<reason>"],
        "session.cancel" => &["cancel", "--reason", "<reason>"],
        "session.retry" => &["retry", "--reason", "<reason>"],
        "session.skip" if skip_requires_reason => &["skip", "--reason", "<reason>"],
        "session.skip" => &["skip"],
        "session.unblock" => &["unblock", "--all"],
        "session.rework" => &["rework", "--to", "<eligible-node>", "--reason", "<reason>"],
        "session.complete" => &["complete"],
        "session.decide" => &["decide", "--option", "<option-id>", "--reason", "<reason>"],
        "group.return" => &["group", "return"],
        "goal.define" => &[
            "goal",
            "define",
            "--goal",
            "<goal>",
            "--criterion",
            "<criterion-id>=<statement>",
        ],
        "goal.assess_criterion" => &[
            "goal",
            "assess-criterion",
            "<criterion-id>",
            "--status",
            "<status>",
            "--reason",
            "<reason>",
        ],
        "item.check" => &[
            "check",
            "<item-id>",
            "--if-item-revision",
            "<item-revision>",
        ],
        "item.uncheck" => &[
            "uncheck",
            "<item-id>",
            "--if-item-revision",
            "<item-revision>",
        ],
        "item.clear" => &[
            "clear",
            "<item-id>",
            "--if-item-revision",
            "<item-revision>",
        ],
        "item.set" => &[
            "set",
            "<item-id>",
            "<value>",
            "--if-item-revision",
            "<item-revision>",
        ],
        "item.add" => &[
            "add",
            "<item-id>",
            "<value>",
            "--if-item-revision",
            "<item-revision>",
        ],
        "item.remove" => &[
            "remove",
            "<item-id>",
            "<value>",
            "--if-item-revision",
            "<item-revision>",
        ],
        "item.attach" => &[
            "attach",
            "<item-id>",
            "<path>",
            "--if-item-revision",
            "<item-revision>",
        ],
        _ => return None,
    })
}

fn observation_stdin_template_agrees(
    observation: &Value,
    template: &Value,
    command: &str,
    arguments: &[&str],
    input: &str,
) -> bool {
    let schema = match (command, arguments) {
        ("item.record_many", ["record", "--stdin"]) => crate::SCOPED_ITEM_RECORD_MANY_INPUT_SCHEMA,
        ("group.activate", ["group", "activate", "--stdin"]) => crate::GROUP_ACTIVATE_INPUT_SCHEMA,
        ("group.replan", ["group", "replan", "--stdin"]) => crate::GROUP_REPLAN_INPUT_SCHEMA,
        ("goal.assess_criteria", ["goal", "assess-criteria", "--stdin"]) => {
            crate::SCOPED_CRITERION_INPUT_SCHEMA
        }
        _ => return false,
    };
    let Ok(input) = serde_json::from_str::<Value>(input) else {
        return false;
    };
    let context = &input["context"];
    input["schema"] == schema
        && template["requires_explicit_authorization"] == false
        && context["workspace_uuid"] == observation["workspace"]
        && context["idempotency_key"] == "<idempotency-key>"
        && context["cursor"] == template["preconditions"]
        && match command {
            "group.activate" => {
                !observation["plan"].is_null()
                    && input["plan"] == observation["plan"]
                    && match input.get("predecessor") {
                        Some(Value::Null) => {
                            input["prior_work"].as_array().is_some_and(Vec::is_empty)
                        }
                        Some(Value::String(_)) => {
                            input["prior_work"] == "<prior-work-dispositions-array>"
                        }
                        _ => false,
                    }
            }
            "group.replan" => {
                observation_has_operation(observation, command)
                    && observation["guidance"]["replan_option_ids"]
                        .as_array()
                        .is_some_and(|options| options.contains(&input["option"]))
            }
            "item.record_many" => {
                observation_has_operation(observation, command)
                    && input["operations"] == "<typed-operations-array>"
            }
            "goal.assess_criteria" => observation_has_operation(observation, command),
            _ => false,
        }
}

fn observation_has_operation(observation: &Value, command: &str) -> bool {
    observation["guidance"]["available_operations"]
        .as_array()
        .is_some_and(|operations| operations.iter().any(|operation| operation == command))
}
