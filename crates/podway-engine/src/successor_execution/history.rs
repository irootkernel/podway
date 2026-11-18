//! Retrospective candidate reads over admitted session snapshots.

use super::*;
use podway_core::{ItemTypeV1, recorded_value_digest_v2};
use podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1;
pub use podway_store::successor_store::CandidateSessionLocation;

const GROUPS_SCHEMA: &str = "podway.successor-group-list-fixture-result/v1";
const GROUP_SCHEMA: &str = "podway.successor-group-show-fixture-result/v1";
const GOAL_REVISIONS_SCHEMA: &str = "podway.successor-goal-revision-history-fixture-result/v1";
const MAX_HISTORY_ROWS: usize = 64;

/// Select one exact session in the isolated Store. Archive membership is explicit.
#[derive(Clone, Copy, Debug)]
pub struct SuccessorHistorySession<'a> {
    pub workspace: &'a WorkspaceId,
    pub session: &'a SessionId,
    pub location: CandidateSessionLocation,
}

/// A bounded page request. Omit the continuation to request the first page.
#[derive(Clone, Copy, Debug)]
pub struct SuccessorHistoryPage<'a> {
    pub limit: usize,
    pub continuation: Option<&'a str>,
}

pub fn list_successor_groups(
    store: &mut CandidateExecutionStore,
    target: SuccessorHistorySession<'_>,
    page: SuccessorHistoryPage<'_>,
) -> Result<CanonicalExecutionJsonV1> {
    page.check_limit()?;
    let state = read_state(store, target)?;
    let groups = state.group_history();
    let mut counts = BTreeMap::new();
    for entry in state
        .attempt_history()
        .iter()
        .filter_map(GroupAttemptStateV1::selected_decision_entry)
    {
        *counts.entry(entry.selected_group()).or_insert(0) += 1;
    }
    let rows = groups.iter().enumerate().map(|(index, group)| {
        let decision_count = counts
            .get(group.identity().execution())
            .copied()
            .unwrap_or(0);
        group_summary(index, group, decision_count)
    });
    let bounds = HistoryPagination::new(target, &state, page, GROUPS_SCHEMA, Value::Null)?;
    bounds.project(groups.len(), None, rows.map(Ok))
}

pub fn show_successor_group(
    store: &mut CandidateExecutionStore,
    target: SuccessorHistorySession<'_>,
    execution: &GroupExecutionIdV1,
    page: SuccessorHistoryPage<'_>,
) -> Result<CanonicalExecutionJsonV1> {
    page.check_limit()?;
    let state = read_state(store, target)?;
    let (index, group) = state
        .group_history()
        .iter()
        .enumerate()
        .find(|(_, group)| group.identity().execution() == execution)
        .ok_or(SuccessorExecutionError::HistoryGroupNotFound)?;
    let decisions: Vec<_> = state
        .attempt_history()
        .iter()
        .filter(|attempt| {
            attempt
                .selected_decision_entry()
                .is_some_and(|entry| entry.selected_group() == execution)
        })
        .collect();
    let identity = group.identity();
    let total = decisions.len();
    let detail = json!({
        "summary": group_summary(index, group, total),
        "base_digest": identity.base_digest(),
        "anchor": identity.anchor(),
        "goal_revision": identity.goal_revision().map(GoalRevisionNumberV2::get),
        "plan": {
            "design_attempt": identity.plan().design_attempt(),
            "item_set_digest": identity.plan().item_set_digest(),
            "items": identity.plan().items().iter().map(item_binding_json).collect::<Vec<_>>(),
        },
        "working_copy_basis": identity.working_copy_basis(),
        "terminal_attempt": group.terminal_attempt(),
        "closure": group.closure().map(closure_json),
    });
    let bounds = HistoryPagination::new(
        target,
        &state,
        page,
        GROUP_SCHEMA,
        json!([execution, identity.definition_digest()]),
    )?;
    bounds.project(
        total,
        Some(detail),
        decisions
            .into_iter()
            .map(|attempt| decision_row(attempt, &state)),
    )
}

/// Read goal-revision and reactivation lineage, including changes made without a selection.
pub fn list_successor_goal_revisions(
    store: &mut CandidateExecutionStore,
    target: SuccessorHistorySession<'_>,
    page: SuccessorHistoryPage<'_>,
) -> Result<CanonicalExecutionJsonV1> {
    page.check_limit()?;
    let state = read_state(store, target)?;
    let revisions = state.goal_revision_history();
    let bounds = HistoryPagination::new(target, &state, page, GOAL_REVISIONS_SCHEMA, Value::Null)?;
    bounds.project(
        revisions.len(),
        None,
        revisions.iter().map(|revision| {
            Ok(json!({
                "goal_revision": revision.revision.get(),
                "predecessor": revision.predecessor,
                "successor": revision.successor,
                "closure": closure_json(&revision.closure),
            }))
        }),
    )
}

