//! Conservative Procedure v2 `next` component accounting (dossier sections 10.4 and 11.3).
//!
//! This module charges the two components whose size depends on one graph placement. It mirrors
//! the canonical `next-result-v2` and shared result-component schemas: a bounded string costs six
//! bytes per Unicode scalar, every object field reserves 64 bytes, and every array element reserves
//! another eight bytes. All arithmetic saturates so future bound increases cannot turn a resource
//! rejection into an under-count through integer overflow.

use podway_core::{GraphPlacementV2, ItemSpecV2};

use crate::procedure_v2_authoring::{placement_definition_id, placement_evidence_from};
use crate::{ParsedNodeDefinition, ParsedProcedureV2};

/// Maximum procedure-snapshot-derived content in one `next` result.
pub const NEXT_STATIC_BUDGET: u64 = 262_144;
/// Maximum resolved evidence read-back in one `next` result.
pub const READBACK_BUDGET: u64 = 524_288;

const STRING_BYTE_FACTOR: u64 = 6;
const FIELD_OVERHEAD: u64 = 64;
const ARRAY_ELEMENT_OVERHEAD: u64 = 8;
const MAX_IDENTIFIER_CHARS: u64 = 64;
const MAX_DEFINITION_TITLE_CHARS: u64 = 120;
const MAX_UUID_CHARS: u64 = 36;
const MAX_DIGEST_CHARS: u64 = 71;
const MAX_TIMESTAMP_CHARS: u64 = 24;
const MAX_RECORD_REASON_CHARS: u64 = 2_000;
const MAX_ACTOR_CHARS: u64 = 256;
const MAX_ARTIFACT_LOCATION_CHARS: u64 = 4_000;
const MAX_ARTIFACT_MEDIA_TYPE_CHARS: u64 = 255;
const MAX_U64_BYTES: u64 = 20;
const MAX_I64_BYTES: u64 = 20;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PlacementBudget {
    pub(crate) next_static: u64,
    pub(crate) readback: u64,
}

