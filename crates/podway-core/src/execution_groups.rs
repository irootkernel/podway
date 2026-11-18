//! Internal successor model contracts (ADR-0044). These values do not admit a new CLI runtime.
//!
//! The existing item, guard, goal and delegation declaration types are shared values, not a
//! selection of Procedure v2 semantics. Config owns document admission and graph analysis.
mod definitions;
mod lifecycle;
mod records;
pub use definitions::*;
pub use lifecycle::*;
pub use records::*;

use crate::{DomainError, ItemSpecV2};
use std::collections::BTreeSet;

pub const PROCEDURE_SCHEMA_V3: &str = "podway.procedure/v3";
pub const EXECUTION_GROUP_SCHEMA_V1: &str = "podway.execution-group/v1";
pub const MAX_GROUP_EXECUTIONS_V1: usize = 64;
pub const MAX_GROUP_PLACEMENTS_V1: usize = crate::MAX_GRAPH_NODES_PER_PROCEDURE_V2;
pub const MAX_GROUP_EDGES_V1: usize = crate::MAX_GRAPH_EDGES_PER_PROCEDURE_V2;
pub const MAX_SESSION_PLACEMENTS_V1: usize = 16_384;
pub const MAX_SESSION_ATTEMPTS_V1: usize = 65_536;
pub const MAX_PRIOR_WORK_DISPOSITIONS_V1: usize = 128;
pub const DEFAULT_REPLAN_SCORE_THRESHOLD_V1: u8 = 80;

fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}
fn bounded_text(value: &str, max: usize) -> Result<(), DomainError> {
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(invalid(
            "group record text is empty or exceeds its scalar bound",
        ));
    }
    Ok(())
}
fn item_contract(items: &[ItemSpecV2]) -> Result<(), DomainError> {
    if items.len() > crate::MAX_ITEMS_PER_DEFINITION_V2 {
        return Err(DomainError::BoundExceeded {
            field: "boundary items",
            actual: items.len() as u64,
            maximum: crate::MAX_ITEMS_PER_DEFINITION_V2 as u64,
            unit: crate::BoundUnitV2::Items,
        });
    }
    if items.iter().map(|i| i.id()).collect::<BTreeSet<_>>().len() != items.len() {
        return Err(invalid("boundary item identifiers must be unique"));
    }
    Ok(())
}

/// Declaration order is part of the canonical contract; only individual prompts may differ.
pub fn same_boundary_contract_v1(required: &[ItemSpecV2], supplied: &[ItemSpecV2]) -> bool {
    required.len() == supplied.len()
        && required
            .iter()
            .zip(supplied)
            .all(|(a, b)| a.same_boundary_contract(b))
}

#[cfg(test)]
mod tests;
