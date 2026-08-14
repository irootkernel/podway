//! Procedure-independent workspace projection for the v2-only store.

use rusqlite::Transaction;

use podway_core::{Revision, WorkspaceState};

use crate::StoreErrorV1;

pub(crate) fn load_workspace_state(
    _transaction: &Transaction<'_>,
    workspace_id: podway_core::WorkspaceId,
) -> Result<WorkspaceState, StoreErrorV1> {
    Ok(WorkspaceState::new(workspace_id, Revision::ZERO))
}