fn closure_json(closure: &GroupSelectionClosureV1) -> Value {
    json!({
        "cause": closure.cause,
        "source": closure.source,
        "closed_at": closure.at,
        "feedback": closure.feedback.as_ref().map(|feedback| json!({
            "reason": feedback.reason(),
            "working_copy_basis": feedback.working_copy_basis(),
            "text": feedback.feedback(),
        })),
    })
}

pub(super) fn read_state(
    store: &mut CandidateExecutionStore,
    target: SuccessorHistorySession<'_>,
) -> Result<ExecutionSessionV3> {
    let (state, _, _) = store.history_snapshot(
        target.workspace,
        target.session,
        target.location,
        admit_candidate_definition,
    )?;
    Ok(state)
}

fn group_summary(index: usize, group: &GroupExecutionStateV1, decision_count: usize) -> Value {
    let identity = group.identity();
    json!({
        "activation_sequence": index + 1,
        "execution": identity.execution(),
        "definition_digest": identity.definition_digest(),
        "design_attempt": identity.plan().design_attempt(),
        "predecessor": identity.predecessor(),
        "outcome": group.outcome(),
        "closure_cause": group.closure().map(|closure| closure.cause),
        "decision_entries_total": decision_count,
    })
}

fn item_binding_json(item: &PlanItemBindingV1) -> Value {
    json!({"item": item.item(), "revision": item.revision(), "value_digest": item.value_digest()})
}

fn decision_row(attempt: &GroupAttemptStateV1, state: &ExecutionSessionV3) -> Result<Value> {
    let decision = attempt.decision();
    let assessment = decision
        .map(|decision| decision_assessment(attempt.target(), state, decision))
        .transpose()?
        .unwrap_or_default();
    Ok(json!({
        "attempt": attempt.target(),
        "entry_ordinal": attempt.decision_entry(),
        "choice": decision.map(|value| &value.option),
        "reason": decision.map(|value| &value.reason),
        "assessment": assessment,
        "guard_bindings": decision.map(|value| value.guard_values.iter().map(|guard| json!({
            "source": guard.source,
            "binding": item_binding_json(&guard.item),
        })).collect::<Vec<_>>()).unwrap_or_default(),
        "goal_assessment": decision.and_then(|value| value.goal_assessment).map(|(revision, outcome)| {
            json!({"revision": revision.get(), "outcome": outcome.as_str()})
        }),
    }))
}

fn decision_assessment(
    target: &ScopedAttemptV1,
    state: &ExecutionSessionV3,
    decision: &GroupDecisionRecordV1,
) -> Result<Vec<Value>> {
    let graph = match target.scope() {
        ExecutionScopeV1::Base => state.base_definition().graph(),
        ExecutionScopeV1::Group {
            definition_digest, ..
        } => state
            .group_definition(definition_digest)
            .ok_or(SuccessorExecutionError::Invalid(
                "retained Decision definition is absent",
            ))?
            .graph(),
    };
    let placement = graph
        .nodes()
        .iter()
        .find(|placement| placement.id() == target.node())
        .ok_or(SuccessorExecutionError::Invalid(
            "retained Decision placement is absent",
        ))?;
    let references = placement.evidence();
    if references.len() != decision.evidence_sources.len() {
        return Err(SuccessorExecutionError::Invalid(
            "retained Decision evidence differs from its declaration",
        ));
    }
    references
        .iter()
        .zip(&decision.evidence_sources)
        .map(|(reference, source)| {
            let Some(source) = source else {
                return Ok(json!({"source": null, "items": []}));
            };
            let attempt = state
                .history_attempt(source.attempt())
                .filter(|attempt| attempt.target() == source)
                .ok_or(SuccessorExecutionError::Invalid(
                    "retained Decision evidence attempt is absent",
                ))?;
            let mut items = Vec::new();
            for slot in attempt.slots() {
                if !reference.items().is_empty() && !reference.items().contains(slot.item_id()) {
                    continue;
                }
                let Some(value) = slot.value() else {
                    continue;
                };
                let digest =
                    recorded_value_digest_v2(value).map_err(|error| DomainError::InvalidState {
                        reason: error.reason(),
                    })?;
                let scalar = match value.item_type() {
                    ItemTypeV1::Confirm => json!(true),
                    ItemTypeV1::Choice => json!(value.as_choice()),
                    ItemTypeV1::Integer => json!(value.as_integer()),
                    _ => Value::Null,
                };
                items.push(json!({
                    "binding": {
                        "item": slot.item_id(), "revision": slot.revision(), "value_digest": digest,
                    },
                    "scalar": scalar,
                }));
            }
            Ok(json!({"source": source, "items": items}))
        })
        .collect()
}

