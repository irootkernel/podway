//! Pure Procedure v2 authoring domain values.
//!
//! The additive, individually-bounded value types composing a Procedure v2 model are split by
//! cohesive ownership: item specifications; action and decision node definitions together with
//! their reason, options, and assessment contracts; placements, routes, evidence, and the
//! declarative graph; and session-goal tracking with its reason values.
//! Each constructor enforces only the identifier, scalar, collection, uniqueness, and cross-field
//! bounds owned by that single value; graph topology, cursor transitions, workflow records,
//! parsing, cross-reference validation, and canonicalization are owned by later tasks.

mod definitions;
mod goal;
mod graph;
mod items;

pub use definitions::*;
pub use goal::*;
pub use graph::*;
pub use items::*;

use crate::DomainError;

/// The exact Procedure v2 schema identifier.
pub const PROCEDURE_SCHEMA_V2: &str = "podway.procedure/v2";

const fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}

#[cfg(test)]
mod tests;
