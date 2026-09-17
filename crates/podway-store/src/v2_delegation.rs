//! Closed, bounded storage codec for one immutable delegated attempt receipt.

use podway_core::{
    AttemptId, DelegationReceiptIdV1, GraphNodeId, SessionId, Sha256Digest, WorkspaceId,
    canonicalize_json_v1, verify_canonical_json_v1,
};
use rusqlite::{Connection, Transaction, params};
use serde_json::{Map, Value};

use crate::{
    GraphSessionStateV2, RusqliteErrorContextV1, StoreErrorV1, StoreRecordKindV1,
    StoreValueErrorV1, map_rusqlite_error_v1,
};

pub const MAX_DELEGATION_RECEIPT_BYTES_V1: usize = 45_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DelegationReceiptV1 {
    canonical_json: String,
    value: Value,
}

fn invalid() -> StoreErrorV1 {
    StoreErrorV1::InvalidStateV1(StoreValueErrorV1::InvalidProcedureV2State {
        reason: "delegation receipt is invalid",
    })
}

fn corrupt() -> StoreErrorV1 {
    StoreErrorV1::CorruptStateV1 {
        record: StoreRecordKindV1::DelegationReceipt,
    }
}

impl DelegationReceiptV1 {
    pub fn new(value: Value) -> Result<Self, StoreErrorV1> {
        validate_receipt(&value)?;
        let canonical_json = canonicalize_json_v1(&value).map_err(|_| invalid())?;
        if canonical_json.len() > MAX_DELEGATION_RECEIPT_BYTES_V1 {
            return Err(invalid());
        }
        Ok(Self {
            canonical_json,
            value,
        })
    }

    pub fn decode(canonical_json: &str) -> Result<Self, StoreErrorV1> {
        if canonical_json.len() > MAX_DELEGATION_RECEIPT_BYTES_V1
            || verify_canonical_json_v1(canonical_json.as_bytes()).is_err()
        {
            return Err(corrupt());
        }
        let value: Value = serde_json::from_str(canonical_json).map_err(|_| corrupt())?;
        let decoded = Self::new(value).map_err(|_| corrupt())?;
        if decoded.canonical_json != canonical_json {
            return Err(corrupt());
        }
        Ok(decoded)
    }

    pub fn canonical_json(&self) -> &str {
        &self.canonical_json
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn session_id(&self) -> SessionId {
        SessionId::new(field(self.value.as_object().unwrap(), "session_id").unwrap()).unwrap()
    }

    pub fn attempt_id(&self) -> AttemptId {
        AttemptId::new(field(self.value.as_object().unwrap(), "attempt_id").unwrap()).unwrap()
    }

    pub fn receipt_id(&self) -> DelegationReceiptIdV1 {
        DelegationReceiptIdV1::new(field(self.value.as_object().unwrap(), "receipt_id").unwrap())
            .unwrap()
    }

    pub fn package_digest(&self) -> Sha256Digest {
        Sha256Digest::new(field(self.value.as_object().unwrap(), "package_digest").unwrap())
            .unwrap()
    }

    pub fn result_digest(&self) -> Sha256Digest {
        Sha256Digest::new(field(self.value.as_object().unwrap(), "result_digest").unwrap()).unwrap()
    }

    pub fn matches_state(&self, state: &GraphSessionStateV2, workspace_uuid: &WorkspaceId) -> bool {
        let Some(attempt) = state.trace().active_attempt() else {
            return false;
        };
        let Some(node) = state
            .snapshot()
            .graph_nodes()
            .iter()
            .find(|node| node.graph_node_id() == attempt.graph_node_id())
        else {
            return false;
        };
        let Ok(placement) = serde_json::from_str::<Value>(node.canonical_placement_json()) else {
            return false;
        };
        self.value.get("workspace_uuid").and_then(Value::as_str) == Some(workspace_uuid.as_str())
            && self.session_id() == *state.trace().session_id()
            && self.attempt_id() == *attempt.attempt_id()
            && self.value.get("placement_id").and_then(Value::as_str)
                == Some(attempt.graph_node_id().as_str())
            && self.value.get("procedure_digest").and_then(Value::as_str)
                == Some(state.snapshot().digest().as_str())
            && placement.get("delegation") == self.value.get("delegation")
    }
}

pub(crate) fn insert_receipt_transaction_v1(
    transaction: &Transaction<'_>,
    receipt: &DelegationReceiptV1,
    state: &GraphSessionStateV2,
    workspace_uuid: &WorkspaceId,
) -> Result<(), StoreErrorV1> {
    if !receipt.matches_state(state, workspace_uuid) {
        return Err(invalid());
    }
    transaction
        .execute(
            "INSERT INTO v2_delegation_receipts (attempt_id, session_id, receipt_id, receipt_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                receipt.attempt_id().as_str(),
                receipt.session_id().as_str(),
                receipt.receipt_id().as_str(),
                receipt.canonical_json(),
            ],
        )
        .map_err(|error| match &error {
            rusqlite::Error::SqliteFailure(failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                invalid()
            }
            _ => map_rusqlite_error_v1(
                error,
                RusqliteErrorContextV1::Record(StoreRecordKindV1::DelegationReceipt),
            ),
        })?;
    Ok(())
}

