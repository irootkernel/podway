//! Conservative Procedure v2 `next` component accounting.
//!
//! ADR-0024 closes `session.next` at a fixed set of named byte allocations that sum to the 1 MiB
//! frame. This module charges the five of them whose size depends on one graph placement; the
//! sixth is the serialization reserve, which no placement can spend. It mirrors the canonical
//! `next-result-v3` and shared result-component schemas: a bounded string costs six bytes per
//! Unicode scalar, every object field reserves 64 bytes, and every array element reserves another
//! eight bytes. All arithmetic saturates so future bound increases cannot turn a resource
//! rejection into an under-count through integer overflow.
//!
//! The allocations are charged separately because they fail separately. An author who selects too
//! much evidence must be told which allocation they exceeded, not that "next is too big".

use podway_core::{GraphNodeId, GraphPlacementV2, ItemSpecV2};

use crate::procedure_v2_authoring::{placement_definition_id, placement_evidence_from};
use crate::{ParsedNodeDefinition, ParsedProcedureV2, ValidatedProcedureV2};

/// Maximum procedure-snapshot-derived and runtime static content in one `next` result.
pub const NEXT_STATIC_BUDGET: u64 = 256 * 1_024;

/// Maximum complete decision-record content carried by one `next` result.
///
/// One complete goal-assessment record is the widest single object `next` carries. The allocation
/// holds exactly one, which is what bounds a placement to one decision source.
pub const DECISION_RECORD_BUDGET: u64 = 264 * 1_024;

/// Maximum reference and item metadata in one `next` result, across at most 128 items.
///
/// ADR-0024 removed complete values from progression guidance, so this charges identity, digest,
/// and size per selected item rather than the value itself.
pub const READBACK_BUDGET: u64 = 216 * 1_024;

/// Maximum preview data in one `next` result, across at most 32 preview slots.
pub const EVIDENCE_PREVIEW_BUDGET: u64 = 176 * 1_024;

/// Maximum continuation-token content in one `next` result, across at most 32 tokens.
pub const PAGE_TOKEN_BUDGET: u64 = 64 * 1_024;

/// Bytes reserved for serialization and the outer envelope.
///
/// No placement charges against this: it exists so the five charged allocations plus the envelope
/// they travel in provably fit the frame.
pub const NEXT_SERIALIZATION_RESERVE: u64 = 48 * 1_024;

/// The frame payload the six allocations must exactly fill.
pub const NEXT_FRAME_BUDGET: u64 = 1_024 * 1_024;

/// The item metadata records one `next` result may carry across every reference.
///
/// `next-result/v3` publishes this ceiling, and no byte allocation implies it, so authoring has to
/// enforce it directly.
pub const READBACK_RECORD_BUDGET: u64 = podway_core::MAX_ITEMS_PER_DEFINITION_V2 as u64;

/// Worst-case escaped bytes per Unicode scalar, owned by the domain so the authoring charge and
/// the runtime page and preview bounds describe the same bytes.
const STRING_BYTE_FACTOR: u64 = podway_core::EVIDENCE_SCALAR_BYTES_V2 as u64;
const FIELD_OVERHEAD: u64 = 64;
const ARRAY_ELEMENT_OVERHEAD: u64 = podway_core::EVIDENCE_ENTRY_OVERHEAD_BYTES_V2 as u64;
const MAX_IDENTIFIER_CHARS: u64 = 64;
const MAX_DEFINITION_TITLE_CHARS: u64 = 120;
const MAX_UUID_CHARS: u64 = 36;
const MAX_DIGEST_CHARS: u64 = 71;
const MAX_TIMESTAMP_CHARS: u64 = 24;
const MAX_RECORD_REASON_CHARS: u64 = 2_000;
const MAX_ACTOR_CHARS: u64 = 256;
const MAX_U64_BYTES: u64 = 20;
const MAX_BOOL_BYTES: u64 = 5;
const MAX_GOAL_CRITERIA: u64 = 16;
const MAX_CRITERION_REASON_CHARS: u64 = 2_000;
const MAX_CITATIONS_PER_CRITERION: u64 = 4;
const MAX_RECORD_REFERENCES: u64 = 8;

const ALL_ALLOWED_ACTIONS: &[&str] = &[
    "session.complete",
    "session.decide",
    "session.retry",
    "session.skip",
    "session.rework",
    "session.block",
    "session.unblock",
    "session.cancel",
    "session.reset",
    "goal.define",
    "goal.revise",
    "goal.assess_criterion",
    "item.check",
    "item.uncheck",
    "item.set",
    "item.add",
    "item.remove",
    "item.attach",
    "item.clear",
];

