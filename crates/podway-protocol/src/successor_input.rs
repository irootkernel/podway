//! Closed, bounded stdin decoding for scoped successor operations.

use crate::{
    IdempotencyKeyV1, ItemRecordManyOperationV1, MAX_DIRECT_INPUT_BYTES_V1, SliceErrorV1,
    slice::{RawItemRecordManyOperationV1, validate_item_record_many_operations_v1},
    validate_json_document_depth,
};
use podway_core::execution_groups::*;
use podway_core::{
    ActorAttributionV2, AttemptId, CheckResultInputBasisV2, CriterionAssessmentReasonV2,
    CriterionId, GoalRevisionNumberV2, GraphNodeId, ItemId, MAX_ITEMS_PER_DEFINITION_V2, OptionId,
    Revision, Sha256Digest, WorkspaceId,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeSet;

pub const GROUP_ACTIVATE_INPUT_SCHEMA: &str = "podway.group-activate-input/v1";
pub const GROUP_REPLAN_INPUT_SCHEMA: &str = "podway.group-replan-input/v1";
pub const SCOPED_CRITERION_INPUT_SCHEMA: &str = "podway.scoped-criterion-assessment-input/v1";
pub const SCOPED_ITEM_RECORD_MANY_INPUT_SCHEMA: &str = "podway.scoped-item-record-many-input/v1";
pub const SCOPED_DELEGATION_ACCEPT_INPUT_SCHEMA: &str = "podway.delegation-accept-input/v2";
pub const MAX_SCOPED_DELEGATION_ACCEPT_BYTES: usize = 32_768;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuccessorMutationContextV1 {
    pub workspace_uuid: WorkspaceId,
    pub idempotency_key: IdempotencyKeyV1,
    pub cursor: SuccessorCursorInputV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuccessorCursorInputV1 {
    pub attempt: ScopedAttemptV1,
    pub session_revision: Revision,
    pub goal_revision: Option<u64>,
    pub selected_group: Option<GroupExecutionIdV1>,
}

impl SuccessorCursorInputV1 {
    pub fn execution(&self) -> ExecutionCursorFenceV1 {
        ExecutionCursorFenceV1 {
            session: self.attempt.session().clone(),
            revision: self.session_revision,
            attempt: Some(self.attempt.clone()),
            goal_revision: self.goal_revision.map(GoalRevisionNumberV2::new),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupDocumentFormatV1 {
    Yaml,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GroupSourceInputV1 {
    Default {},
    Document {
        format: GroupDocumentFormatV1,
        source: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupActivateInputV1 {
    pub context: SuccessorMutationContextV1,
    pub source: GroupSourceInputV1,
    pub expected_definition: Sha256Digest,
    pub plan: PlanBindingV1,
    pub predecessor: Option<GroupExecutionIdV1>,
    pub working_copy_basis: CheckResultInputBasisV2,
    pub prior_work: Vec<PriorWorkDispositionV1>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupReplanInputV1 {
    pub context: SuccessorMutationContextV1,
    pub feedback: ReplanFeedbackV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopedCriterionAssessmentInputV1 {
    pub context: SuccessorMutationContextV1,
    pub actor: Option<ActorAttributionV2>,
    pub entries: Vec<SuccessorCriterionInputV1>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopedItemRecordManyInputV1 {
    pub context: SuccessorMutationContextV1,
    pub operations: Vec<ItemRecordManyOperationV1>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopedDelegationAcceptInputV1 {
    pub context: SuccessorMutationContextV1,
    pub package_digest: Sha256Digest,
    pub result: ScopedDelegationResultV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScopedDelegationAccept {
    #[serde(rename = "schema")]
    _schema: String,
    context: SuccessorMutationContextV1,
    package_digest: Sha256Digest,
    execution: ScopedDelegationExecutionV1,
    rounds_used: u8,
    result_digest: Sha256Digest,
    summary: String,
    worker_disposition: DelegationWorkerDispositionV1,
    accepted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScopedItemRecordMany {
    #[serde(rename = "schema")]
    _schema: String,
    context: SuccessorMutationContextV1,
    operations: Vec<RawItemRecordManyOperationV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawActivate {
    #[serde(rename = "schema")]
    _schema: String,
    context: SuccessorMutationContextV1,
    source: GroupSourceInputV1,
    expected_definition: Sha256Digest,
    plan: RawPlan,
    predecessor: Option<GroupExecutionIdV1>,
    working_copy_basis: RawBasis,
    prior_work: Vec<RawPriorWork>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlan {
    design_attempt: AttemptId,
    item_set_digest: Sha256Digest,
    items: Vec<RawPlanItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlanItem {
    item: ItemId,
    revision: Revision,
    value_digest: Sha256Digest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPriorWork {
    group_execution_id: GroupExecutionIdV1,
    attempt: Option<AttemptId>,
    item: Option<HistoricalItemV1>,
    choice: PriorWorkChoiceV1,
    rationale: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBasis {
    descriptor: String,
    digest: Sha256Digest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReplan {
    #[serde(rename = "schema")]
    _schema: String,
    context: SuccessorMutationContextV1,
    option: OptionId,
    reason: String,
    working_copy_basis: RawBasis,
    feedback: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCriterionAssessment {
    #[serde(rename = "schema")]
    _schema: String,
    context: SuccessorMutationContextV1,
    actor: Option<String>,
    assessments: Vec<RawCriterion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCriterion {
    criterion: CriterionId,
    status: String,
    reason: String,
    citations: Vec<RawCriterionCitation>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RawCriterionCitation {
    Local { node: GraphNodeId },
    Plan {},
    ExecutionResult {},
    Item { item: ItemId },
}

fn invalid(error: impl std::fmt::Display) -> SliceErrorV1 {
    SliceErrorV1::InvalidPayload {
        message: error.to_string(),
    }
}

fn find_placeholder<'a>(value: &'a Value, pointer: &mut String) -> Option<&'a str> {
    let prefix_len = pointer.len();
    match value {
        Value::String(text) => {
            let body = text.strip_prefix('<')?.strip_suffix('>')?;
            if !body.is_empty() && !body.contains(['<', '>']) && body.chars().count() <= 62 {
                return Some(text);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                pointer.push('/');
                pointer.push_str(&index.to_string());
                if let Some(token) = find_placeholder(value, pointer) {
                    return Some(token);
                }
                pointer.truncate(prefix_len);
            }
        }
        Value::Object(entries) => {
            for (key, value) in entries {
                pointer.push('/');
                for character in key.chars() {
                    match character {
                        '~' => pointer.push_str("~0"),
                        '/' => pointer.push_str("~1"),
                        other => pointer.push(other),
                    }
                }
                if let Some(token) = find_placeholder(value, pointer) {
                    return Some(token);
                }
                pointer.truncate(prefix_len);
            }
        }
        _ => {}
    }
    None
}

fn invalid_successor_document(schema: &str, value: &Value) -> SliceErrorV1 {
    const MAX_ERROR_MESSAGE_SCALARS: usize = 65_536;

    let mut pointer = String::new();
    let Some(token) = find_placeholder(value, &mut pointer) else {
        return SliceErrorV1::InvalidSuccessorDocument {
            message: format!(
                "invalid successor input document {schema}: the document does not match its schema"
            ),
            pointer: None,
        };
    };
    let mut message = format!(
        "invalid successor input document {schema}: replace template placeholder {token:?}"
    );
    // Error v2 bounds its message in Unicode scalars. Keep the token even if
    // the complete RFC 6901 pointer cannot fit that envelope.
    let pointer = if message.chars().count() + " at ".len() + pointer.chars().count()
        <= MAX_ERROR_MESSAGE_SCALARS
    {
        Some(pointer)
    } else {
        message.push_str(" (RFC 6901 pointer exceeds the error message length limit)");
        None
    };
    SliceErrorV1::InvalidSuccessorDocument { message, pointer }
}

fn decode<T: DeserializeOwned>(bytes: &[u8], expected: &str) -> Result<T, SliceErrorV1> {
    if bytes.len() > MAX_DIRECT_INPUT_BYTES_V1 {
        return Err(SliceErrorV1::ValueTooLong {
            field: "successor stdin request",
            maximum: MAX_DIRECT_INPUT_BYTES_V1,
            actual: bytes.len(),
        });
    }
    let value: Value = serde_json::from_slice(bytes).map_err(invalid)?;
    validate_json_document_depth(&value).map_err(invalid)?;
    if value["schema"].as_str() != Some(expected) {
        return Err(SliceErrorV1::InvalidValue { field: "schema" });
    }
    if !crate::validate_daemon_free_document_v1(expected, &value) {
        return Err(invalid_successor_document(expected, &value));
    }
    // Decode original bytes so that duplicate typed properties are always rejected.
    serde_json::from_slice(bytes).map_err(invalid)
}

pub fn decode_group_activate_input_v1(bytes: &[u8]) -> Result<GroupActivateInputV1, SliceErrorV1> {
    let RawActivate {
        _schema: _,
        context,
        source,
        expected_definition,
        plan,
        predecessor,
        working_copy_basis,
        prior_work,
    } = decode(bytes, GROUP_ACTIVATE_INPUT_SCHEMA)?;
    if plan.items.len() > MAX_ITEMS_PER_DEFINITION_V2
        || prior_work.len() > MAX_PRIOR_WORK_DISPOSITIONS_V1
    {
        return Err(SliceErrorV1::InvalidValue {
            field: "activation collections",
        });
    }
    if matches!(&source, GroupSourceInputV1::Document { source, .. } if source.is_empty()) {
        return Err(SliceErrorV1::EmptyValue { field: "source" });
    }
    let plan = PlanBindingV1::new(
        plan.design_attempt,
        plan.item_set_digest,
        plan.items
            .into_iter()
            .map(|item| PlanItemBindingV1::new(item.item, item.revision, item.value_digest))
            .collect::<Result<_, _>>()
            .map_err(invalid)?,
    )
    .map_err(invalid)?;
    let prior_work = prior_work
        .into_iter()
        .map(|row| {
            let target = match (row.attempt, row.item) {
                (Some(_), Some(_)) => {
                    return Err(SliceErrorV1::InvalidValue {
                        field: "prior_work reference",
                    });
                }
                (Some(attempt), None) => Some(HistoricalReferenceV1::Attempt(attempt)),
                (None, Some(item)) => Some(HistoricalReferenceV1::Item(item)),
                (None, None) => None,
            };
            PriorWorkDispositionV1::new(row.group_execution_id, target, row.choice, row.rationale)
                .map_err(invalid)
        })
        .collect::<Result<_, _>>()?;
    let working_copy_basis =
        CheckResultInputBasisV2::new(working_copy_basis.descriptor, working_copy_basis.digest)
            .map_err(invalid)?;
    Ok(GroupActivateInputV1 {
        context,
        source,
        expected_definition,
        plan,
        predecessor,
        working_copy_basis,
        prior_work,
    })
}

pub fn decode_scoped_item_record_many_input_v1(
    bytes: &[u8],
) -> Result<ScopedItemRecordManyInputV1, SliceErrorV1> {
    let RawScopedItemRecordMany {
        _schema: _,
        context,
        operations,
    } = decode(bytes, SCOPED_ITEM_RECORD_MANY_INPUT_SCHEMA)?;
    Ok(ScopedItemRecordManyInputV1 {
        context,
        operations: validate_item_record_many_operations_v1(operations)?,
    })
}

pub fn decode_scoped_delegation_accept_input_v1(
    bytes: &[u8],
) -> Result<ScopedDelegationAcceptInputV1, SliceErrorV1> {
    if bytes.len() > MAX_SCOPED_DELEGATION_ACCEPT_BYTES {
        return Err(SliceErrorV1::ValueTooLong {
            field: "scoped delegation acceptance",
            maximum: MAX_SCOPED_DELEGATION_ACCEPT_BYTES,
            actual: bytes.len(),
        });
    }
    let raw: RawScopedDelegationAccept = decode(bytes, SCOPED_DELEGATION_ACCEPT_INPUT_SCHEMA)?;
    if !raw.accepted {
        return Err(SliceErrorV1::InvalidValue { field: "accepted" });
    }
    let result = ScopedDelegationResultV1 {
        execution: raw.execution,
        rounds_used: raw.rounds_used,
        result_digest: raw.result_digest,
        summary: raw.summary,
        worker_disposition: raw.worker_disposition,
    };
    result.validate().map_err(invalid)?;
    Ok(ScopedDelegationAcceptInputV1 {
        context: raw.context,
        package_digest: raw.package_digest,
        result,
    })
}

pub fn decode_group_replan_input_v1(bytes: &[u8]) -> Result<GroupReplanInputV1, SliceErrorV1> {
    let RawReplan {
        _schema: _,
        context,
        option,
        reason,
        working_copy_basis,
        feedback,
    } = decode(bytes, GROUP_REPLAN_INPUT_SCHEMA)?;
    let basis =
        CheckResultInputBasisV2::new(working_copy_basis.descriptor, working_copy_basis.digest)
            .map_err(invalid)?;
    let feedback = ReplanFeedbackV1::new(
        context.cursor.attempt.clone(),
        option,
        reason,
        basis,
        feedback,
    )
    .map_err(invalid)?;
    Ok(GroupReplanInputV1 { context, feedback })
}

pub fn decode_scoped_criterion_input_v1(
    bytes: &[u8],
) -> Result<ScopedCriterionAssessmentInputV1, SliceErrorV1> {
    let RawCriterionAssessment {
        _schema: _,
        context,
        actor,
        assessments,
    } = decode(bytes, SCOPED_CRITERION_INPUT_SCHEMA)?;
    if assessments.is_empty() || assessments.len() > 16 || context.cursor.goal_revision.is_none() {
        return Err(SliceErrorV1::InvalidValue {
            field: "criterion assessment basis or count",
        });
    }
    let mut seen = BTreeSet::new();
    let entries = assessments
        .into_iter()
        .map(|row| {
            if !seen.insert(row.criterion.clone()) || row.citations.len() > 4 {
                return Err(SliceErrorV1::InvalidValue {
                    field: "criterion identity or citation count",
                });
            }
            let citations = row
                .citations
                .into_iter()
                .map(|citation| match citation {
                    RawCriterionCitation::Local { node } => {
                        SuccessorCriterionCitationV1::Evidence(EvidenceSourceV3::Local(node))
                    }
                    RawCriterionCitation::Plan {} => {
                        SuccessorCriterionCitationV1::Evidence(EvidenceSourceV3::Plan)
                    }
                    RawCriterionCitation::ExecutionResult {} => {
                        SuccessorCriterionCitationV1::Evidence(EvidenceSourceV3::ExecutionResult)
                    }
                    RawCriterionCitation::Item { item } => SuccessorCriterionCitationV1::Item(item),
                })
                .collect();
            Ok(SuccessorCriterionInputV1 {
                criterion: row.criterion,
                status: row.status.parse().map_err(invalid)?,
                reason: CriterionAssessmentReasonV2::new(row.reason).map_err(invalid)?,
                citations,
            })
        })
        .collect::<Result<_, _>>()?;
    let actor = actor
        .map(ActorAttributionV2::new)
        .transpose()
        .map_err(invalid)?;
    Ok(ScopedCriterionAssessmentInputV1 {
        context,
        actor,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn successor_inputs_share_canonical_scope_and_reject_open_documents() {
        let workspace = "00000000-0000-4000-8000-000000000091";
        let session = "00000000-0000-4000-8000-000000000092";
        let attempt = "00000000-0000-4000-8000-000000000093";
        let group = "00000000-0000-4000-8000-000000000094";
        let digest = format!("sha256:{}", "a".repeat(64));
        let context = json!({
           "workspace_uuid": workspace, "idempotency_key": "input-test",
           "cursor": {"attempt": {"session":session,"base_digest":digest,"scope":{"kind":"base"},"node":"design","attempt":attempt},
              "session_revision":2,"goal_revision":1,"selected_group":null}
        });
        let basis = json!({"descriptor":"Exact working-copy evidence","digest":digest});
        let activate = json!({"schema":GROUP_ACTIVATE_INPUT_SCHEMA,"context":context,"source":{"kind":"default"},"expected_definition":digest,
           "plan":{"design_attempt":attempt,"item_set_digest":digest,"items":[]},"predecessor":null,"working_copy_basis":basis,"prior_work":[]});
        let mut decision_context = context.clone();
        decision_context["cursor"]["selected_group"] = json!(group);
        decision_context["cursor"]["attempt"]["node"] = json!("review");
        decision_context["cursor"]["attempt"]["scope"] =
            json!({"kind":"group","group_execution_id":group,"definition_digest":digest});
        let replan = json!({"schema":GROUP_REPLAN_INPUT_SCHEMA,"context":decision_context,"option":"replan","reason":"New dependency",
           "working_copy_basis":basis,"feedback":"Preserve completed implementation and revalidate it."});
        let mut goal_context = context;
        goal_context["cursor"]["selected_group"] = json!(group);
        goal_context["cursor"]["attempt"]["node"] = json!("assess-goal");
        let assessment = json!({"schema":SCOPED_CRITERION_INPUT_SCHEMA,"context":goal_context,
           "assessments":[{"criterion":"checks","status":"satisfied","reason":"Review passed","citations":[{"kind":"execution_result"}]}]});
        let bytes = |value: &Value| serde_json::to_vec(value).unwrap();
        let a = decode_group_activate_input_v1(&bytes(&activate)).unwrap();
        assert_eq!(a.context.cursor.execution().session.as_str(), session);
        assert_eq!(
            json!(a.context.cursor.attempt.scope()),
            json!({"kind":"base"})
        );
        assert!(matches!(a.source, GroupSourceInputV1::Default {}));
        let r = decode_group_replan_input_v1(&bytes(&replan)).unwrap();
        assert_eq!(r.feedback.decision().attempt().as_str(), attempt);
        assert_eq!(
            json!(r.feedback.decision().scope()),
            decision_context["cursor"]["attempt"]["scope"]
        );
        assert_eq!(
            decode_scoped_criterion_input_v1(&bytes(&assessment))
                .unwrap()
                .entries
                .len(),
            1
        );
        let mut invalid = assessment.clone();
        invalid["assessments"][0]["citations"][0]["ignored"] = json!(true);
        assert!(decode_scoped_criterion_input_v1(&bytes(&invalid)).is_err());
        let mut invalid = activate;
        invalid["context"]["cursor"]
            .as_object_mut()
            .unwrap()
            .remove("selected_group");
        assert!(decode_group_activate_input_v1(&bytes(&invalid)).is_err());
        assert!(decode_group_replan_input_v1(&vec![0; MAX_DIRECT_INPUT_BYTES_V1 + 1]).is_err());
        let encoded = replan.to_string();
        let duplicate = format!(
            "{},\"schema\":\"{GROUP_REPLAN_INPUT_SCHEMA}\"}}",
            encoded.strip_suffix('}').unwrap()
        );
        assert!(decode_group_replan_input_v1(duplicate.as_bytes()).is_err());
    }
}
