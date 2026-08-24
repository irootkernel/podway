//! V2REF: structural contract for the canonical analysis preset.

use std::fs;
use std::path::Path;

use podway_config::{
    ParsedNodeDefinition, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2,
};
use podway_core::{GraphPlacementV2, TransitionEffectV2};

#[test]
fn analysis_v2_pins_its_graph_and_conditional_review_contract() {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets/analysis-v2.yaml"),
    )
    .expect("the canonical analysis-v2 source must be readable");
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml)
            .expect("analysis-v2 must parse");
    let validated = validate_procedure_v2(parsed).expect("analysis-v2 must validate");
    let procedure = validated.parsed();

    assert_eq!(procedure.id(), "analysis-v2");
    assert_eq!(procedure.version(), "1");
    assert_eq!(
        procedure
            .graph()
            .placements()
            .iter()
            .map(|placement| placement.id().as_str())
            .collect::<Vec<_>>(),
        [
            "define",
            "collect",
            "analyze",
            "challenge",
            "evaluate-evidence",
            "synthesize",
            "review",
            "evaluate-review",
            "assess-goal",
            "closeout",
        ]
    );
    assert_eq!(
        procedure
            .graph()
            .manual_rework()
            .expect("analysis-v2 must declare bounded manual rework")
            .targets()
            .iter()
            .map(|target| target.as_str())
            .collect::<Vec<_>>(),
        [
            "define",
            "collect",
            "analyze",
            "challenge",
            "synthesize",
            "review"
        ]
    );
    assert_eq!(
        procedure
            .graph()
            .placements()
            .iter()
            .filter_map(|placement| match placement {
                GraphPlacementV2::Decision(decision) => Some(decision),
                GraphPlacementV2::Action(_) => None,
            })
            .flat_map(|decision| decision.routes().entries())
            .filter(|route| route.route().effect() == TransitionEffectV2::Rework)
            .count(),
        3
    );
    let guarded_option_counts: Vec<_> = procedure
        .node_definitions()
        .iter()
        .filter_map(|definition| match definition {
            ParsedNodeDefinition::Decision(decision) => Some(
                decision
                    .options()
                    .iter()
                    .filter(|option| option.guards().is_some())
                    .count(),
            ),
            ParsedNodeDefinition::Action(_) => None,
        })
        .collect();
    assert_eq!(guarded_option_counts, [2, 3, 0]);

    let conditional_items: Vec<_> = procedure
        .node_definitions()
        .iter()
        .flat_map(|definition| match definition {
            ParsedNodeDefinition::Action(action) => action.items(),
            ParsedNodeDefinition::Decision(decision) => decision.items(),
        })
        .filter(|item| item.common().required_when().is_some())
        .map(|item| item.common().id().as_str())
        .collect();
    assert_eq!(conditional_items, ["source-findings", "analysis-findings"]);
}
