//! V2DOG-001: canonical full-feature software-development preset.

use std::fs;
use std::path::Path;

use podway_config::{
    AuthoringContext, FormatRequest, ParsedNodeDefinition, ParsedProcedure,
    ProcedureDocumentFormat, format_procedure_v2, lint_procedure_v2, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{GraphPlacementV2, TransitionEffectV2};

fn source() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets/sw-dev-v2.yaml"),
    )
    .expect("the canonical sw-dev-v2 source must be readable")
}

#[test]
fn sw_dev_v2_is_canonical_clean_and_exercises_the_full_graph_contract() {
    let source = source();
    let parsed = match parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml)
        .expect("sw-dev-v2 must parse")
    {
        ParsedProcedure::V2(parsed) => parsed,
    };
    let validated = validate_procedure_v2(parsed).expect("sw-dev-v2 must validate");
    let context = AuthoringContext::new(
        "assets/presets/sw-dev-v2.yaml",
        &source,
        ProcedureDocumentFormat::Yaml,
    );

    assert!(vet_procedure_v2(&validated, &context).is_empty());
    assert!(lint_procedure_v2(&validated, &context).is_empty());
    let formatted = format_procedure_v2(FormatRequest {
        source: &source,
        source_path: "assets/presets/sw-dev-v2.yaml",
        format: ProcedureDocumentFormat::Yaml,
    })
    .expect("sw-dev-v2 must format");
    assert!(!formatted.changed());
    assert_eq!(formatted.document(), source);

    let procedure = validated.parsed();
    assert_eq!(procedure.id(), "sw-dev-v2");
    assert!(
        procedure
            .goal_tracking()
            .is_some_and(|policy| policy.is_enabled())
    );
    assert_eq!(procedure.node_definitions().len(), 9);
    assert_eq!(procedure.graph().placements().len(), 13);
    assert_eq!(
        procedure
            .graph()
            .placements()
            .iter()
            .filter(|placement| matches!(placement, GraphPlacementV2::Decision(_)))
            .count(),
        4
    );
    assert!(procedure.graph().placements().iter().any(|placement| {
        matches!(
            placement,
            GraphPlacementV2::Action(action)
                if action.skip().is_some_and(|policy| policy.is_allowed())
        )
    }));
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
    assert_eq!(
        procedure
            .graph()
            .manual_rework()
            .expect("sw-dev-v2 must declare bounded manual rework")
            .targets()
            .len(),
        3
    );
    assert!(procedure.node_definitions().iter().any(|definition| {
        matches!(
            definition,
            ParsedNodeDefinition::Decision(decision) if decision.assessment().is_some()
        )
    }));
    assert!(procedure.graph().placements().iter().any(|placement| {
        matches!(
            placement,
            GraphPlacementV2::Decision(decision) if decision.evidence_from().is_some()
        )
    }));
}