pub(crate) fn verify_receipts_connection_v1(connection: &Connection) -> Result<(), StoreErrorV1> {
    let total: i64 = connection
        .query_row("SELECT COUNT(*) FROM v2_delegation_receipts", [], |row| {
            row.get(0)
        })
        .map_err(|_| corrupt())?;
    let mut statement = connection
        .prepare(
            "SELECT r.attempt_id, r.session_id, r.receipt_id, r.receipt_json,
                    a.graph_node_id, p.digest, n.canonical_placement_json, w.workspace_uuid
             FROM v2_delegation_receipts r
             JOIN v2_attempts a ON a.attempt_id = r.attempt_id AND a.session_id = r.session_id
             JOIN v2_task_sessions s ON s.session_id = r.session_id
             JOIN v2_procedure_snapshots p ON p.snapshot_id = s.procedure_snapshot_id
             JOIN v2_graph_nodes n ON n.snapshot_id = a.snapshot_id AND n.graph_node_id = a.graph_node_id
             JOIN workspace_state w ON w.singleton = 1",
        )
        .map_err(|_| corrupt())?;
    let mut rows = statement.query([]).map_err(|_| corrupt())?;
    let mut verified = 0_i64;
    while let Some(row) = rows.next().map_err(|_| corrupt())? {
        verified += 1;
        let (
            attempt_id,
            session_id,
            receipt_id,
            canonical_json,
            graph_node_id,
            digest,
            placement_json,
            workspace_uuid,
        ): (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        ) = (
            row.get(0).map_err(|_| corrupt())?,
            row.get(1).map_err(|_| corrupt())?,
            row.get(2).map_err(|_| corrupt())?,
            row.get(3).map_err(|_| corrupt())?,
            row.get(4).map_err(|_| corrupt())?,
            row.get(5).map_err(|_| corrupt())?,
            row.get(6).map_err(|_| corrupt())?,
            row.get(7).map_err(|_| corrupt())?,
        );
        let receipt = DelegationReceiptV1::decode(&canonical_json)?;
        let placement: Value = serde_json::from_str(&placement_json).map_err(|_| corrupt())?;
        if receipt.attempt_id().as_str() != attempt_id
            || receipt.session_id().as_str() != session_id
            || receipt.receipt_id().as_str() != receipt_id
            || receipt.value.get("workspace_uuid").and_then(Value::as_str)
                != Some(workspace_uuid.as_str())
            || receipt
                .value
                .get("procedure_digest")
                .and_then(Value::as_str)
                != Some(digest.as_str())
            || receipt.value.get("placement_id").and_then(Value::as_str)
                != Some(graph_node_id.as_str())
            || placement.get("delegation") != receipt.value.get("delegation")
        {
            return Err(corrupt());
        }
    }
    if verified != total {
        return Err(corrupt());
    }
    Ok(())
}

fn field<'a>(object: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    object.get(name)?.as_str()
}

fn exact_keys(object: &Map<String, Value>, keys: &[&str]) -> bool {
    object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
}

fn digest(object: &Map<String, Value>, key: &str) -> bool {
    field(object, key).is_some_and(|text| Sha256Digest::new(text).is_ok())
}

fn opaque(object: &Map<String, Value>, key: &str) -> bool {
    field(object, key).is_some_and(|text| {
        !text.is_empty()
            && text.len() <= 256
            && !text.chars().any(|character| character.is_control())
    })
}

fn ascii_identity(object: &Map<String, Value>, key: &str) -> bool {
    field(object, key).is_some_and(|text| {
        !text.is_empty() && text.len() <= 128 && text.bytes().all(|byte| (33..=126).contains(&byte))
    })
}

fn declaration(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    exact_keys(
        object,
        &[
            "binding_ref",
            "role_ref",
            "role_snapshot_digest",
            "max_rounds",
            "after_accept",
        ],
    ) && field(object, "binding_ref")
        .is_some_and(|text| podway_core::DelegationBindingRefV1::new(text).is_ok())
        && opaque(object, "role_ref")
        && digest(object, "role_snapshot_digest")
        && object
            .get("max_rounds")
            .and_then(Value::as_u64)
            .is_some_and(|rounds| (1..=32).contains(&rounds))
        && matches!(field(object, "after_accept"), Some("retain" | "release"))
}

