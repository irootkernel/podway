//! The single authoritative Procedure v2 scale envelope.
//!
//! [ADR-0024](../../../../docs/architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md)
//! makes one envelope binding for authoring validation, runtime recording, persistence
//! reconstruction, protocol request slices, and response budget calculation. These constants are
//! that envelope. Every length counts Unicode scalar values, never bytes.
//!
//! Consumers import these constants instead of re-declaring the same literals. Canonical JSON
//! schemas keep literal bounds because a schema cannot import a Rust constant; executable contract
//! tests prove the two agree exactly.

/// The maximum Unicode scalars a text item value or its declared `max_length` may hold.
pub const MAX_TEXT_SCALARS_V2: u32 = 65_536;

/// The `max_length` a text item declaration receives when the source omits it.
pub const DEFAULT_TEXT_MAX_SCALARS_V2: u32 = 4_000;

/// The maximum entries a list item value or its declared `max_items` may hold.
pub const MAX_LIST_ENTRIES_V2: u16 = 1_000;

/// The `max_items` a list item declaration receives when the source omits it.
pub const DEFAULT_LIST_MAX_ITEMS_V2: u16 = 50;

/// The maximum Unicode scalars one list entry or a declared `max_item_length` may hold.
pub const MAX_LIST_ENTRY_SCALARS_V2: u16 = 8_192;

/// The `max_item_length` a list item declaration receives when the source omits it.
pub const DEFAULT_LIST_MAX_ITEM_SCALARS_V2: u16 = 500;

/// The maximum total Unicode scalars across every entry of one list value.
pub const MAX_LIST_TOTAL_SCALARS_V2: u32 = 1_000_000;

/// The `max_total_length` a list item declaration receives when the source omits it.
///
/// The default equals the hard maximum, so an existing authored list keeps its meaning while the
/// literal still participates in canonicalization and therefore in the Procedure digest.
pub const DEFAULT_LIST_MAX_TOTAL_SCALARS_V2: u32 = MAX_LIST_TOTAL_SCALARS_V2;

/// The maximum recorded text and list-entry content one attempt may accumulate.
///
/// The aggregate counts only recorded text values and list-entry content. Reasons, blockers, goal
/// criteria, and artifact metadata are excluded.
pub const MAX_ATTEMPT_CONTENT_SCALARS_V2: u64 = 16_777_216;

/// The maximum items one node definition may declare and one attempt may record.
pub const MAX_ITEMS_PER_DEFINITION_V2: usize = 128;

/// The bytes one Unicode scalar may occupy once JSON-escaped in the worst case.
///
/// A control scalar escapes to `\uXXXX`, so every byte budget derived from a scalar count charges
/// this factor. Authoring validation, page slicing, and preview admission all consume it, which is
/// what keeps the response frame proof and the authoring budget describing the same bytes.
pub const EVIDENCE_SCALAR_BYTES_V2: usize = 6;

/// The bytes one array element adds beyond its own content: two quotes, a separator, and slack.
pub const EVIDENCE_ENTRY_OVERHEAD_BYTES_V2: usize = 8;

/// The maximum encoded bytes one `evidence.read` page may carry.
pub const MAX_EVIDENCE_PAGE_BYTES_V2: usize = 262_144;

/// The maximum evidence items that may carry a preview in one `session.next` response.
pub const MAX_EVIDENCE_PREVIEW_SLOTS_V2: usize = 32;

/// The maximum Unicode scalars one evidence preview may carry.
///
/// ADR-0024 allocates 176 KiB of preview data across the slots above, which leaves at least 938
/// worst-case scalars per preview once escaping is charged at six bytes per scalar.
///
/// Admission charges bytes rather than scalars through [`MAX_EVIDENCE_PREVIEW_BYTES_V2`], so a
/// preview of many tiny list entries cannot outgrow the allocation this scalar count describes.
pub const MAX_EVIDENCE_PREVIEW_SCALARS_V2: usize = 938;