pub(crate) fn placement_budget(
    procedure: &ParsedProcedureV2,
    placement: &GraphPlacementV2,
) -> PlacementBudget {
    let definition = procedure
        .node_definitions()
        .iter()
        .find(|candidate| candidate.id().as_str() == placement_definition_id(placement));
    let Some(definition) = definition else {
        // Closed-reference validation makes this unreachable. Saturation preserves fail-closed
        // behavior if a caller ever violates the validated-model precondition.
        return PlacementBudget {
            next_static: u64::MAX,
            readback: u64::MAX,
        };
    };
    PlacementBudget {
        next_static: next_static_charge(procedure, placement, definition),
        readback: readback_charge(procedure, placement),
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
    charge = add(charge, fixed_field(2)); // missing_required_item_count is bounded at 64
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
        let (command, verb, placeholder) = match item {
            ItemSpecV2::Confirm(_) => ("item.check", "check", None),
            ItemSpecV2::Text(_) => ("item.set", "set", Some("<text>")),
            ItemSpecV2::Choice(_) => ("item.set", "set", Some("<choice>")),
            ItemSpecV2::Integer(_) => ("item.set", "set", Some("<integer>")),
            ItemSpecV2::List(_) => ("item.add", "add", Some("<value>")),
            ItemSpecV2::Artifact(_) => ("item.attach", "attach", Some("<path>")),
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
    // Section 10.1 reuses the v1 JSON-contract argv shape, whose first element is always the
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

fn readback_charge(procedure: &ParsedProcedureV2, placement: &GraphPlacementV2) -> u64 {
    // Both arrays are required by next-result-v2. Their elements repeat the reference metadata.
    let mut charge = add(array_field(), array_field());
    let Some(evidence) = placement_evidence_from(placement) else {
        return charge;
    };

    for reference in evidence.entries() {
        let source = procedure
            .graph()
            .placements()
            .iter()
            .find(|candidate| candidate.id() == reference.source_node());
        let Some(source) = source else {
            return u64::MAX;
        };
        let definition = procedure
            .node_definitions()
            .iter()
            .find(|candidate| candidate.id().as_str() == placement_definition_id(source));
        let Some(definition) = definition else {
            return u64::MAX;
        };

        charge = add(charge, reference_metadata_charge());
        charge = add(charge, readback_entry_charge(reference, definition));
    }
    charge
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

fn readback_entry_charge(
    reference: &podway_core::EvidenceReferenceV2,
    definition: &ParsedNodeDefinition,
) -> u64 {
    let mut charge = reference_metadata_charge();
    charge = add(charge, array_field());
    let items = match definition {
        ParsedNodeDefinition::Action(action) => action.items(),
        ParsedNodeDefinition::Decision(decision) => decision.items(),
    };
    match reference.selected_items() {
        Some(selected) => {
            for selected_id in selected {
                let Some(item) = items.iter().find(|item| item.id() == selected_id) else {
                    return u64::MAX;
                };
                charge = add(charge, readback_item_charge(item));
            }
        }
        None => {
            for item in items {
                charge = add(charge, readback_item_charge(item));
            }
        }
    }
    if let ParsedNodeDefinition::Decision(decision) = definition {
        charge = add(
            charge,
            decision_record_charge(decision.assessment().is_some()),
        );
    }
    charge
}

fn readback_item_charge(item: &ItemSpecV2) -> u64 {
    let mut charge = ARRAY_ELEMENT_OVERHEAD;
    charge = add(charge, string_field(MAX_IDENTIFIER_CHARS));
    let (kind, value) = match item {
        ItemSpecV2::Confirm(_) => ("confirm", fixed_field(MAX_BOOL_BYTES)),
        ItemSpecV2::Text(text) => ("text", string_field(u64::from(text.max_length()))),
        ItemSpecV2::Choice(choice) => {
            let maximum = choice
                .choices()
                .iter()
                .map(|value| actual_chars(value))
                .max()
                .unwrap_or(0);
            ("choice", string_field(maximum))
        }
        ItemSpecV2::Integer(_) => ("integer", fixed_field(MAX_I64_BYTES)),
        ItemSpecV2::List(list) => {
            let entries = u64::from(list.max_items());
            let one = add(
                ARRAY_ELEMENT_OVERHEAD,
                string_bytes(u64::from(list.max_item_length())),
            );
            ("list", add(array_field(), entries.saturating_mul(one)))
        }
        ItemSpecV2::Artifact(_) => ("artifact", artifact_value_charge()),
    };
    charge = add(charge, string_field(actual_chars(kind)));
    add(charge, value)
}

fn artifact_value_charge() -> u64 {
    let mut charge = field(); // the `value` object field
    charge = add(charge, string_field(actual_chars("reference")));
    charge = add(charge, string_field(MAX_ARTIFACT_LOCATION_CHARS));
    charge = add(charge, string_field(MAX_DIGEST_CHARS));
    charge = add(charge, fixed_field(MAX_U64_BYTES));
    add(charge, string_field(MAX_ARTIFACT_MEDIA_TYPE_CHARS))
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

    #[test]
    fn v2dog001_sw_dev_preset_records_budget_headroom() {
        let source = include_bytes!("../../../assets/presets/sw-dev-v2.yaml");
        let parsed = match parse_procedure_document(source, ProcedureDocumentFormat::Yaml)
            .expect("sw-dev-v2 must parse")
        {
            ParsedProcedure::V2(parsed) => parsed,
            ParsedProcedure::V1(_) => panic!("sw-dev-v2 must dispatch as Procedure v2"),
        };
        let validated = validate_procedure_v2(parsed).expect("sw-dev-v2 must validate");
        let usages: Vec<PlacementBudget> = validated
            .parsed()
            .graph()
            .placements()
            .iter()
            .map(|placement| placement_budget(validated.parsed(), placement))
            .collect();
        let maximum_static = usages
            .iter()
            .map(|usage| usage.next_static)
            .max()
            .expect("sw-dev-v2 has graph placements");
        let maximum_readback = usages
            .iter()
            .map(|usage| usage.readback)
            .max()
            .expect("sw-dev-v2 has graph placements");

        assert_eq!(maximum_static, 8_875);
        assert_eq!(maximum_readback, 359_734);
        assert_eq!(NEXT_STATIC_BUDGET - maximum_static, 253_269);
        assert_eq!(READBACK_BUDGET - maximum_readback, 164_554);
    }

    #[test]
    fn v2dog002_bug_fix_preset_records_budget_headroom() {
        let source = include_bytes!("../../../assets/presets/bug-fix-v2.yaml");
        let parsed = match parse_procedure_document(source, ProcedureDocumentFormat::Yaml)
            .expect("bug-fix-v2 must parse")
        {
            ParsedProcedure::V2(parsed) => parsed,
            ParsedProcedure::V1(_) => panic!("bug-fix-v2 must dispatch as Procedure v2"),
        };
        let validated = validate_procedure_v2(parsed).expect("bug-fix-v2 must validate");
        let usages: Vec<PlacementBudget> = validated
            .parsed()
            .graph()
            .placements()
            .iter()
            .map(|placement| placement_budget(validated.parsed(), placement))
            .collect();
        let maximum_static = usages
            .iter()
            .map(|usage| usage.next_static)
            .max()
            .expect("bug-fix-v2 has graph placements");
        let maximum_readback = usages
            .iter()
            .map(|usage| usage.readback)
            .max()
            .expect("bug-fix-v2 has graph placements");

        assert_eq!(maximum_static, 9_115);
        assert_eq!(maximum_readback, 359_734);
        assert_eq!(NEXT_STATIC_BUDGET - maximum_static, 253_029);
        assert_eq!(READBACK_BUDGET - maximum_readback, 164_554);
    }

    #[test]
    fn two_maximal_goal_assessment_sources_cannot_fit_readback() {
        let per_source = add(
            add(reference_metadata_charge(), reference_metadata_charge()),
            add(array_field(), decision_record_charge(true)),
        );
        let two_arrays = add(array_field(), array_field());
        let total = add(two_arrays, per_source.saturating_mul(2));
        assert!(per_source < READBACK_BUDGET);
        assert_eq!(total, 533_452);
        assert!(total > READBACK_BUDGET);
    }

    #[test]
    fn every_item_kind_has_an_independent_worst_case_known_answer() {
        let confirm = ItemSpecV2::confirm(common("confirm"));
        let text = ItemSpecV2::text(common("text"), 0, 10, false).expect("text item");
        let choice = ItemSpecV2::choice(common("choice"), vec!["x".into(), "zz".into()])
            .expect("choice item");
        let integer = ItemSpecV2::integer(common("integer"), None, None).expect("integer item");
        let list = ItemSpecV2::list(common("list"), 0, 2, 3, false).expect("list item");
        let artifact = ItemSpecV2::artifact(common("artifact"), Vec::new()).expect("artifact item");

        assert_eq!(readback_item_charge(&confirm), 631);
        assert_eq!(readback_item_charge(&text), 668);
        assert_eq!(readback_item_charge(&choice), 632);
        assert_eq!(readback_item_charge(&integer), 646);
        assert_eq!(readback_item_charge(&list), 660);
        assert_eq!(readback_item_charge(&artifact), 26_982);
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
}
