//! Pure, attempt-bound projection of a host-neutral delegation work package.

use podway_core::{
    AttemptId, AttemptNumberV2, EvidenceReferenceV2, GoalRevisionRecordV2, GraphNodeId, ItemId,
    ItemSpecV2, ItemTypeV1, MAX_DELEGATION_WORK_PACKAGE_BYTES_V1, MAX_ITEMS_PER_DEFINITION_V2,
    Revision, SessionId, Sha256Digest, WorkspaceId,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::procedure_v2_document::node_definition_value;
use crate::{
    CanonicalJsonV1, ConfigError, ParsedNodeDefinition, ParsedProcedureV2, ValidatedProcedureV2,
    canonical_json_from_serializable,
};

pub const DELEGATION_WORK_PACKAGE_SCHEMA_V1: &str = "podway.delegation-work-package/v1";
pub const DELEGATION_WORK_PACKAGE_MAX_BYTES_V1: usize = MAX_DELEGATION_WORK_PACKAGE_BYTES_V1;
// Goal: 1000 + 16*300 characters at six escaped bytes each, plus criterion identifiers and
// fixed identity/declaration fields. Evidence reserves cover selector metadata and each item's
// worst-case canonical metadata plus an explicit selector ID.
const PACKAGE_FIXED_RESERVE_BYTES: usize = 53_000;
const PACKAGE_EVIDENCE_RESERVE_BYTES: usize = 1_500;
const PACKAGE_ITEM_RESERVE_BYTES: usize = 400;

/// Admission charge for a package's immutable Procedure content and largest permitted goal and
/// evidence metadata. Identifiers are ASCII; current source strings use their canonical JSON byte
/// lengths, while every runtime-supplied field receives a fixed worst-case allocation.
pub(crate) fn validate_delegation_package_budget_v1(
    parsed: &ParsedProcedureV2,
) -> Result<(), ConfigError> {
    for placement in parsed.graph().placements() {
        let podway_core::GraphPlacementV2::Action(action) = placement else {
            continue;
        };
        if action.delegation().is_none() {
            continue;
        }
        let definition = parsed
            .node_definitions()
            .iter()
            .find(|definition| definition.id() == action.definition())
            .ok_or(ConfigError::InvalidValue {
                field: "delegation work package definition",
                reason: "action definition is missing",
            })?;
        let action_bytes =
            canonical_json_from_serializable(&node_definition_value(definition).into_json())?
                .as_str()
                .len();
        let mut charge = action_bytes.saturating_add(PACKAGE_FIXED_RESERVE_BYTES);
        if let Some(references) = action.evidence_from() {
            for reference in references.entries() {
                let count = if let Some(selected) = reference.selected_items() {
                    selected.len()
                } else {
                    source_items(parsed, reference.source_node())?.len()
                };
                charge = charge
                    .saturating_add(PACKAGE_EVIDENCE_RESERVE_BYTES)
                    .saturating_add(count.saturating_mul(PACKAGE_ITEM_RESERVE_BYTES));
            }
        }
        if charge > DELEGATION_WORK_PACKAGE_MAX_BYTES_V1 {
            return Err(ConfigError::OutOfBounds {
                field: "delegation work package worst-case JSON bytes",
                min: 0,
                max: DELEGATION_WORK_PACKAGE_MAX_BYTES_V1,
                actual: charge,
            });
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DelegationEvidenceItemV1 {
    Missing {
        item_id: ItemId,
    },
    Present {
        item_id: ItemId,
        item_revision: Revision,
        value_digest: Sha256Digest,
        value_type: ItemTypeV1,
        size_bytes: u64,
    },
}

impl DelegationEvidenceItemV1 {
    fn item_id(&self) -> &ItemId {
        match self {
            Self::Missing { item_id } | Self::Present { item_id, .. } => item_id,
        }
    }

    fn value(&self) -> Value {
        match self {
            Self::Missing { item_id } => json!({"item_id": item_id, "state": "missing"}),
            Self::Present {
                item_id,
                item_revision,
                value_digest,
                value_type,
                size_bytes,
            } => json!({
                "item_id": item_id,
                "state": "present",
                "item_revision": item_revision.get(),
                "value_digest": value_digest.as_str(),
                "value_type": item_type_name(*value_type),
                "size_bytes": size_bytes,
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DelegationEvidenceResolutionV1 {
    Resolved {
        source_attempt_id: AttemptId,
        source_attempt_number: AttemptNumberV2,
        source_items_digest: Sha256Digest,
    },
    Skipped {
        source_attempt_id: AttemptId,
        source_attempt_number: AttemptNumberV2,
        source_items_digest: Sha256Digest,
    },
    Unresolved,
}

impl DelegationEvidenceResolutionV1 {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Resolved { .. } => "resolved",
            Self::Skipped { .. } => "skipped",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DelegationEvidenceV1 {
    pub selector: EvidenceReferenceV2,
    pub resolution: DelegationEvidenceResolutionV1,
    pub items: Vec<DelegationEvidenceItemV1>,
}

pub struct DelegationWorkPackageInputV1<'a> {
    pub procedure: &'a ValidatedProcedureV2,
    pub workspace_uuid: &'a WorkspaceId,
    pub session_id: &'a SessionId,
    pub placement_id: &'a GraphNodeId,
    pub attempt_id: &'a AttemptId,
    pub goal: Option<&'a GoalRevisionRecordV2>,
    pub evidence: &'a [DelegationEvidenceV1],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DelegationWorkPackageV1 {
    package: Value,
    canonical_json: CanonicalJsonV1,
    digest: Sha256Digest,
}

impl DelegationWorkPackageV1 {
    pub fn package(&self) -> &Value {
        &self.package
    }

    pub fn canonical_json(&self) -> &CanonicalJsonV1 {
        &self.canonical_json
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

pub fn project_delegation_work_package_v1(
    input: DelegationWorkPackageInputV1<'_>,
) -> Result<DelegationWorkPackageV1, ConfigError> {
    if input.goal.is_some() && input.procedure.parsed().goal_tracking().is_none() {
        return Err(ConfigError::InvalidValue {
            field: "delegation work package goal",
            reason: "Procedure does not enable goal tracking",
        });
    }
    let placement = input
        .procedure
        .parsed()
        .graph()
        .placements()
        .iter()
        .find(|placement| placement.id() == input.placement_id)
        .and_then(|placement| match placement {
            podway_core::GraphPlacementV2::Action(action) => Some(action),
            podway_core::GraphPlacementV2::Decision(_) => None,
        })
        .ok_or(ConfigError::InvalidValue {
            field: "delegation work package placement",
            reason: "placement must be an action",
        })?;
    let declaration = placement.delegation().ok_or(ConfigError::InvalidValue {
        field: "delegation work package placement",
        reason: "placement must declare delegation",
    })?;
    let definition = input
        .procedure
        .parsed()
        .node_definitions()
        .iter()
        .find(|definition| definition.id() == placement.definition())
        .ok_or(ConfigError::InvalidValue {
            field: "delegation work package definition",
            reason: "action definition is missing",
        })?;
    let ParsedNodeDefinition::Action(_) = definition else {
        return Err(ConfigError::InvalidValue {
            field: "delegation work package definition",
            reason: "definition must be an action",
        });
    };
    let declared = placement
        .evidence_from()
        .map_or(&[][..], |value| value.entries());
    if declared.len() != input.evidence.len() {
        return Err(invalid_evidence());
    }
    let evidence = declared
        .iter()
        .zip(input.evidence)
        .map(|(selector, frozen)| evidence_value(input.procedure.parsed(), selector, frozen))
        .collect::<Result<Vec<_>, _>>()?;
    let mut action = node_definition_value(definition).into_json();
    action
        .as_object_mut()
        .expect("action projection is an object")
        .remove("type");
    let goal = input.goal.map(|record| {
        json!({
            "revision": record.revision().get(),
            "statement": record.statement().as_str(),
            "criteria": record.criteria().criteria().iter().map(|criterion| json!({
                "criterion_id": criterion.id().as_str(),
                "statement": criterion.statement(),
            })).collect::<Vec<_>>(),
        })
    });
    let package = json!({
        "schema": DELEGATION_WORK_PACKAGE_SCHEMA_V1,
        "workspace_uuid": input.workspace_uuid,
        "procedure_digest": input.procedure.digest().as_str(),
        "session_id": input.session_id,
        "placement_id": input.placement_id,
        "attempt_id": input.attempt_id,
        "delegation": {
            "binding_ref": declaration.binding_ref().as_str(),
            "role_ref": declaration.role_ref().as_str(),
            "role_snapshot_digest": declaration.role_snapshot_digest().as_str(),
            "max_rounds": declaration.max_rounds(),
            "after_accept": declaration.after_accept().as_str(),
        },
        "action": action,
        "goal": goal,
        "evidence": evidence,
    });
    let canonical_json = canonical_json_from_serializable(&package)?;
    if canonical_json.as_str().len() > DELEGATION_WORK_PACKAGE_MAX_BYTES_V1 {
        return Err(ConfigError::OutOfBounds {
            field: "delegation work package JSON bytes",
            min: 0,
            max: DELEGATION_WORK_PACKAGE_MAX_BYTES_V1,
            actual: canonical_json.as_str().len(),
        });
    }
    let mut hasher = Sha256::new();
    hasher.update(DELEGATION_WORK_PACKAGE_SCHEMA_V1.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical_json.as_str().as_bytes());
    let digest = Sha256Digest::new(format!("sha256:{:x}", hasher.finalize()))
        .map_err(|_| ConfigError::InvalidDigest)?;
    Ok(DelegationWorkPackageV1 {
        package,
        canonical_json,
        digest,
    })
}

fn evidence_value(
    parsed: &ParsedProcedureV2,
    selector: &EvidenceReferenceV2,
    frozen: &DelegationEvidenceV1,
) -> Result<Value, ConfigError> {
    if selector != &frozen.selector
        || (matches!(
            frozen.resolution,
            DelegationEvidenceResolutionV1::Skipped { .. }
                | DelegationEvidenceResolutionV1::Unresolved
        ) && frozen
            .items
            .iter()
            .any(|item| matches!(item, DelegationEvidenceItemV1::Present { .. })))
        || frozen.items.len() > MAX_ITEMS_PER_DEFINITION_V2
    {
        return Err(invalid_evidence());
    }
    let source_items = source_items(parsed, selector.source_node())?;
    let expected = selector.selected_items().map_or_else(
        || {
            source_items
                .iter()
                .map(|item| item.id())
                .collect::<Vec<_>>()
        },
        |items| items.iter().collect::<Vec<_>>(),
    );
    if expected.len() != frozen.items.len()
        || !expected
            .iter()
            .zip(&frozen.items)
            .all(|(expected, item)| *expected == item.item_id())
        || frozen.items.iter().any(|item| match item {
            DelegationEvidenceItemV1::Missing { .. } => false,
            DelegationEvidenceItemV1::Present {
                item_id,
                value_type,
                ..
            } => !source_items
                .iter()
                .any(|source| source.id() == item_id && source.item_type() == *value_type),
        })
    {
        return Err(invalid_evidence());
    }
    let mut selector_value = json!({
        "node": selector.source_node().as_str(),
        "required": selector.required(),
    });
    if let Some(items) = selector.selected_items() {
        selector_value["items"] = json!(items);
    }
    let mut result = json!({
        "selector": selector_value,
        "resolution": frozen.resolution.as_str(),
        "items": frozen.items.iter().map(DelegationEvidenceItemV1::value).collect::<Vec<_>>(),
    });
    if let DelegationEvidenceResolutionV1::Resolved {
        source_attempt_id,
        source_attempt_number,
        source_items_digest,
    }
    | DelegationEvidenceResolutionV1::Skipped {
        source_attempt_id,
        source_attempt_number,
        source_items_digest,
    } = &frozen.resolution
    {
        if *source_attempt_number < AttemptNumberV2::FIRST {
            return Err(invalid_evidence());
        }
        result["source_attempt_id"] = json!(source_attempt_id);
        result["source_attempt_number"] = json!(source_attempt_number.get());
        result["source_items_digest"] = json!(source_items_digest.as_str());
    }
    Ok(result)
}

fn source_items<'a>(
    parsed: &'a ParsedProcedureV2,
    source_node: &GraphNodeId,
) -> Result<&'a [ItemSpecV2], ConfigError> {
    let source = parsed
        .graph()
        .placements()
        .iter()
        .find(|placement| placement.id() == source_node)
        .ok_or_else(invalid_evidence)?;
    let definition_id = match source {
        podway_core::GraphPlacementV2::Action(action) => action.definition(),
        podway_core::GraphPlacementV2::Decision(decision) => decision.definition(),
    };
    let definition = parsed
        .node_definitions()
        .iter()
        .find(|definition| definition.id() == definition_id)
        .ok_or_else(invalid_evidence)?;
    Ok(match definition {
        ParsedNodeDefinition::Action(action) => action.items(),
        ParsedNodeDefinition::Decision(_) => &[],
    })
}

fn invalid_evidence() -> ConfigError {
    ConfigError::InvalidValue {
        field: "delegation work package evidence",
        reason: "frozen evidence does not match declared selectors",
    }
}

fn item_type_name(value: ItemTypeV1) -> &'static str {
    match value {
        ItemTypeV1::Confirm => "confirm",
        ItemTypeV1::Text => "text",
        ItemTypeV1::Choice => "choice",
        ItemTypeV1::Integer => "integer",
        ItemTypeV1::List => "list",
        ItemTypeV1::Artifact => "artifact",
        ItemTypeV1::CheckResult => "check_result",
    }
}