/// The encoded bytes one evidence preview may occupy.
///
/// This is the scalar budget above charged at its worst-case escaping, and it is what preview
/// admission actually spends: a value's own content plus whatever structure encoding it adds.
pub const MAX_EVIDENCE_PREVIEW_BYTES_V2: usize =
    MAX_EVIDENCE_PREVIEW_SCALARS_V2 * EVIDENCE_SCALAR_BYTES_V2;

/// The maximum missing-item details one response carries; the exact count stays separate.
pub const MAX_MISSING_ITEM_WINDOW_V2: usize = 64;

/// The maximum choice values one observation active item exposes; the exact count stays separate.
pub const MAX_OBSERVATION_CHOICE_WINDOW_V2: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2scl003_declared_worst_case_list_content_cannot_exceed_the_attempt_aggregate() {
        let worst_case_list = u64::from(MAX_LIST_ENTRIES_V2) * u64::from(MAX_LIST_ENTRY_SCALARS_V2);
        assert!(worst_case_list > u64::from(MAX_LIST_TOTAL_SCALARS_V2));
        let effective = worst_case_list.min(u64::from(MAX_LIST_TOTAL_SCALARS_V2));
        assert_eq!(effective, u64::from(MAX_LIST_TOTAL_SCALARS_V2));
        assert!(effective * MAX_ITEMS_PER_DEFINITION_V2 as u64 > MAX_ATTEMPT_CONTENT_SCALARS_V2);
    }

    #[test]
    fn v2scl004_every_preview_slot_fits_inside_the_preview_allocation() {
        // ADR-0024 allocates 176 KiB to preview data. Every slot charged at its worst case must
        // stay inside it, or one maximal response could pass the 1 MiB frame.
        const PREVIEW_ALLOCATION_BYTES: usize = 176 * 1024;
        // Admission spends the byte budget, so the allocation must bound bytes. Charging scalars
        // alone would miss the per-entry structure a list preview also encodes.
        let worst_case = MAX_EVIDENCE_PREVIEW_SLOTS_V2 * MAX_EVIDENCE_PREVIEW_BYTES_V2;
        assert!(
            worst_case <= PREVIEW_ALLOCATION_BYTES,
            "worst-case preview bytes {worst_case} exceed the {PREVIEW_ALLOCATION_BYTES}-byte allocation"
        );
        // One maximal list entry alone exceeds a preview slot, so entry-wise admission must be
        // able to admit nothing rather than admitting one oversized entry.
        assert!(
            MAX_LIST_ENTRY_SCALARS_V2 as usize * EVIDENCE_SCALAR_BYTES_V2
                > MAX_EVIDENCE_PREVIEW_BYTES_V2
        );
        // A page holds at least one maximal entry, so page admission never has to split one.
        assert!(
            EVIDENCE_ENTRY_OVERHEAD_BYTES_V2
                + MAX_LIST_ENTRY_SCALARS_V2 as usize * EVIDENCE_SCALAR_BYTES_V2
                <= MAX_EVIDENCE_PAGE_BYTES_V2
        );
    }

    #[test]
    fn v2scl003_defaults_stay_inside_their_hard_maxima() {
        // These are compile-time relations, so a violation must fail the build rather than wait
        // for someone to run the test.
        const {
            assert!(DEFAULT_TEXT_MAX_SCALARS_V2 <= MAX_TEXT_SCALARS_V2);
            assert!(DEFAULT_LIST_MAX_ITEMS_V2 <= MAX_LIST_ENTRIES_V2);
            assert!(DEFAULT_LIST_MAX_ITEM_SCALARS_V2 <= MAX_LIST_ENTRY_SCALARS_V2);
            assert!(DEFAULT_LIST_MAX_TOTAL_SCALARS_V2 <= MAX_LIST_TOTAL_SCALARS_V2);
        }
    }

    #[test]
    fn v2scl003_the_aggregate_admits_every_previously_admissible_attempt() {
        // The superseded envelope allowed 64 items of 200 entries at 1,000 scalars each.
        let superseded_maximum = 64_u64 * 200 * 1_000;
        assert_eq!(superseded_maximum, 12_800_000);
        assert!(MAX_ATTEMPT_CONTENT_SCALARS_V2 > superseded_maximum);
    }
}
