//! V2DOG-002: canonical full-feature bug-fix preset.

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
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets/bug-fix-v2.yaml"),
    )
    .expect("the canonical bug-fix-v2 source must be readable")
}

#[test]
fn bug_fix_v2_is_canonical_clean_and_covers_the_full_defect_workflow() {
    let source = source();
    let parsed = match parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml)
        .expect("bug-fix-v2 must parse")
    {
        ParsedProcedure::V2(parsed) => parsed,
        ParsedProcedure::V1(_) => panic!("bug-fix-v2 must dispatch as Procedure v2"),
    };
    let validated = validate_procedure_v2(parsed).expect("bug-fix-v2 must validate");
    let context = AuthoringContext::new(
        "assets/presets/bug-fix-v2.yaml",
        &source,
        ProcedureDocumentFormat::Yaml,
    );

    assert!(vet_procedure_v2(&validated, &context).is_empty());
    assert!(lint_procedure_v2(&validated, &context).is_empty());
    let formatted = format_procedure_v2(FormatRequest {
        source: &source,
        source_path: "assets/presets/bug-fix-v2.yaml",
        format: ProcedureDocumentFormat::Yaml,
    })
    .expect("bug-fix-v2 must format");
    assert!(!formatted.changed());
    assert_eq!(formatted.document(), source);

    let procedure = validated.parsed();
    assert_eq!(procedure.id(), "bug-fix-v2");
    assert!(
        procedure
            .goal_tracking()
            .is_some_and(|policy| policy.is_enabled())
    );
    assert_eq!(procedure.node_definitions().len(), 12);
    assert_eq!(procedure.graph().placements().len(), 14);
    assert_eq!(
        procedure
            .graph()
            .placements()
            .iter()
            .filter(|placement| matches!(placement, GraphPlacementV2::Decision(_)))
            .count(),
        4
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
    assert_eq!(
        procedure
            .graph()
            .manual_rework()
            .expect("bug-fix-v2 must declare bounded manual rework")
            .targets()
            .len(),
        6
    );
    assert!(procedure.node_definitions().iter().any(|definition| {
        matches!(
            definition,
            ParsedNodeDefinition::Decision(decision) if decision.assessment().is_some()
        )
    }));
    for definition in procedure.node_definitions() {
        let items = match definition {
            ParsedNodeDefinition::Action(action) => action.items(),
            ParsedNodeDefinition::Decision(decision) => decision.items(),
        };
        for item in items {
            if let ItemSpecV2::Text(text) = item
                && text.common().required()
            {
                assert!(
                    text.min_length() >= 1,
                    "required text item {} must reject empty evidence",
                    text.common().id()
                );
            }
        }
    }
    for required_node in [
        "reproduce",
        "implement",
        "verify",
        "review",
        "assess-session-goal",
        "record-closeout",
    ] {
        assert!(
            procedure
                .graph()
                .placements()
                .iter()
                .any(|placement| placement.id().as_str() == required_node),
            "missing required bug-fix placement {required_node}"
        );
    }
}