/// Conservative wire-size charges for one validated Procedure v2 graph placement.
///
/// These are the production charges used by vetting, exposed so diagnostics and contract tests can
/// compare admitted procedure content with the corresponding serialized runtime projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcedurePlacementBudgetV2 {
    pub(crate) next_static: u64,
    pub(crate) decision_records: u64,
    pub(crate) readback: u64,
    pub(crate) readback_records: u64,
    pub(crate) evidence_preview: u64,
    pub(crate) page_tokens: u64,
}

impl ProcedurePlacementBudgetV2 {
    /// Conservative charge for procedure-snapshot-derived `session.next` content.
    pub const fn next_static(&self) -> u64 {
        self.next_static
    }

    /// Conservative charge for the complete decision records this placement can carry.
    pub const fn decision_records(&self) -> u64 {
        self.decision_records
    }

    /// Conservative charge for evidence reference and item metadata.
    pub const fn readback(&self) -> u64 {
        self.readback
    }

    /// The item metadata records this placement can publish in one response.
    ///
    /// The byte allocation alone does not bound this: metadata is value-independent, so many small
    /// items cost little while still exceeding the record ceiling the result schema publishes.
    pub const fn readback_records(&self) -> u64 {
        self.readback_records
    }

    /// Conservative charge for bounded evidence previews.
    pub const fn evidence_preview(&self) -> u64 {
        self.evidence_preview
    }

    /// Conservative charge for the continuation tokens previews publish.
    pub const fn page_tokens(&self) -> u64 {
        self.page_tokens
    }

    /// The five charged allocations together.
    pub const fn charged_total(&self) -> u64 {
        self.next_static
            .saturating_add(self.decision_records)
            .saturating_add(self.readback)
            .saturating_add(self.evidence_preview)
            .saturating_add(self.page_tokens)
    }
}

/// Returns the production wire-size charges for a placement in a validated Procedure v2 model.
///
/// `None` means `graph_node_id` does not identify a placement in the validated graph. Validation
/// guarantees that every returned placement has closed definition and evidence references.
pub fn procedure_placement_budget_v2(
    procedure: &ValidatedProcedureV2,
    graph_node_id: &GraphNodeId,
) -> Option<ProcedurePlacementBudgetV2> {
    procedure
        .parsed()
        .graph()
        .placements()
        .iter()
        .find(|placement| placement.id() == graph_node_id)
        .map(|placement| placement_budget(procedure.parsed(), placement))
}

pub(crate) fn placement_budget(
    procedure: &ParsedProcedureV2,
    placement: &GraphPlacementV2,
) -> ProcedurePlacementBudgetV2 {
    let definition = procedure
        .node_definitions()
        .iter()
        .find(|candidate| candidate.id().as_str() == placement_definition_id(placement));
    let Some(definition) = definition else {
        // Closed-reference validation makes this unreachable. Saturation preserves fail-closed
        // behavior if a caller ever violates the validated-model precondition.
        return ProcedurePlacementBudgetV2 {
            next_static: u64::MAX,
            decision_records: u64::MAX,
            readback: u64::MAX,
            readback_records: u64::MAX,
            evidence_preview: u64::MAX,
            page_tokens: u64::MAX,
        };
    };
    let evidence = evidence_charges(procedure, placement);
    ProcedurePlacementBudgetV2 {
        next_static: next_static_charge(procedure, placement, definition),
        decision_records: evidence.decision_records,
        readback: evidence.metadata,
        readback_records: evidence.metadata_records,
        evidence_preview: evidence.preview,
        page_tokens: evidence.page_tokens,
    }
}

pub(crate) const fn exceeds_budget(charged: u64, budget: u64) -> bool {
    charged > budget
}

