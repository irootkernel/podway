//! Bounded host-neutral facts in a scoped delegation acceptance.

use super::ScopedDelegationBindingV1;
use crate::{
    DelegationBindingRefV1, DelegationReceiptIdV1, DelegationRoleRefV1, DomainError,
    MAX_DELEGATION_ROUNDS_V1, MIN_DELEGATION_ROUNDS_V1, Sha256Digest, UnixMillis,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationPrimaryHostV1 {
    External,
    Managed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationWorkerDispositionV1 {
    Idle,
    Released,
}

/// Executor-owned identities. No value in this record grants authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedDelegationExecutionV1 {
    pub binding_ref: DelegationBindingRefV1,
    pub adapter_id: String,
    pub adapter_version: String,
    pub primary_host: DelegationPrimaryHostV1,
    pub scope_kind: String,
    pub scope_ref: String,
    pub supervisor_ref: String,
    pub worker_ref: String,
    pub task_ref: String,
    pub role_ref: String,
    pub role_snapshot_digest: Sha256Digest,
    pub agent_configuration_digest: Sha256Digest,
    pub accepted_task_digest: Sha256Digest,
}

impl ScopedDelegationExecutionV1 {
    pub fn validate(&self) -> Result<(), DomainError> {
        for field in [&self.adapter_id, &self.adapter_version, &self.scope_kind] {
            if field.is_empty()
                || field.len() > 128
                || !field.bytes().all(|byte| (b'!'..=b'~').contains(&byte))
            {
                return Err(invalid(
                    "delegation ASCII identity is not one to 128 printable ASCII bytes",
                ));
            }
        }
        for field in [
            &self.scope_ref,
            &self.supervisor_ref,
            &self.worker_ref,
            &self.task_ref,
            &self.role_ref,
        ] {
            DelegationRoleRefV1::new(field.clone())?;
        }
        Ok(())
    }
}

/// Closed structural result facts copied into the immutable receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedDelegationResultV1 {
    pub execution: ScopedDelegationExecutionV1,
    pub rounds_used: u8,
    pub result_digest: Sha256Digest,
    pub summary: String,
    pub worker_disposition: DelegationWorkerDispositionV1,
}

impl ScopedDelegationResultV1 {
    pub fn validate(&self) -> Result<(), DomainError> {
        self.execution.validate()?;
        if !(MIN_DELEGATION_ROUNDS_V1..=MAX_DELEGATION_ROUNDS_V1).contains(&self.rounds_used) {
            return Err(invalid(
                "delegation result round count is outside the supported bound",
            ));
        }
        if self.summary.is_empty() || self.summary.len() > 4096 || self.summary.contains('\0') {
            return Err(invalid(
                "delegation result summary must contain one to 4096 UTF-8 bytes without NUL",
            ));
        }
        Ok(())
    }
}

/// Persisted once for an exact attempt. Its package excludes the action's mutable outputs;
/// the distinct accepted-item digest fixes which outputs actually passed acceptance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedDelegationReceiptV1 {
    pub receipt_id: DelegationReceiptIdV1,
    pub binding: ScopedDelegationBindingV1,
    pub result: ScopedDelegationResultV1,
    pub accepted_items_digest: Sha256Digest,
    pub accepted_at: UnixMillis,
}

fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}
