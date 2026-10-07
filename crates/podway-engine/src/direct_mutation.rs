//! Shared direct mutation fences and complete, bounded success serialization.

use podway_core::{AttemptId, DomainError, Revision, SessionId, canonicalize_json_v1};
use podway_protocol::{
    CommandNameV1, DirectSessionOutputV1, OutputEnvelopeInputV4, OutputEnvelopeV4, RequestIdV1,
    Rfc3339MillisV1, WorkspaceOutputV1,
};
use podway_store::{
    CanonicalExecutionJsonV1, GraphMutationErrorV2, GraphSessionStateV2,
    direct_store::{DirectStoreErrorV1, DirectTransactionV1},
};

use crate::workspace::ResolvedWorkspaceV1;

pub(crate) fn validate_disposition_input(
    input: &podway_protocol::TerminalDispositionInputV1,
) -> Result<(), DirectStoreErrorV1> {
    let fields: &[&str] = match input {
        podway_protocol::TerminalDispositionInputV1::HandedOff { summary, reference } => {
            &[summary, reference]
        }
        podway_protocol::TerminalDispositionInputV1::NotRequired { reason } => &[reason],
    };
    if fields.iter().any(|value| {
        value.trim().is_empty()
            || value.chars().count() > podway_core::MAX_TERMINAL_DISPOSITION_CHARACTERS_V2
    }) {
        return Err(DirectStoreErrorV1::InvalidInput {
            reason: "invalid terminal disposition text",
        });
    }
    Ok(())
}

pub(crate) fn check_session_fences(
    graph: &GraphSessionStateV2,
    session: &SessionId,
    revision: Revision,
) -> Result<(), DirectStoreErrorV1> {
    if graph.trace().session_id() != session {
        return Err(
            GraphMutationErrorV2::Domain(DomainError::SessionIdentityMismatch {
                expected: session.clone(),
                actual: Some(graph.trace().session_id().clone()),
            })
            .into(),
        );
    }
    if graph.trace().revision() != revision {
        return Err(GraphMutationErrorV2::SessionRevisionConflict {
            expected: revision,
            actual: graph.trace().revision(),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn check_attempt_fence(
    graph: &GraphSessionStateV2,
    attempt: &AttemptId,
) -> Result<(), DirectStoreErrorV1> {
    let actual = graph
        .trace()
        .active_attempt()
        .map(|active| active.attempt_id());
    if actual != Some(attempt) {
        return Err(GraphMutationErrorV2::AttemptNotCurrent {
            expected: attempt.clone(),
            actual: actual.cloned(),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn graph_success<Clock: FnOnce() -> podway_core::UnixMillis>(
    transaction: &mut DirectTransactionV1<'_, '_, Clock>,
    resolved: &ResolvedWorkspaceV1,
    request_id: RequestIdV1,
    command: &str,
    prior: &GraphSessionStateV2,
    next: &GraphSessionStateV2,
    result: serde_json::Map<String, serde_json::Value>,
) -> Result<CanonicalExecutionJsonV1, DirectStoreErrorV1> {
    let output = OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
        request_id,
        command: CommandNameV1::new(command).map_err(|_| DirectStoreErrorV1::InvalidInput {
            reason: "invalid direct command",
        })?,
        generated_at: Rfc3339MillisV1::from_unix_millis(transaction.now().get()).map_err(|_| {
            DirectStoreErrorV1::InvalidInput {
                reason: "clock exceeds the public timestamp range",
            }
        })?,
        workspace: Some(
            WorkspaceOutputV1::new(
                transaction.binding().identity().workspace_uuid().clone(),
                resolved
                    .worktree()
                    .roots()
                    .worktree_root()
                    .display()
                    .as_str(),
                transaction.next_workspace_sequence()?,
            )
            .map_err(|_| DirectStoreErrorV1::BindingMismatch)?,
        ),
        session: Some(DirectSessionOutputV1 {
            id: next.trace().session_id().clone(),
            title: next.task_title().into(),
            lifecycle: next.trace().lifecycle(),
            revision_before: prior.trace().revision(),
            revision_after: next.trace().revision(),
        }),
        result,
        warnings: Vec::new(),
    })
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "direct success violates the canonical envelope",
    })?;
    CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(output.as_value()).map_err(|_| DirectStoreErrorV1::Unreadable)?,
    )
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "direct success exceeds the outcome bound",
    })
}