fn next_static_charge(
    procedure: &ParsedProcedureV2,
    placement: &GraphPlacementV2,
    definition: &ParsedNodeDefinition,
) -> u64 {
    // `goal_tracking` is a required `next` field derived directly from the immutable snapshot.
    // `goal_defined` is runtime state and belongs to GOAL_DISPLAY_MAX instead.
    let mut charge = fixed_field(MAX_BOOL_BYTES);
    let items = match definition {
        ParsedNodeDefinition::Action(action) => {
            charge = add(charge, string_field(actual_chars(action.title())));
            charge = add(charge, string_field(actual_chars(action.intent())));
            if let Some(description) = action.description() {
                charge = add(charge, string_field(actual_chars(description)));
            }
            charge = add(charge, array_field());
            for instruction in action.instructions() {
                charge = add(charge, array_string(actual_chars(instruction)));
            }
            action.items()
        }
        ParsedNodeDefinition::Decision(decision) => {
            charge = add(charge, string_field(actual_chars(decision.title())));
            if let Some(description) = decision.description() {
                charge = add(charge, string_field(actual_chars(description)));
            }
            charge = add(charge, string_field(actual_chars(decision.objective())));
            charge = add(charge, string_field(actual_chars(decision.prompt())));

            // The reason object itself and its required boolean are always present.
            charge = add(charge, field());
            charge = add(charge, fixed_field(4));
            if let Some(prompt) = decision.reason().prompt() {
                charge = add(charge, string_field(actual_chars(prompt)));
            }

            charge = add(charge, array_field());
            for option in decision.options() {
                charge = add(charge, ARRAY_ELEMENT_OVERHEAD);
                charge = add(charge, string_field(actual_chars(option.id().as_str())));
                charge = add(charge, string_field(actual_chars(option.label())));
                if let Some(criteria) = option.criteria() {
                    charge = add(charge, string_field(actual_chars(criteria)));
                }
            }
            if !decision.evidence_guidance().is_empty() {
                charge = add(charge, array_field());
                for guidance in decision.evidence_guidance() {
                    charge = add(charge, array_string(actual_chars(guidance)));
                }
            }
            decision.items()
        }
    };

    // At some reachable state every required item may be missing simultaneously. The compact
    // count is a separate required result field from the optional item-detail array.
    charge = add(charge, fixed_field(3)); // missing_required_item_count reaches 128
    charge = add(charge, array_field());
    for item in items.iter().filter(|item| item.common().required()) {
        charge = add(charge, ARRAY_ELEMENT_OVERHEAD);
        charge = add(charge, string_field(actual_chars(item.id().as_str())));
        charge = add(charge, string_field(actual_chars(item.common().prompt())));
    }

    match placement {
        GraphPlacementV2::Action(action) => {
            match action.outcome().next_target() {
                Some(target) => {
                    charge = add(charge, string_field(actual_chars(target.as_str())));
                }
                None => charge = add(charge, fixed_field(4)),
            }
            if action.skip().is_some() {
                charge = add(charge, field());
                charge = add(charge, fixed_field(4));
                charge = add(charge, fixed_field(MAX_BOOL_BYTES));
            }
        }
        GraphPlacementV2::Decision(_) => {}
    }

    charge = add(charge, array_field());
    if let Some(rework) = procedure.graph().manual_rework() {
        for target in rework.targets() {
            charge = add(charge, array_string(actual_chars(target.as_str())));
        }
    }

    // The closed command inventory is a small conservative superset of every state-specific set.
    charge = add(charge, array_field());
    for command in ALL_ALLOWED_ACTIONS {
        charge = add(charge, array_string(actual_chars(command)));
    }

    charge = add(charge, suggestion_charge(placement, definition));
    charge
}

fn suggestion_charge(placement: &GraphPlacementV2, definition: &ParsedNodeDefinition) -> u64 {
    let mut charge = array_field();
    let items = match definition {
        ParsedNodeDefinition::Action(action) => action.items(),
        ParsedNodeDefinition::Decision(decision) => decision.items(),
    };
    for item in items.iter().filter(|item| item.common().required()) {
        if matches!(item, ItemSpecV2::CheckResult(_)) {
            charge = add(
                charge,
                suggestion(
                    "item.record_many",
                    &["record", "--stdin"],
                    Some(item.id().as_str()),
                ),
            );
            continue;
        }
        let (command, verb, placeholder) = match item {
            ItemSpecV2::Confirm(_) => ("item.check", "check", None),
            ItemSpecV2::Text(_) => ("item.set", "set", Some("<text>")),
            ItemSpecV2::Choice(_) => ("item.set", "set", Some("<choice>")),
            ItemSpecV2::Integer(_) => ("item.set", "set", Some("<integer>")),
            ItemSpecV2::List(_) => ("item.add", "add", Some("<value>")),
            ItemSpecV2::Artifact(_) => ("item.attach", "attach", Some("<path>")),
            ItemSpecV2::CheckResult(_) => unreachable!("handled above"),
        };
        let mut argv = vec![verb, item.id().as_str()];
        if let Some(placeholder) = placeholder {
            argv.push(placeholder);
        }
        charge = add(charge, suggestion(command, &argv, Some(item.id().as_str())));
    }

    // Charging mutually exclusive states together is a deliberate, bounded over-approximation.
    charge = add(
        charge,
        suggestion("session.retry", &["retry", "--reason", "<reason>"], None),
    );
    match (placement, definition) {
        (GraphPlacementV2::Action(action), ParsedNodeDefinition::Action(_)) => {
            charge = add(charge, suggestion("session.complete", &["complete"], None));
            if action.skip().is_some() {
                charge = add(
                    charge,
                    suggestion("session.skip", &["skip", "--reason", "<text>"], None),
                );
            }
        }
        (GraphPlacementV2::Decision(_), ParsedNodeDefinition::Decision(decision)) => {
            for option in decision.options() {
                charge = add(
                    charge,
                    suggestion(
                        "session.decide",
                        &[
                            "decide",
                            "--option",
                            option.id().as_str(),
                            "--reason",
                            "<reason>",
                        ],
                        None,
                    ),
                );
            }
        }
        // Kind agreement is a validation precondition. Saturate rather than under-count if broken.
        _ => return u64::MAX,
    }
    charge
}

