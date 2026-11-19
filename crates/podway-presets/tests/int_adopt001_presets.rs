use podway_config::execution_groups::{SuccessorDefinition, format_successor_definition};
use podway_config::{FormatRequest, ProcedureDocumentFormat};
use podway_core::execution_groups::NodeDefinitionV3;
use podway_core::{ItemPredicateOperatorV2, ItemSpecV2, OptionId, PredicateScalarV2};
use podway_presets::{EmbeddedPresetV3, PresetError, list_v3, lookup_v3};

#[test]
fn successor_presets_pair_canonical_sources_with_independent_pins() {
    let replan = OptionId::new("replan".to_owned()).unwrap();
    for preset in list_v3() {
        let id = preset.metadata.id;
        assert_eq!(lookup_v3(id), Some(*preset));
        let admitted = preset.validate().unwrap();
        let SuccessorDefinition::Base(base) = admitted.base.definition() else {
            panic!("{id}: expected a base Procedure");
        };
        let SuccessorDefinition::Group(group) = admitted.default_group.definition() else {
            panic!("{id}: expected a default execution group");
        };
        assert_eq!(base.goal_tracking().is_some(), id != "small-change-v3");
        let mut guarded_replan = false;
        let mut typed_verification = false;
        for definition in group.definitions() {
            match definition {
                NodeDefinitionV3::Action(action) => {
                    typed_verification |= action
                        .items()
                        .iter()
                        .any(|item| matches!(item, ItemSpecV2::CheckResult(_)));
                }
                NodeDefinitionV3::Decision(decision) => {
                    let guards = decision.guards(&replan).expect("guarded replan option");
                    let clauses = guards
                        .predicates()
                        .iter()
                        .map(|guard| {
                            (
                                guard.predicate().item().as_str(),
                                guard.predicate().operator(),
                            )
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(
                        clauses,
                        [
                            (
                                "assessment-verdict",
                                &ItemPredicateOperatorV2::Equals(PredicateScalarV2::Text(
                                    "failure".to_owned()
                                ))
                            ),
                            (
                                "assessment-stage",
                                &ItemPredicateOperatorV2::Equals(PredicateScalarV2::Text(
                                    "scored".to_owned()
                                ))
                            ),
                            ("assessment-score", &ItemPredicateOperatorV2::AtLeast(80)),
                        ],
                    );
                    guarded_replan = true;
                }
            }
        }
        assert!(guarded_replan, "{id}");
        assert_eq!(typed_verification, matches!(id, "bug-fix-v3" | "sw-dev-v3"));
        for (source, pin) in [
            (preset.base_yaml, preset.base_digest),
            (preset.group_yaml, preset.group_digest),
        ] {
            let formatted = format_successor_definition(FormatRequest {
                format: ProcedureDocumentFormat::Yaml,
                source_path: id,
                source,
            })
            .unwrap();
            assert!(!formatted.changed(), "{id}");
            assert_eq!(formatted.digest().as_str(), pin);
        }
    }
    assert_eq!(lookup_v3("sw-dev-v2"), None);
}

#[test]
fn default_group_cannot_be_substituted_without_its_own_pin() {
    let preset = lookup_v3("sw-dev-v3").unwrap();
    let mismatched = EmbeddedPresetV3 {
        group_digest: preset.base_digest,
        ..preset
    };
    assert!(matches!(
        mismatched.validate(),
        Err(PresetError::PinnedDigestMismatch { expected, actual, .. })
            if expected.as_str() == preset.base_digest && actual.as_str() == preset.group_digest
    ));
}
