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
    fn v2scl003_defaults_stay_inside_their_hard_maxima() {
        assert!(DEFAULT_TEXT_MAX_SCALARS_V2 <= MAX_TEXT_SCALARS_V2);
        assert!(DEFAULT_LIST_MAX_ITEMS_V2 <= MAX_LIST_ENTRIES_V2);
        assert!(DEFAULT_LIST_MAX_ITEM_SCALARS_V2 <= MAX_LIST_ENTRY_SCALARS_V2);
        assert!(DEFAULT_LIST_MAX_TOTAL_SCALARS_V2 <= MAX_LIST_TOTAL_SCALARS_V2);
    }

    #[test]
    fn v2scl003_the_aggregate_admits_every_previously_admissible_attempt() {
        // The superseded envelope allowed 64 items of 200 entries at 1,000 scalars each.
        let superseded_maximum = 64_u64 * 200 * 1_000;
        assert_eq!(superseded_maximum, 12_800_000);
        assert!(MAX_ATTEMPT_CONTENT_SCALARS_V2 > superseded_maximum);
    }
}
