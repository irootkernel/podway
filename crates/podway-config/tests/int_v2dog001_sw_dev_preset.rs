//! V2DOG-001: canonical full-feature software-development preset.

use std::fs;
use std::path::Path;

use podway_config::{
    AuthoringContext, FormatRequest, ParsedNodeDefinition, ParsedProcedure,
    ProcedureDocumentFormat, format_procedure_v2, lint_procedure_v2, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{CheckResultOutcomeV2, GraphPlacementV2, ItemSpecV2, TransitionEffectV2};

fn source() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets/sw-dev-v2.yaml"),
    )
    .expect("the canonical sw-dev-v2 source must be readable")
}

#[test]
fn sw_dev_v2_is_canonical_clean_and_exercises_the_full_graph_contract() {
    let source = source();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml)
            .expect("sw-dev-v2 must parse");
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
    assert_eq!(procedure.version(), "3");
    assert!(
        procedure
            .goal_tracking()
            .is_some_and(|policy| policy.is_enabled())
    );
    assert_eq!(procedure.node_definitions().len(), 9);
    assert_eq!(procedure.graph().placements().len(), 9);
    assert_eq!(
        procedure
            .graph()
            .placements()
            .iter()
            .filter(|placement| matches!(placement, GraphPlacementV2::Decision(_)))
            .count(),
        3
    );
    assert!(procedure.graph().placements().iter().all(|placement| {
        !matches!(
            placement,
            GraphPlacementV2::Action(action) if action.skip().is_some()
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
            .iter()
            .map(|target| target.as_str())
            .collect::<Vec<_>>(),
        ["plan", "implement", "verify", "document", "review"]
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

    let action_items = |definition_id: &str| {
        procedure
            .node_definitions()
            .iter()
            .find_map(|definition| match definition {
                ParsedNodeDefinition::Action(action) if action.id().as_str() == definition_id => {
                    Some(action.items())
                }
                _ => None,
            })
            .expect("the accepted action definition must exist")
    };
    let verification = action_items("verification");
    let ItemSpecV2::CheckResult(result) = &verification[0] else {
        panic!("verification-result must be a check result");
    };
    assert_eq!(
        result.operation_id().as_str(),
        "software-change-verification"
    );
    assert_eq!(
        result.operation_digest().as_str(),
        "sha256:b904aefd4dbd6b01337645fd34b1424efc9faf3d22c936de666305335076969e"
    );
    assert_eq!(
        result.accepted_outcomes(),
        &[
            CheckResultOutcomeV2::Pass,
            CheckResultOutcomeV2::Fail,
            CheckResultOutcomeV2::Inconclusive,
        ]
    );
    assert!(verification[1].common().required_when().is_some());
    assert!(matches!(&verification[2], ItemSpecV2::Artifact(_)));

    let documentation = action_items("documentation");
    assert!(documentation[1].common().required_when().is_some());
    let review = action_items("review-work");
    let ItemSpecV2::List(findings) = &review[3] else {
        panic!("review-findings must be a list");
    };
    assert_eq!(findings.max_total_length(), 400_000);

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
}