fn suggestion(command: &str, argv: &[&str], item_id: Option<&str>) -> u64 {
    let mut charge = ARRAY_ELEMENT_OVERHEAD;
    charge = add(charge, string_field(actual_chars(command)));
    charge = add(charge, array_field());
    // Section 10.1 reuses the current JSON-contract v1 argv shape, whose first element is always the
    // executable name even though callers do not type it as a subcommand argument.
    charge = add(charge, array_string(actual_chars("podway")));
    for argument in argv {
        charge = add(charge, array_string(actual_chars(argument)));
    }
    if let Some(item_id) = item_id {
        charge = add(charge, string_field(actual_chars(item_id)));
    }
    charge
}
/// The four evidence-derived allocations one placement can spend.
///
/// They are charged together because they walk the same declaration once, and reported separately
/// because ADR-0024 gives each its own ceiling and its own diagnostic.
struct EvidenceChargesV2 {
    decision_records: u64,
    metadata: u64,
    metadata_records: u64,
    preview: u64,
    page_tokens: u64,
}

fn evidence_charges(
    procedure: &ParsedProcedureV2,
    placement: &GraphPlacementV2,
) -> EvidenceChargesV2 {
    // Both arrays are required by next-result-v3. Their elements repeat the reference metadata.
    let mut metadata = add(array_field(), array_field());
    let mut metadata_records = 0u64;
    let mut decision_records = 0u64;
    let mut previewable = 0u64;
    let Some(evidence) = placement_evidence_from(placement) else {
        return EvidenceChargesV2 {
            decision_records,
            metadata,
            metadata_records: 0,
            preview: 0,
            page_tokens: 0,
        };
    };

    for reference in evidence.entries() {
        let source = procedure
            .graph()
            .placements()
            .iter()
            .find(|candidate| candidate.id() == reference.source_node());
        let Some(source) = source else {
            return EvidenceChargesV2 {
                decision_records: u64::MAX,
                metadata: u64::MAX,
                metadata_records: u64::MAX,
                preview: u64::MAX,
                page_tokens: u64::MAX,
            };
        };
        let definition = procedure
            .node_definitions()
            .iter()
            .find(|candidate| candidate.id().as_str() == placement_definition_id(source));
        let Some(definition) = definition else {
            return EvidenceChargesV2 {
                decision_records: u64::MAX,
                metadata: u64::MAX,
                metadata_records: u64::MAX,
                preview: u64::MAX,
                page_tokens: u64::MAX,
            };
        };

        let items = match definition {
            ParsedNodeDefinition::Action(action) => action.items(),
            ParsedNodeDefinition::Decision(decision) => decision.items(),
        };
        // `references` and `readback` each repeat the reference metadata for this source.
        metadata = add(metadata, reference_metadata_charge());
        metadata = add(metadata, reference_metadata_charge());
        metadata = add(metadata, array_field());

        let selected: Vec<&ItemSpecV2> = match reference.selected_items() {
            Some(ids) => {
                let mut selected = Vec::with_capacity(ids.len());
                for id in ids {
                    let Some(item) = items.iter().find(|item| item.id() == id) else {
                        return EvidenceChargesV2 {
                            decision_records: u64::MAX,
                            metadata: u64::MAX,
                            metadata_records: u64::MAX,
                            preview: u64::MAX,
                            page_tokens: u64::MAX,
                        };
                    };
                    selected.push(item);
                }
                // Only an explicit selector receives previews; an omitted one means metadata-only,
                // so it spends no preview slot and issues no continuation token.
                previewable = previewable.saturating_add(selected.len() as u64);
                selected
            }
            None => items.iter().collect(),
        };
        metadata_records = add(metadata_records, selected.len() as u64);
        for item in selected {
            metadata = add(metadata, readback_item_charge(item));
        }

        if let ParsedNodeDefinition::Decision(decision) = definition {
            decision_records = add(
                decision_records,
                decision_record_charge(decision.assessment().is_some()),
            );
        }
    }

    // Slots and tokens are response-wide, not per-reference: the projection stops at the budget
    // however many references asked for one.
    let slots = previewable.min(podway_core::MAX_EVIDENCE_PREVIEW_SLOTS_V2 as u64);
    EvidenceChargesV2 {
        decision_records,
        metadata,
        metadata_records,
        preview: slots.saturating_mul(podway_core::MAX_EVIDENCE_PREVIEW_BYTES_V2 as u64),
        page_tokens: slots.saturating_mul(page_token_charge()),
    }
}

