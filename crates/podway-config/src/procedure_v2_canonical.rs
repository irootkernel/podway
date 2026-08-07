//! Procedure v2 canonicalization: the model-derived canonical document, its Canonical JSON v1
//! bytes, and the digest computed over them (dossier section 12.1).
//!
//! The authority hierarchy section 12.1 fixes is `YAML or JSON source -> parsed Procedure v2 model
//! -> Canonical JSON/IR -> digest`. This module owns the third arrow. Canonical form is therefore
//! never a reformatting of the source document: it is rebuilt from the validated model through its
//! public accessors, so anything the source could carry but the model does not — comments, key
//! order, block versus flow style, quoting, and omitted-versus-explicit defaults — is gone before
//! the first byte is written, and section 13.3's promise that "formatting and comments never affect
//! it" holds by construction rather than by normalization rules.
//!
//! The canonical document is authoring-shaped: it is the same closed shape
//! `assets/schemas/procedure-v2.schema.json` fixes for source documents, so a canonical document is
//! itself a valid Procedure v2 document that re-parses to the same model (the fixpoint the
//! canonical golden test asserts). Four rules fix the shape exactly:
//!
//! 1. Documented defaults are materialized. Every default `procedure_v2_wire` declares through
//!    `serde` — text `min_length`/`max_length`/`multiline`, list `min_items`/`max_items`/
//!    `max_item_length`/`unique`, and evidence-reference `required` — appears explicitly, so
//!    omitting a default and authoring it produce identical bytes.
//! 2. Absent optional scalars and empty optional collections are omitted. The schema gives every
//!    optional collection `minItems: 1` and the parser rejects an explicitly empty one, so an
//!    empty collection has no representable canonical form; `goal_tracking` is present exactly when
//!    the procedure opted in, mirroring its `const: true` schema shape.
//! 3. Author-order-meaningful arrays keep author order: `graph.nodes`, `options`, `items`,
//!    `instructions`, `evidence_guidance`, `evidence_from`, selected evidence `items`, `choices`,
//!    `allowed_media_types`, and `manual_rework.allowed_targets`. Reordering any of them is a
//!    semantic edit and changes the digest.
//! 4. Authoring maps stay maps: `node_definitions`, decision `routes`, and assessment `outcomes`
//!    become JSON objects, so Canonical JSON v1's byte-sorted key order normalizes them away and
//!    reordering their keys cannot change the digest.
//!
//! Those four rules live in `procedure_v2_document`, not here: the same tree feeds the canonical
//! digest and the authoring formatter, so the two cannot disagree about what the document contains.
//! This module adds only the projection bound, the Canonical JSON v1 bytes, and the digest.
//!
//! Numbers are integers only, which Canonical JSON v1 requires; the v2 model has no non-integer
//! number.

use podway_core::{SOURCE_PROJECTION_MAX_CHARACTERS, Sha256Digest};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::procedure_v2_document::authoring_document_value;
use crate::{CanonicalJsonV1, ConfigError, ParsedProcedureV2, canonical_json_from_serializable};

/// The `ConfigError` field name the canonical source projection bound reports.
///
/// Shared by the bound check below and by `procedure_v2_diagnostics`, which switches on it to emit
/// `SOURCE_PROJECTION_BUDGET_EXCEEDED` rather than the generic schema code. Two literals could
/// drift; one constant cannot.
pub(crate) const CANONICAL_PROJECTION_FIELD: &str = "canonical source projection";

/// Builds the canonical bytes and digest of an already closed-reference-validated model.
///
/// The projection bound of section 5.1 is enforced here because this is where the projection first
/// exists: no earlier stage can measure a document that is only produced by canonicalization. The
/// future stable diagnostic code for this rejection is `SOURCE_PROJECTION_BUDGET_EXCEEDED`
/// (sections 11.1 and 11.2); binding `ConfigError` values to the catalog in
/// `assets/specifications/authoring-diagnostics.json` is V2AUT-008's task.
pub(crate) fn canonical_projection(
    parsed: &ParsedProcedureV2,
) -> Result<(CanonicalJsonV1, Sha256Digest), ConfigError> {
    let canonical_json = canonical_json_from_serializable(&canonical_document_value(parsed))?;
    crate::validate_count(
        CANONICAL_PROJECTION_FIELD,
        canonical_json.as_str().chars().count(),
        1,
        SOURCE_PROJECTION_MAX_CHARACTERS,
    )?;
    let digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_json.as_bytes())
    ))
    .map_err(|_| ConfigError::InvalidDigest)?;
    Ok((canonical_json, digest))
}

/// Projects the authoring document into `serde_json::Value` for canonicalization.
///
/// Canonical JSON v1 byte-sorts object keys, so the author map order the authoring tree carries is
/// erased here and the digest is unchanged by it — the property that lets the formatter preserve
/// author order without ever moving the digest.
pub(crate) fn canonical_document_value(parsed: &ParsedProcedureV2) -> Value {
    authoring_document_value(parsed).into_json()
}
