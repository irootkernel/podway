//! V2AGT-006: canonical lightweight verified-change preset.

use std::fs;
use std::path::Path;

use podway_config::{
    AuthoringContext, FormatRequest, ParsedNodeDefinition, ParsedProcedure,
    ProcedureDocumentFormat, format_procedure_v2, lint_procedure_v2, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{GraphPlacementV2, ItemSpecV2, TransitionEffectV2};

fn source() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets/small-change-v2.yaml"),
    )
    .expect("the canonical small-change-v2 source must be readable")
}

#[test]
fn small_change_v2_is_canonical_clean_and_matches_the_lightweight_contract() {
    let source = source();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml)
            .expect("small-change-v2 must parse");
    let validated = validate_procedure_v2(parsed).expect("small-change-v2 must validate");
    let context = AuthoringContext::new(
        "assets/presets/small-change-v2.yaml",
        &source,
        ProcedureDocumentFormat::Yaml,
    );

    assert!(vet_procedure_v2(&validated, &context).is_empty());
    assert!(lint_procedure_v2(&validated, &context).is_empty());
    let formatted = format_procedure_v2(FormatRequest {
        source: &source,
        source_path: "assets/presets/small-change-v2.yaml",
        format: ProcedureDocumentFormat::Yaml,
    })
    .expect("small-change-v2 must format");
    assert!(!formatted.changed());
    assert_eq!(formatted.document(), source);

    let procedure = validated.parsed();
    assert_eq!(procedure.id(), "small-change-v2");
    assert!(procedure.goal_tracking().is_none());
    assert_eq!(procedure.node_definitions().len(), 5);
    assert_eq!(procedure.graph().placements().len(), 5);
    assert_eq!(
        procedure
            .graph()
            .placements()
            .iter()
            .map(|placement| placement.id().as_str())
            .collect::<Vec<_>>(),
        ["inspect", "implement", "verify", "review", "closeout"]
    );

    let review = procedure
        .graph()
        .placements()
        .iter()
        .find_map(|placement| match placement {
            GraphPlacementV2::Decision(decision) if decision.id().as_str() == "review" => {
                Some(decision)
            }
            _ => None,
        })
        .expect("review must be the sole decision");
    assert_eq!(review.routes().entries().len(), 2);
    assert!(review.routes().entries().iter().any(|route| {
        route.option_id().as_str() == "changes-requested"
            && route.route().to().as_str() == "implement"
            && route.route().effect() == TransitionEffectV2::Rework
    }));
    assert_eq!(
        review
            .evidence_from()
            .expect("review evidence")
            .entries()
            .len(),
        3
    );
    assert_eq!(
        procedure
            .graph()
            .manual_rework()
            .expect("bounded manual rework")
            .targets()
            .iter()
            .map(|target| target.as_str())
            .collect::<Vec<_>>(),
        ["inspect", "implement", "verify"]
    );

    let item_types = procedure
        .node_definitions()
        .iter()
        .flat_map(|definition| match definition {
            ParsedNodeDefinition::Action(action) => action.items(),
            ParsedNodeDefinition::Decision(decision) => decision.items(),
        })
        .map(|item| match item {
            ItemSpecV2::Text(_) => "text",
            ItemSpecV2::Integer(_) => "integer",
            _ => "other",
        })
        .collect::<Vec<_>>();
    assert_eq!(item_types, ["text", "text", "text", "integer", "text"]);
}
