//! Request-local preparation and writer-time validation of required local artifacts.

use crate::{
    execution::ExecutionBoundaryErrorV1,
    native_execution::{NativeDirectWorkspaceV1, PreparedLocalArtifactV1},
};
use podway_core::{
    ArtifactLocationKindV1, ArtifactValueV1, DomainError, ItemId, Revision,
    execution_groups::{ExecutionSessionV3, ScopedAttemptV1},
};
use podway_store::{
    WorkspaceBindingV1, direct_store::DirectStoreErrorV1, successor_store::CandidateStoreError,
};

#[derive(Debug, Eq, PartialEq)]
struct RequiredArtifact {
    target: ScopedAttemptV1,
    item: ItemId,
    revision: Revision,
    value: ArtifactValueV1,
}

#[derive(Debug)]
pub struct PreparedSuccessorProgressV1(Vec<(RequiredArtifact, PreparedLocalArtifactV1)>);

impl PreparedSuccessorProgressV1 {
    pub(crate) fn artifact_count(&self) -> usize {
        self.0.len()
    }
}

pub fn prepare_successor_progress(
    native: &NativeDirectWorkspaceV1,
    binding: &WorkspaceBindingV1,
    state: &ExecutionSessionV3,
) -> Result<PreparedSuccessorProgressV1, CandidateStoreError> {
    let artifacts = required_artifacts(state)?
        .into_iter()
        .map(|required| {
            let prepared = native
                .prepare_recorded_local_artifact(binding, &required.value)
                .map_err(artifact_boundary)?;
            Ok((required, prepared))
        })
        .collect::<Result<_, CandidateStoreError>>()?;
    Ok(PreparedSuccessorProgressV1(artifacts))
}

pub fn verify_successor_progress(
    native: &NativeDirectWorkspaceV1,
    binding: &WorkspaceBindingV1,
    state: &ExecutionSessionV3,
    prepared: &PreparedSuccessorProgressV1,
) -> Result<(), CandidateStoreError> {
    let current = required_artifacts(state)?;
    if current.len() != prepared.0.len() {
        return Err(DomainError::ArtifactChanged.into());
    }
    for (current, (prior, artifact)) in current.iter().zip(&prepared.0) {
        if current != prior {
            return Err(DomainError::ArtifactChanged.into());
        }
        native
            .verify_prepared_local_artifact_v1(binding, artifact)
            .map_err(artifact_boundary)?;
    }
    Ok(())
}

fn required_artifacts(
    state: &ExecutionSessionV3,
) -> Result<Vec<RequiredArtifact>, CandidateStoreError> {
    let Some(active) = state.active_attempt() else {
        return Ok(Vec::new());
    };
    let declarations = state
        .current_item_declarations()
        .ok_or(CandidateStoreError::Corrupt(
            "active successor item declarations are absent",
        ))?;
    let slots = active.slots();
    if declarations.len() != slots.len()
        || declarations.iter().zip(slots).any(|(declaration, slot)| {
            declaration.id() != slot.item_id() || declaration.item_type() != slot.item_type()
        })
    {
        return Err(CandidateStoreError::Corrupt(
            "active successor item declarations disagree with admitted slots",
        ));
    }
    let values = slots.iter().map(|slot| slot.value()).collect::<Vec<_>>();
    Ok(declarations
        .iter()
        .zip(slots)
        .filter_map(|(declaration, slot)| {
            let value = slot.value()?.as_artifact()?;
            (value.location_kind() == ArtifactLocationKindV1::LocalPath
                && declaration.required_now(declarations, &values))
            .then(|| RequiredArtifact {
                target: active.target().clone(),
                item: slot.item_id().clone(),
                revision: slot.revision(),
                value: value.clone(),
            })
        })
        .collect())
}

fn artifact_boundary(error: ExecutionBoundaryErrorV1) -> CandidateStoreError {
    match error {
        ExecutionBoundaryErrorV1::Domain(error) => error.into(),
        ExecutionBoundaryErrorV1::LocalArtifact(_) => DomainError::ArtifactChanged.into(),
        ExecutionBoundaryErrorV1::Transient { .. } => DirectStoreErrorV1::Busy.into(),
        ExecutionBoundaryErrorV1::ProcedureV2Unsupported => CandidateStoreError::Corrupt(
            "procedure source admission is not an artifact validation boundary",
        ),
    }
}
