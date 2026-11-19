//! Scoped, immutable receipt records bound to admitted successor attempts.

use super::{Result, bounded_text, corrupt, decode_row, encode};
use crate::v2_delegation::accepted_slot_set_digest_v1;
use podway_core::{
    AttemptId, AttemptLifecycle, DelegationDeclarationV1, MAX_DELEGATION_RECEIPT_BYTES_V1,
    SessionId, UnixMillis,
    execution_groups::{
        DelegationWorkerDispositionV1, ExecutionScopeV1, ExecutionSessionV3, GroupAttemptStateV1,
        PlacementKindV3, ScopedDelegationReceiptV1,
    },
};
use rusqlite::{Connection, Row, Transaction, params};

fn declaration<'a>(
    state: &'a ExecutionSessionV3,
    attempt: &GroupAttemptStateV1,
) -> Result<&'a DelegationDeclarationV1> {
    let target = attempt.target();
    let graph = match target.scope() {
        ExecutionScopeV1::Base => state.base_definition().graph(),
        ExecutionScopeV1::Group {
            definition_digest, ..
        } => state
            .group_definition(definition_digest)
            .ok_or_else(|| corrupt("scoped receipt definition is unavailable"))?
            .graph(),
    };
    let placement = graph
        .nodes()
        .iter()
        .find(|node| node.id() == target.node())
        .ok_or_else(|| corrupt("scoped receipt placement is unavailable"))?;
    match placement.kind() {
        PlacementKindV3::Action {
            delegation: Some(declaration),
            ..
        } => Ok(declaration),
        _ => Err(corrupt(
            "scoped receipt placement does not declare delegation",
        )),
    }
}

fn verify_receipt(
    state: &ExecutionSessionV3,
    attempt: &GroupAttemptStateV1,
    receipt: &ScopedDelegationReceiptV1,
) -> Result<()> {
    let declaration = declaration(state, attempt)?;
    let result = &receipt.result;
    result
        .validate()
        .map_err(|_| corrupt("invalid retained delegation result"))?;
    let disposition = match result.worker_disposition {
        DelegationWorkerDispositionV1::Idle => "idle",
        DelegationWorkerDispositionV1::Released => "released",
    };
    if receipt.binding.target() != attempt.target()
        || result.execution.binding_ref != *declaration.binding_ref()
        || result.execution.role_ref != declaration.role_ref().as_str()
        || result.execution.role_snapshot_digest != *declaration.role_snapshot_digest()
        || result.rounds_used > declaration.max_rounds()
        || !declaration
            .after_accept()
            .accepts_worker_disposition(disposition)
        || receipt.accepted_at < attempt.opened_at()
        || receipt.accepted_at > attempt.closed_at().unwrap_or(state.updated_at())
    {
        return Err(corrupt(
            "scoped receipt does not bind its retained attempt and declaration",
        ));
    }
    if let Some(binding) = attempt.delegation() {
        if binding != &receipt.binding
            || accepted_slot_set_digest_v1(attempt.slots())
                .map_err(|_| corrupt("invalid accepted slot set"))?
                != receipt.accepted_items_digest
        {
            return Err(corrupt(
                "current delegation binding does not bind the accepted slot set",
            ));
        }
    } else if attempt.lifecycle() == AttemptLifecycle::Completed {
        return Err(corrupt(
            "completed delegated action has no current acceptance binding",
        ));
    }
    Ok(())
}

fn read_row(row: &Row<'_>, session: &SessionId) -> Result<ScopedDelegationReceiptV1> {
    let receipt: ScopedDelegationReceiptV1 = decode_row(row, 2, MAX_DELEGATION_RECEIPT_BYTES_V1)?;
    if receipt.binding.target().session() != session
        || receipt.binding.target().attempt().as_str() != bounded_text(row, 0, super::UUID_BYTES)?
        || receipt.receipt_id.as_str() != bounded_text(row, 1, super::UUID_BYTES)?
    {
        return Err(corrupt("retained receipt identity differs from its row"));
    }
    Ok(receipt)
}

pub(crate) fn read_receipt(
    connection: &Connection,
    state: &ExecutionSessionV3,
    attempt: &AttemptId,
) -> Result<Option<ScopedDelegationReceiptV1>> {
    let session = state.fence().session;
    let mut statement = connection.prepare(
        "SELECT attempt_id, receipt_id, record_json, record_digest FROM successor_delegation_receipts
         WHERE session_id = ?1 AND attempt_id = ?2",
    )?;
    let mut rows = statement.query(params![session.as_str(), attempt.as_str()])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let receipt = read_row(row, &session)?;
    let attempt = state
        .history_attempt(attempt)
        .ok_or_else(|| corrupt("delegation receipt attempt is absent"))?;
    verify_receipt(state, attempt, &receipt)?;
    Ok(Some(receipt))
}

pub(crate) fn verify_receipts(connection: &Connection, state: &ExecutionSessionV3) -> Result<()> {
    let session = state.fence().session;
    let mut statement = connection.prepare(
        "SELECT attempt_id, receipt_id, record_json, record_digest FROM successor_delegation_receipts WHERE session_id = ?1",
    )?;
    let mut rows = statement.query([session.as_str()])?;
    let mut bound_receipts = 0;
    while let Some(row) = rows.next()? {
        let receipt = read_row(row, &session)?;
        let attempt = state
            .history_attempt(receipt.binding.target().attempt())
            .ok_or_else(|| corrupt("delegation receipt attempt is absent"))?;
        verify_receipt(state, attempt, &receipt)?;
        bound_receipts += usize::from(attempt.delegation().is_some());
    }
    if state
        .attempt_history()
        .iter()
        .filter(|attempt| attempt.delegation().is_some())
        .count()
        != bound_receipts
    {
        return Err(corrupt(
            "retained scoped delegation binding has no immutable receipt",
        ));
    }
    Ok(())
}

pub(crate) fn record_receipt(
    transaction: &Transaction<'_>,
    state: &ExecutionSessionV3,
    receipt: &ScopedDelegationReceiptV1,
    now: UnixMillis,
) -> Result<()> {
    let attempt = state
        .active_attempt()
        .ok_or_else(|| corrupt("delegation acceptance has no active attempt"))?;
    if receipt.accepted_at != now || attempt.delegation() != Some(&receipt.binding) {
        return Err(corrupt(
            "delegation receipt must bind the accepted current action and transaction time",
        ));
    }
    verify_receipt(state, attempt, receipt)?;
    let (record, digest) = encode(receipt)?;
    if record.len() > MAX_DELEGATION_RECEIPT_BYTES_V1 {
        return Err(corrupt(
            "delegation receipt exceeds the retained byte bound",
        ));
    }
    transaction.execute(
        "INSERT INTO successor_delegation_receipts VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            attempt.target().session().as_str(),
            attempt.target().attempt().as_str(),
            receipt.receipt_id.as_str(),
            record,
            digest.as_str(),
        ],
    )?;
    verify_receipts(transaction, state)
}
