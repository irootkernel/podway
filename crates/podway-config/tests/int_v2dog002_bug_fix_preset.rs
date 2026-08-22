//! V2DOG-002: canonical full-feature bug-fix preset.

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
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets/bug-fix-v2.yaml"),
    )
    .expect("the canonical bug-fix-v2 source must be readable")
}

#[test]
fn bug_fix_v2_is_canonical_clean_and_covers_the_full_defect_workflow() {
    let source = source();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml)
            .expect("bug-fix-v2 must parse");
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
        2
    );
    assert_eq!(
        procedure
            .graph()
            .manual_rework()
            .expect("bug-fix-v2 must declare bounded manual rework")
            .targets()
            .iter()
            .map(|target| target.as_str())
            .collect::<Vec<_>>(),
        ["reproduce", "diagnose", "implement", "verify", "review"]
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
    assert_eq!(
        procedure
            .graph()
            .placements()
            .iter()
            .map(|placement| placement.id().as_str())
            .collect::<Vec<_>>(),
        [
            "reproduce",
            "diagnose",
            "implement",
            "verify",
            "evaluate-verification",
            "review",
            "evaluate-review",
            "assess-goal",
            "closeout",
        ]
    );

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
    assert_eq!(result.operation_id().as_str(), "bug-fix-verification");
    assert_eq!(
        result.operation_digest().as_str(),
        "sha256:01122b6057efcfa3f22c453aa48fb647ccba9f7db437dd44473a9f32a854a979"
    );
    assert_eq!(
        result.accepted_outcomes(),
        &[
            CheckResultOutcomeV2::Pass,
            CheckResultOutcomeV2::Fail,
            CheckResultOutcomeV2::Inconclusive,
        ]
    );
    let implementation = action_items("implementation");
    assert!(matches!(&implementation[1], ItemSpecV2::List(_)));
    let review = action_items("review-work");
    let ItemSpecV2::Integer(count) = &review[1] else {
        panic!("unresolved-valid-findings must be an integer");
    };
    assert_eq!(count.minimum(), Some(0));
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
    assert_eq!(guarded_option_counts, [2, 2, 0]);
}