fn reference_metadata_charge() -> u64 {
    let mut charge = ARRAY_ELEMENT_OVERHEAD;
    charge = add(charge, string_field(MAX_IDENTIFIER_CHARS));
    charge = add(charge, string_field(MAX_DEFINITION_TITLE_CHARS));
    charge = add(charge, string_field(MAX_UUID_CHARS));
    charge = add(charge, fixed_field(MAX_U64_BYTES));
    charge = add(charge, string_field(MAX_DIGEST_CHARS));
    add(charge, string_field(actual_chars("resolved")))
}

/// One continuation token at its published maximum length.
fn page_token_charge() -> u64 {
    string_field(podway_core::MAX_EVIDENCE_PAGE_TOKEN_CHARS_V2 as u64)
}

/// One item's evidence metadata in `next-result/v3`.
///
/// ADR-0024 replaced the complete value with identity, digest, and size, so this charge no longer
/// depends on how much the item may hold: a 65,536-scalar text and a confirm cost the same here.
/// What the value costs is charged against the preview and page allocations instead.
fn readback_item_charge(item: &ItemSpecV2) -> u64 {
    let kind = match item {
        ItemSpecV2::Confirm(_) => "confirm",
        ItemSpecV2::Text(_) => "text",
        ItemSpecV2::Choice(_) => "choice",
        ItemSpecV2::Integer(_) => "integer",
        ItemSpecV2::List(_) => "list",
        ItemSpecV2::Artifact(_) => "artifact",
        ItemSpecV2::CheckResult(_) => "check_result",
    };
    let mut charge = ARRAY_ELEMENT_OVERHEAD;
    charge = add(charge, string_field(MAX_IDENTIFIER_CHARS)); // item_id
    charge = add(charge, string_field(actual_chars(kind))); // type
    charge = add(charge, fixed_field(MAX_U64_BYTES)); // revision
    charge = add(charge, string_field(MAX_DIGEST_CHARS)); // value_digest
    charge = add(charge, fixed_field(MAX_U64_BYTES)); // total_size
    charge = add(charge, string_field(actual_chars("scalars"))); // size_unit
    add(charge, fixed_field(MAX_BOOL_BYTES)) // preview_truncated
}

fn decision_record_charge(is_goal_assessment: bool) -> u64 {
    let mut charge = field(); // the optional `decision_record` object field
    charge = add(charge, fixed_field(MAX_U64_BYTES)); // trace_sequence
    charge = add(charge, string_field(MAX_UUID_CHARS)); // session_id
    charge = add(charge, fixed_field(MAX_U64_BYTES)); // session_revision
    charge = add(charge, string_field(actual_chars("podway.procedure/v2")));
    charge = add(charge, string_field(MAX_UUID_CHARS)); // procedure_snapshot_id
    charge = add(charge, string_field(MAX_DIGEST_CHARS));
    charge = add(charge, string_field(MAX_IDENTIFIER_CHARS)); // graph_node_id
    charge = add(charge, string_field(MAX_IDENTIFIER_CHARS)); // node_definition_id
    charge = add(charge, string_field(MAX_UUID_CHARS)); // attempt_id
    charge = add(charge, fixed_field(MAX_U64_BYTES)); // attempt_number
    charge = add(charge, fixed_field(MAX_U64_BYTES)); // goal_revision, choosing non-null
    charge = add(charge, string_field(MAX_IDENTIFIER_CHARS)); // option_id
    charge = add(charge, string_field(actual_chars("advance"))); // longest effect
    charge = add(charge, string_field(MAX_IDENTIFIER_CHARS)); // target_graph_node_id
    charge = add(charge, string_field(MAX_RECORD_REASON_CHARS));
    charge = add(charge, string_field(MAX_ACTOR_CHARS));
    charge = add(charge, string_field(MAX_TIMESTAMP_CHARS));

    charge = add(charge, array_field());
    charge = add(
        charge,
        MAX_RECORD_REFERENCES.saturating_mul(reference_snapshot_charge()),
    );
    if is_goal_assessment {
        charge = add(charge, goal_assessment_record_charge());
    }
    charge
}