impl SuccessorHistoryPage<'_> {
    fn check_limit(self) -> Result<()> {
        if !(1..=MAX_HISTORY_ROWS).contains(&self.limit) {
            return Err(SuccessorExecutionError::InvalidHistoryPageLimit);
        }
        Ok(())
    }
}

struct HistoryPagination {
    identity: String,
    offset: usize,
    limit: usize,
    header: Value,
}

impl HistoryPagination {
    fn new(
        target: SuccessorHistorySession<'_>,
        state: &ExecutionSessionV3,
        page: SuccessorHistoryPage<'_>,
        schema: &str,
        selector: Value,
    ) -> Result<Self> {
        let revision = state.fence().revision;
        let location = target.location.as_str();
        let identity = canonical_digest(
            &canonicalize_json_v1(&json!([
                schema,
                target.workspace,
                target.session,
                location,
                revision,
                selector,
            ]))
            .map_err(|_| SuccessorExecutionError::ResponseBudget)?,
        )?;
        let identity = identity.as_str().to_owned();
        let offset = history_offset(&identity, page.continuation)?;
        let header = json!({
            "schema": schema, "workspace": target.workspace, "session": target.session,
            "session_location": location, "session_revision": revision,
        });
        Ok(Self {
            identity,
            offset,
            limit: page.limit,
            header,
        })
    }

    fn project(
        self,
        total: usize,
        group: Option<Value>,
        rows: impl Iterator<Item = Result<Value>>,
    ) -> Result<CanonicalExecutionJsonV1> {
        if self.offset > 0 && self.offset >= total {
            return Err(SuccessorExecutionError::InvalidHistoryPageToken);
        }
        let mut record = self.header;
        record["total"] = json!(total);
        if let Some(group) = group {
            record["group"] = group;
        }
        // Reserve the continuation metadata before admitting any row. The canonical JSON
        // wrapper independently checks both the final byte bound and nesting depth.
        record["truncated"] = json!(true);
        record["next_page_token"] = json!(history_token(&self.identity, self.offset));
        record["rows"] = json!([]);
        let mut bytes = canonical_response(&record)?.as_str().len();
        let mut page = Vec::new();
        for row in rows.skip(self.offset).take(self.limit) {
            let row = row?;
            let row_bytes =
                canonical_response(&row)?.as_str().len() + usize::from(!page.is_empty());
            if bytes + row_bytes > MAX_DIRECT_OUTPUT_BYTES_V1 {
                break;
            }
            bytes += row_bytes;
            page.push(row);
        }
        let next = self.offset + page.len();
        if next < total && page.is_empty() {
            return Err(SuccessorExecutionError::ResponseBudget);
        }
        record["truncated"] = json!(next < total);
        record["next_page_token"] =
            json!((next < total).then(|| history_token(&self.identity, next)));
        record["rows"] = json!(page);
        canonical_response(&record)
    }
}

fn history_token(identity: &str, offset: usize) -> String {
    format!("{identity}:{offset:016x}")
}