fn execution(value: &Value, declaration: &Map<String, Value>) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    exact_keys(
        object,
        &[
            "binding_ref",
            "adapter_id",
            "adapter_version",
            "primary_host",
            "scope_kind",
            "scope_ref",
            "supervisor_ref",
            "worker_ref",
            "task_ref",
            "role_ref",
            "role_snapshot_digest",
            "agent_configuration_digest",
            "accepted_task_digest",
        ],
    ) && object.get("binding_ref") == declaration.get("binding_ref")
        && object.get("role_ref") == declaration.get("role_ref")
        && object.get("role_snapshot_digest") == declaration.get("role_snapshot_digest")
        && ascii_identity(object, "adapter_id")
        && ascii_identity(object, "adapter_version")
        && matches!(field(object, "primary_host"), Some("external" | "managed"))
        && ascii_identity(object, "scope_kind")
        && ["scope_ref", "supervisor_ref", "worker_ref", "task_ref"]
            .iter()
            .all(|key| opaque(object, key))
        && digest(object, "agent_configuration_digest")
        && digest(object, "accepted_task_digest")
}

fn timestamp(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 24
        || !bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            13 | 16 => *byte == b':',
            19 => *byte == b'.',
            23 => *byte == b'Z',
            _ => byte.is_ascii_digit(),
        })
    {
        return false;
    }
    let component = |start: usize, end: usize| {
        bytes[start..end]
            .iter()
            .fold(0_u16, |value, byte| value * 10 + u16::from(*byte - b'0'))
    };
    let year = component(0, 4);
    let month = component(5, 7);
    let day = component(8, 10);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days_in_month).contains(&day)
        && component(11, 13) <= 23
        && component(14, 16) <= 59
        && component(17, 19) <= 59
}

fn validate_receipt(value: &Value) -> Result<(), StoreErrorV1> {
    let object = value.as_object().ok_or_else(invalid)?;
    if !exact_keys(
        object,
        &[
            "schema",
            "receipt_id",
            "workspace_uuid",
            "procedure_digest",
            "session_id",
            "placement_id",
            "attempt_id",
            "delegation",
            "package_digest",
            "execution",
            "rounds_used",
            "result_digest",
            "summary",
            "worker_disposition",
            "accepted",
            "accepted_items_digest",
            "accepted_at",
        ],
    ) || field(object, "schema") != Some("podway.delegation-receipt/v1")
        || !field(object, "receipt_id").is_some_and(|text| DelegationReceiptIdV1::new(text).is_ok())
        || !field(object, "workspace_uuid").is_some_and(|text| WorkspaceId::new(text).is_ok())
        || !field(object, "session_id").is_some_and(|text| SessionId::new(text).is_ok())
        || !field(object, "attempt_id").is_some_and(|text| AttemptId::new(text).is_ok())
        || !field(object, "placement_id").is_some_and(|text| GraphNodeId::new(text).is_ok())
        || ![
            "procedure_digest",
            "package_digest",
            "result_digest",
            "accepted_items_digest",
        ]
        .iter()
        .all(|key| digest(object, key))
        || !object.get("delegation").is_some_and(declaration)
        || !object
            .get("delegation")
            .and_then(Value::as_object)
            .is_some_and(|parent| {
                object
                    .get("execution")
                    .is_some_and(|child| execution(child, parent))
            })
        || !object
            .get("rounds_used")
            .and_then(Value::as_u64)
            .is_some_and(|rounds| {
                rounds >= 1
                    && rounds
                        <= object
                            .get("delegation")
                            .and_then(|value| value.get("max_rounds"))
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
            })
        || !field(object, "summary")
            .is_some_and(|text| !text.is_empty() && text.len() <= 4096 && !text.contains('\0'))
        || !matches!(
            field(object, "worker_disposition"),
            Some("idle" | "released")
        )
        || !object
            .get("delegation")
            .and_then(|value| value.get("after_accept"))
            .and_then(Value::as_str)
            .is_some_and(|policy| {
                matches!(
                    (policy, field(object, "worker_disposition")),
                    ("retain", Some("idle")) | ("release", Some("released"))
                )
            })
        || object.get("accepted") != Some(&Value::Bool(true))
        || !field(object, "accepted_at").is_some_and(timestamp)
    {
        return Err(invalid());
    }
    Ok(())
}