fn reference_snapshot_charge() -> u64 {
    let mut charge = ARRAY_ELEMENT_OVERHEAD;
    charge = add(charge, string_field(MAX_IDENTIFIER_CHARS));
    charge = add(charge, string_field(MAX_UUID_CHARS));
    charge = add(charge, fixed_field(MAX_U64_BYTES));
    charge = add(charge, string_field(MAX_DIGEST_CHARS));
    add(charge, string_field(actual_chars("resolved")))
}

fn goal_assessment_record_charge() -> u64 {
    // `not_achieved` is the largest valid shape: it permits four citations on every criterion,
    // unlike the slightly longer `not_applicable` spelling, whose citations must be empty.
    let mut charge = string_field(actual_chars("session_goal"));
    charge = add(charge, string_field(actual_chars("assessment")));
    charge = add(charge, string_field(actual_chars("not_achieved")));
    charge = add(charge, array_field());
    let criterion = criterion_result_charge();
    add(charge, MAX_GOAL_CRITERIA.saturating_mul(criterion))
}

fn criterion_result_charge() -> u64 {
    let mut charge = ARRAY_ELEMENT_OVERHEAD;
    charge = add(charge, string_field(MAX_IDENTIFIER_CHARS));
    charge = add(charge, string_field(actual_chars("unsatisfied")));
    charge = add(charge, string_field(MAX_CRITERION_REASON_CHARS));
    charge = add(charge, array_field());
    let citation = add(ARRAY_ELEMENT_OVERHEAD, string_field(MAX_IDENTIFIER_CHARS));
    add(charge, MAX_CITATIONS_PER_CRITERION.saturating_mul(citation))
}

const fn add(left: u64, right: u64) -> u64 {
    left.saturating_add(right)
}

const fn field() -> u64 {
    FIELD_OVERHEAD
}

const fn fixed_field(encoded_bytes: u64) -> u64 {
    add(FIELD_OVERHEAD, encoded_bytes)
}

const fn array_field() -> u64 {
    FIELD_OVERHEAD
}

const fn string_bytes(characters: u64) -> u64 {
    characters.saturating_mul(STRING_BYTE_FACTOR)
}

const fn string_field(characters: u64) -> u64 {
    add(FIELD_OVERHEAD, string_bytes(characters))
}

const fn array_string(characters: u64) -> u64 {
    add(ARRAY_ELEMENT_OVERHEAD, string_bytes(characters))
}

