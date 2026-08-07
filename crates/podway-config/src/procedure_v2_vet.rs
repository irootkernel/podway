//! Mandatory graph-wide semantic analysis for Procedure v2 (dossier section 11.3).
//!
//! **This is a seam, not an engine.** The rule set is empty until V2GRF-001 and V2GRF-002 land it;
//! `procedure vet` itself stays a reserved contract route until then, and section 11.5's aggregate
//! check calls this hook as its third stage so that the day the rules arrive, no caller changes.
//!
//! The contract every future rule inherits, stated here because the empty implementation cannot
//! demonstrate it:
//!
//! - **Deterministic.** The same validated model and the same source yield byte-identical findings,
//!   in the same order, on every run and under any allocator. Rules iterate model slices and
//!   ordered maps, never a hash-map iteration order.
//! - **Sorted by the shared key.** The returned vector is ordered the way every other authoring
//!   stage orders its findings — `(line, column, code, field)` — so
//!   [`crate::finalize_diagnostics`] only has to interleave stages, and a rule's own emission order
//!   survives a full tie.
//! - **Only the vet subset of the catalog.** Vet emits the graph-semantic codes of
//!   `assets/specifications/authoring-diagnostics.json` that no other stage owns — the reachability,
//!   terminal-path, cycle, dominance, skip, and budget findings of section 11.3. It never emits a
//!   schema, source-construct, formatting, or advisory-lint code: those belong to stages that have
//!   already run by the time vet does.
//! - **Validated models only.** Section 11.3's checks resolve placements against definitions and
//!   routes against options; the parameter type is the guarantee that resolution already succeeded.
//!
//! No rule registry is declared yet on purpose. A `VetRule` function-pointer table over an empty
//! slice is dead code under this workspace's `clippy::warnings = "deny"`, so V2GRF introduces the
//! registry in the commit that gives it a first rule. The published seam is the signature.

use podway_core::AuthoringDiagnostic;

use crate::ValidatedProcedureV2;
use crate::procedure_v2_diagnostics::AuthoringContext;

/// Runs the section 11.3 graph-semantic rule set over a validated Procedure v2 model.
///
/// Returns no findings today: the rule set arrives with V2GRF-001 and V2GRF-002. Callers must
/// treat an empty result as "vet found nothing", which is exactly what it will mean afterwards —
/// the aggregate check therefore needs no change when the rules land.
pub fn vet_procedure_v2(
    _validated: &ValidatedProcedureV2,
    _context: &AuthoringContext<'_>,
) -> Vec<AuthoringDiagnostic> {
    Vec::new()
}
