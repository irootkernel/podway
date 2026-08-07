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

/// The maximum number of Unicode characters a Procedure v2 canonical source projection may hold
/// (dossier section 5.1, "source document input / canonical source projection / nesting depth /
/// parsed nodes"). The projection is the model-derived authoring-shaped document produced by
/// canonicalization; the bound is enforced where that projection is built, and exceeding it is
/// reported as `SOURCE_PROJECTION_BUDGET_EXCEEDED` (sections 11.1, 11.2) by the production mapping
/// in `podway-config`'s `procedure_v2_diagnostics::config_error_diagnostic`.
pub const SOURCE_PROJECTION_MAX_CHARACTERS: usize = 131_072;

const fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}

#[cfg(test)]
mod tests;