fn actual_chars(value: &str) -> u64 {
    u64::try_from(value.chars().count()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
    };
    use podway_core::{ItemCommonV2, ItemId};

    fn common(id: &str) -> ItemCommonV2 {
        ItemCommonV2::new(ItemId::new(id).expect("test item id"), "Prompt", None, true)
            .expect("test item metadata")
    }

    #[test]
    fn accounting_primitives_use_six_sixty_four_and_eight() {
        assert_eq!(string_field(3), 82);
        assert_eq!(array_string(3), 26);
        assert_eq!(fixed_field(20), 84);
    }

    #[test]
    fn accounting_saturates_instead_of_wrapping() {
        assert_eq!(string_bytes(u64::MAX), u64::MAX);
        assert_eq!(string_field(u64::MAX), u64::MAX);
        assert_eq!(add(u64::MAX, 1), u64::MAX);
    }

    #[test]
    fn budget_boundary_accepts_equality_and_rejects_one_byte_over() {
        assert!(!exceeds_budget(NEXT_STATIC_BUDGET, NEXT_STATIC_BUDGET));
        assert!(exceeds_budget(NEXT_STATIC_BUDGET + 1, NEXT_STATIC_BUDGET));
        assert!(!exceeds_budget(READBACK_BUDGET, READBACK_BUDGET));
        assert!(exceeds_budget(READBACK_BUDGET + 1, READBACK_BUDGET));
    }

    fn preset_maxima(source: &[u8]) -> [u64; 5] {
        let ParsedProcedure::V2(parsed) =
            parse_procedure_document(source, ProcedureDocumentFormat::Yaml)
                .expect("a shipped preset must parse");
        let validated = validate_procedure_v2(parsed).expect("a shipped preset must validate");
        let usages: Vec<ProcedurePlacementBudgetV2> = validated
            .parsed()
            .graph()
            .placements()
            .iter()
            .map(|placement| placement_budget(validated.parsed(), placement))
            .collect();
        let maximum = |select: fn(&ProcedurePlacementBudgetV2) -> u64| {
            usages
                .iter()
                .map(select)
                .max()
                .expect("a shipped preset has graph placements")
        };
        [
            maximum(|usage| usage.next_static),
            maximum(|usage| usage.decision_records),
            maximum(|usage| usage.readback),
            maximum(|usage| usage.evidence_preview),
            maximum(|usage| usage.page_tokens),
        ]
    }

    fn assert_preset_fits(name: &str, maxima: [u64; 5]) {
        for (charged, allocation, bucket) in [
            (maxima[0], NEXT_STATIC_BUDGET, "static"),
            (maxima[1], DECISION_RECORD_BUDGET, "decision records"),
            (maxima[2], READBACK_BUDGET, "metadata"),
            (maxima[3], EVIDENCE_PREVIEW_BUDGET, "preview"),
            (maxima[4], PAGE_TOKEN_BUDGET, "page tokens"),
        ] {
            assert!(
                !exceeds_budget(charged, allocation),
                "{name} charges {charged} bytes of {bucket}, over {allocation}"
            );
        }
        let total: u64 = maxima.iter().sum();
        assert!(
            !exceeds_budget(total, NEXT_FRAME_BUDGET - NEXT_SERIALIZATION_RESERVE),
            "{name} charges {total} bytes across every allocation"
        );
    }

    #[test]
    fn v2dog001_sw_dev_preset_records_budget_headroom() {
        // Every shipped preset must sit well inside every allocation. The exact charges are pinned
        // so a change to the charge model shows up here as a number, not as a silent shift.
        let maxima = preset_maxima(include_bytes!("../../../assets/presets/sw-dev-v2.yaml"));
        assert_eq!(maxima, [8_876, 262_186, 29_393, 5_628, 1_600]);
        assert_preset_fits("sw-dev-v2", maxima);
    }

    #[test]
    fn v2dog002_bug_fix_preset_records_budget_headroom() {
        let maxima = preset_maxima(include_bytes!("../../../assets/presets/bug-fix-v2.yaml"));
        assert_eq!(maxima, [9_116, 262_186, 29_393, 5_628, 1_600]);
        assert_preset_fits("bug-fix-v2", maxima);
    }

    #[test]
    fn v2scl005_small_change_preset_records_budget_headroom() {
        let maxima = preset_maxima(include_bytes!(
            "../../../assets/presets/small-change-v2.yaml"
        ));
        assert_preset_fits("small-change-v2", maxima);
    }

    #[test]
    fn v2scl005_two_maximal_goal_assessment_sources_cannot_fit_the_record_allocation() {
        // A complete goal-assessment decision record is the widest single thing `next` carries.
        // One fits its own allocation; two do not, which is what bounds how many decision sources
        // a placement may read back.
        let one = decision_record_charge(true);
        assert!(!exceeds_budget(one, DECISION_RECORD_BUDGET));
        assert_eq!(one, 262_186);
        let two = one.saturating_mul(2);
        assert!(exceeds_budget(two, DECISION_RECORD_BUDGET));
    }

    #[test]
    fn v2scl005_item_metadata_differs_only_by_the_type_name() {
        // Metadata carries identity, digest, and size, so the only thing that varies between item
        // kinds is the length of the type discriminant itself. An artifact no longer costs forty
        // times a confirm, because its descriptor is no longer in this allocation.
        let confirm = ItemSpecV2::confirm(common("confirm"));
        let text = ItemSpecV2::text(common("text"), 0, 10, false).expect("text item");
        let choice = ItemSpecV2::choice(common("choice"), vec!["x".into(), "zz".into()])
            .expect("choice item");
        let integer = ItemSpecV2::integer(common("integer"), None, None).expect("integer item");
        let list = ItemSpecV2::list(common("list"), 0, 2, 3, 1_000_000, false).expect("list item");
        let artifact = ItemSpecV2::artifact(common("artifact"), Vec::new()).expect("artifact item");

        let base = readback_item_charge(&confirm);
        for (item, kind) in [
            (&text, "text"),
            (&choice, "choice"),
            (&integer, "integer"),
            (&list, "list"),
            (&artifact, "artifact"),
        ] {
            let expected =
                base + string_bytes(actual_chars(kind)) - string_bytes(actual_chars("confirm"));
            assert_eq!(readback_item_charge(item), expected, "{kind}");
        }

        // 128 items plus their reference metadata must fit the allocation they share, or the
        // 128-item ceiling the schema publishes would be unreachable in practice.
        let widest = readback_item_charge(&artifact);
        let references = reference_metadata_charge()
            .saturating_mul(2)
            .saturating_mul(8);
        let total = add(
            add(array_field(), array_field()),
            add(references, widest.saturating_mul(128)),
        );
        assert!(
            !exceeds_budget(total, READBACK_BUDGET),
            "128 items across 8 references charge {total}, over {READBACK_BUDGET}"
        );
    }

    #[test]
    fn nexts_two_reference_surfaces_are_both_reserved() {
        assert_eq!(reference_metadata_charge(), 2_206);
        assert_eq!(
            add(
                add(array_field(), array_field()),
                add(
                    reference_metadata_charge(),
                    add(reference_metadata_charge(), array_field())
                ),
            ),
            4_604,
        );
    }

    #[test]
    fn suggestion_argv_reserves_the_v1_executable_element() {
        assert_eq!(suggestion("session.complete", &["complete"], None), 332);
    }

    #[test]
    fn v2scl005_the_six_allocations_exactly_fill_the_frame() {
        // ADR-0024 publishes a closed composition. If these stop summing to the frame the table is
        // no longer a proof of anything, so the arithmetic is checked rather than described.
        const ALLOCATIONS: [u64; 6] = [
            NEXT_STATIC_BUDGET,
            DECISION_RECORD_BUDGET,
            READBACK_BUDGET,
            EVIDENCE_PREVIEW_BUDGET,
            PAGE_TOKEN_BUDGET,
            NEXT_SERIALIZATION_RESERVE,
        ];
        let total: u64 = ALLOCATIONS.iter().copied().sum();
        assert_eq!(
            total, NEXT_FRAME_BUDGET,
            "the published allocations must exactly fill the {NEXT_FRAME_BUDGET}-byte frame"
        );
        assert_eq!(NEXT_FRAME_BUDGET, 1_048_576);
    }

    #[test]
    fn v2scl005_observation_guidance_holds_every_allocation_it_still_carries() {
        // Guidance is `next-result/v3` without its previews or page tokens, so its observation
        // allocation must equal the three that remain. podway-core owns the observation constant
        // and cannot see these, so this is the only place the derivation can be checked: without
        // it, rebalancing this table inside its own frame would silently make a Procedure that
        // `session.next` admits into one `session.observe` cannot answer.
        assert_eq!(
            podway_core::MAX_OBSERVATION_GUIDANCE_BYTES_V2 as u64,
            NEXT_STATIC_BUDGET + DECISION_RECORD_BUDGET + READBACK_BUDGET,
        );
    }

    #[test]
    fn v2scl005_the_preview_and_token_allocations_hold_every_slot() {
        // The runtime spends these allocations one slot at a time, so the authoring ceiling must
        // admit the widest response the runtime can actually build.
        let slots = podway_core::MAX_EVIDENCE_PREVIEW_SLOTS_V2 as u64;
        assert!(
            slots * podway_core::MAX_EVIDENCE_PREVIEW_BYTES_V2 as u64 <= EVIDENCE_PREVIEW_BUDGET
        );
        assert!(slots * page_token_charge() <= PAGE_TOKEN_BUDGET);
    }

    #[test]
    fn v2scl005_item_metadata_is_independent_of_declared_value_size() {
        // ADR-0024 removed complete values from guidance. A text item that may hold 65,536 scalars
        // must therefore cost the same metadata as one that may hold four.
        use podway_core::{ItemCommonV2, ItemSpecV2, TextItemSpecV2};
        let common = |id: &str| {
            ItemCommonV2::new(
                podway_core::ItemId::new(id).unwrap(),
                "Record it.".to_owned(),
                None,
                true,
            )
            .unwrap()
        };
        let narrow = ItemSpecV2::Text(TextItemSpecV2::new(common("narrow"), 0, 4, false).unwrap());
        let wide = ItemSpecV2::Text(
            TextItemSpecV2::new(common("wide"), 0, podway_core::MAX_TEXT_SCALARS_V2, false)
                .unwrap(),
        );
        assert_eq!(readback_item_charge(&narrow), readback_item_charge(&wide));
    }
}