fn history_offset(identity: &str, token: Option<&str>) -> Result<usize> {
    let Some(token) = token else { return Ok(0) };
    let invalid = || SuccessorExecutionError::InvalidHistoryPageToken;
    let (binding, offset) = token.rsplit_once(':').ok_or_else(invalid)?;
    let hex = |value: &str| {
        value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if binding
        .strip_prefix("sha256:")
        .is_none_or(|digest| digest.len() != 64 || !hex(digest))
        || offset.len() != 16
        || !hex(offset)
    {
        return Err(invalid());
    }
    let offset = usize::from_str_radix(offset, 16).map_err(|_| invalid())?;
    if offset == 0 {
        return Err(invalid());
    }
    if binding != identity {
        return Err(SuccessorExecutionError::StaleHistoryPageToken);
    }
    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_rejects_each_changed_identity_at_the_same_revision() {
        let document = json!({
            "schema":"podway.procedure/v3", "id":"history-token-fixture", "version":"1",
            "name":"History token", "purpose":"Exercise exact continuation binding.",
            "node_definitions":{
                "design":{"type":"action", "title":"Design", "intent":"Record the design."},
                "close":{"type":"action", "title":"Close", "intent":"Close the session."}
            },
            "graph":{"entry":"design","nodes":[
                {"id":"design","use":"design","next":"close"},
                {"id":"close","use":"close","terminal":true}
            ]},
            "execution_group":{"anchor":"design","success_return":"close",
                "plan_inputs":[], "result_items":[], "replan_decisions":[]}
        });
        let admitted =
            admit_canonical_definition(&canonicalize_json_v1(&document).unwrap()).unwrap();
        let SuccessorDefinition::Base(base) = admitted.definition() else {
            panic!("the identity fixture must admit a base definition");
        };
        let workspace = WorkspaceId::new("38dae986-f92e-460c-a96b-e8e216386db4").unwrap();
        let other_workspace = WorkspaceId::new("df2decc8-75d0-461a-b530-a20ae6ce90b0").unwrap();
        let session = SessionId::new("342db08c-dba5-48e4-bbd3-0cde6c76f078").unwrap();
        let other_session = SessionId::new("c791f1ec-9e67-4f91-8b45-c35b8370572d").unwrap();
        let state = ExecutionSessionV3::begin(
            session.clone(),
            base.clone(),
            admitted.digest().clone(),
            None,
            AttemptId::new("9c9b8234-b1c8-4e56-916f-37e97c83a04d").unwrap(),
            UnixMillis::new(1),
        )
        .unwrap();
        let target = SuccessorHistorySession {
            workspace: &workspace,
            session: &session,
            location: CandidateSessionLocation::Current,
        };
        let first = HistoryPagination::new(
            target,
            &state,
            SuccessorHistoryPage {
                limit: 1,
                continuation: None,
            },
            GROUPS_SCHEMA,
            Value::Null,
        )
        .unwrap()
        .project(
            2,
            None,
            [Ok(json!("first")), Ok(json!("second"))].into_iter(),
        )
        .unwrap();
        let first: Value = serde_json::from_str(first.as_str()).unwrap();
        let page = SuccessorHistoryPage {
            limit: 1,
            continuation: Some(first["next_page_token"].as_str().unwrap()),
        };
        assert_eq!(
            HistoryPagination::new(target, &state, page, GROUPS_SCHEMA, Value::Null)
                .unwrap()
                .offset,
            1
        );
        for changed in [
            SuccessorHistorySession {
                workspace: &other_workspace,
                ..target
            },
            SuccessorHistorySession {
                session: &other_session,
                ..target
            },
        ] {
            assert!(matches!(
                HistoryPagination::new(changed, &state, page, GROUPS_SCHEMA, Value::Null),
                Err(SuccessorExecutionError::StaleHistoryPageToken),
            ));
        }
    }

    #[test]
    fn history_page_reserves_response_metadata_before_accepting_rows() {
        let identity = format!("sha256:{}", "0".repeat(64));
        let header = json!({"schema": GROUP_SCHEMA});
        let total = 3;
        // The first two rows and the mandatory continuation fill the actual response ceiling.
        let mut expected = json!({
            "schema": GROUP_SCHEMA, "total": total, "truncated": true,
            "next_page_token": history_token(&identity, 2), "rows": ["a", ""],
        });
        let boundary =
            MAX_DIRECT_OUTPUT_BYTES_V1 - canonical_response(&expected).unwrap().as_str().len();
        let row = "b".repeat(boundary);
        expected["rows"][1] = json!(row);
        let project = |offset, large: &str| {
            HistoryPagination {
                identity: identity.clone(),
                offset,
                limit: 64,
                header: header.clone(),
            }
            .project(
                total,
                None,
                ["a", large, "tail"].into_iter().map(|text| Ok(json!(text))),
            )
            .unwrap()
        };
        let first = project(0, &row);
        assert_eq!(first.as_str().len(), MAX_DIRECT_OUTPUT_BYTES_V1);
        let first: Value = serde_json::from_str(first.as_str()).unwrap();
        assert_eq!(first, expected);
        let token = first["next_page_token"].as_str().unwrap();
        let offset = history_offset(&identity, Some(token)).unwrap();
        assert_eq!(offset, 2);
        let second: Value = serde_json::from_str(project(offset, &row).as_str()).unwrap();
        assert_eq!(second["rows"], json!(["tail"]));
        assert_eq!(second["truncated"], false);
        assert_eq!(second["next_page_token"], Value::Null);
        let too_large = row + "b";
        let first: Value = serde_json::from_str(project(0, &too_large).as_str()).unwrap();
        assert_eq!(first["rows"], json!(["a"]));
        assert_eq!(
            history_offset(&identity, first["next_page_token"].as_str()).unwrap(),
            1
        );
    }
}
