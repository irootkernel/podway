//! V2AUT-006: the authoring starting points `podway procedure scaffold` emits (dossier section
//! 11.6).
//!
//! A template is a checked-in literal, so most of what a generator would need proving is already
//! true by construction. What is *not* automatic is that the literal agrees with the rest of the
//! toolchain, and that is what this file asserts:
//!
//! - the template is the **canonical emitter's fixpoint**, so `format --check` on a freshly
//!   scaffolded file reports no drift;
//! - the aggregate gate finds **nothing at all** — not one error, not one advisory — so
//!   `podway procedure scaffold > new.yaml && podway procedure check --warnings-as-errors new.yaml`
//!   is quiet, which is the strongest available statement that the shipped bytes are lint-clean;
//! - the identity of the document is **pinned to its semantic digest** rather than to a second copy
//!   of the same literal, which would only assert that the file equals itself;
//! - the emitted model really does demonstrate the shape the template claims to, so a future edit
//!   cannot quietly reduce the scaffold to a one-node stub; and
//! - the template is **small enough to read**, which is the reviewability half of the roadmap gate.

use podway_config::{
    FormatRequest, ParsedNodeDefinition, ParsedProcedure, ProcedureDocumentFormat,
    SCAFFOLD_TEMPLATE_MINIMAL, ScaffoldTemplate, ValidatedProcedureV2, check_procedure_v2,
    format_procedure_v2, parse_procedure_document, scaffold_procedure_v2, validate_procedure_v2,
};
use podway_core::{
    GraphNodeId, GraphPlacementV2, ItemSpecV2, ItemTypeV1, SOURCE_PROJECTION_MAX_CHARACTERS,
};

/// The label a scaffold reports under: there is no file, and the template is what it describes.
const SOURCE_PATH: &str = "scaffold";

/// The canonical semantic digest of the `minimal` template, computed once from the shipped literal.
///
/// This is the golden. Comparing the template against a second literal copy of itself would assert
/// nothing; the digest is derived through the same parse-validate-canonicalize path every other
/// Procedure v2 digest comes from, so pinning it here fails whenever an edit changes what the
/// scaffold *means* — while leaving the comment prose, which carries no meaning to the model, free
/// to be improved.
const MINIMAL_TEMPLATE_DIGEST: &str =
    "sha256:0d6bfb59bded0f08da06c75d317babc991dec9060c1d84621ed24f80365df8b4";

/// The largest a template may grow and still be a document a reviewer reads in one sitting.
///
/// Far below the 131,072-character projection bound the emitter enforces: that bound stops an
/// unprintable document, while this one keeps a *starting point* from becoming a workflow.
const REVIEWABLE_TEMPLATE_MAX_CHARACTERS: usize = 4096;

fn admit(source: &str) -> ValidatedProcedureV2 {
    match parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml) {
        Ok(ParsedProcedure::V2(parsed)) => {
            validate_procedure_v2(parsed).expect("a scaffold template must validate")
        }
        Ok(ParsedProcedure::V1(_)) => panic!("a scaffold template declares the v2 schema"),
        Err(error) => panic!("a scaffold template must parse: {error}\n{source}"),
    }
}

#[test]
fn v2aut006_the_minimal_template_is_the_canonical_emitters_fixpoint() {
    let formatted = format_procedure_v2(FormatRequest {
        source: SCAFFOLD_TEMPLATE_MINIMAL,
        source_path: SOURCE_PATH,
        format: ProcedureDocumentFormat::Yaml,
    })
    .expect("the scaffold template must render in canonical authoring form");

    assert_eq!(
        formatted.document(),
        SCAFFOLD_TEMPLATE_MINIMAL,
        "the shipped template must be exactly what the emitter would produce from it"
    );
    assert!(
        !formatted.changed(),
        "a fixpoint has no drift against itself"
    );
    assert_eq!(formatted.digest().as_str(), MINIMAL_TEMPLATE_DIGEST);
}

#[test]
fn v2aut006_the_minimal_template_parses_validates_and_keeps_its_pinned_digest() {
    let validated = admit(SCAFFOLD_TEMPLATE_MINIMAL);
    assert_eq!(validated.digest().as_str(), MINIMAL_TEMPLATE_DIGEST);
}

#[test]
fn v2aut006_the_aggregate_gate_reports_nothing_at_all_about_the_minimal_template() {
    let report = check_procedure_v2(FormatRequest {
        source: SCAFFOLD_TEMPLATE_MINIMAL,
        source_path: SOURCE_PATH,
        format: ProcedureDocumentFormat::Yaml,
    });

    // Not "no errors": no findings whatsoever. `--warnings-as-errors` moves the exit code whenever
    // any finding is present, so an advisory here would make the documented smoke pipeline fail.
    assert_eq!(
        report.diagnostics(),
        &[],
        "the scaffold must be clean under every authoring stage"
    );
    assert_eq!(report.total(), 0);
    assert!(!report.truncated());
    assert!(report.valid());
    assert_eq!(
        report.digest().map(|digest| digest.as_str()),
        Some(MINIMAL_TEMPLATE_DIGEST),
        "an admissible document carries its digest through the aggregate gate"
    );
}

#[test]
fn v2aut006_the_minimal_template_demonstrates_the_whole_authoring_shape() {
    let validated = admit(SCAFFOLD_TEMPLATE_MINIMAL);
    let parsed = validated.parsed();

    let definitions: Vec<&str> = parsed
        .node_definitions()
        .iter()
        .map(|definition| definition.id().as_str())
        .collect();
    assert_eq!(definitions, ["perform-work", "confirm-completion"]);

    let graph = parsed.graph();
    let placements: Vec<&str> = graph
        .placements()
        .iter()
        .map(|placement| placement.id().as_str())
        .collect();
    assert_eq!(placements, ["perform", "confirm"]);
    assert_eq!(graph.entry().as_str(), "perform");

    // Both dispositions an action placement can have, so the template shows chaining and ending.
    let outcomes: Vec<Option<&str>> = graph
        .placements()
        .iter()
        .map(|placement| match placement {
            GraphPlacementV2::Action(action) => {
                action.outcome().next_target().map(GraphNodeId::as_str)
            }
            GraphPlacementV2::Decision(_) => panic!("the minimal template declares no decision"),
        })
        .collect();
    assert_eq!(outcomes, [Some("confirm"), None]);

    // One multiline text item and one confirm item: the two kinds an author needs to see before
    // writing a third, and the multiline flag is what makes the text item worth demonstrating.
    let items: Vec<&ItemSpecV2> = parsed
        .node_definitions()
        .iter()
        .flat_map(|definition| match definition {
            ParsedNodeDefinition::Action(action) => action.items(),
            ParsedNodeDefinition::Decision(_) => {
                panic!("the minimal template declares no decision")
            }
        })
        .collect();
    let item_types: Vec<ItemTypeV1> = items.iter().map(|item| item.item_type()).collect();
    assert_eq!(item_types, [ItemTypeV1::Text, ItemTypeV1::Confirm]);
    let ItemSpecV2::Text(summary) = items[0] else {
        unreachable!("the first item is the text item asserted above");
    };
    assert!(summary.multiline());

    // Rework is present, which is also what keeps the template clear of `NO_REACTIVATION_PATH`.
    let targets: Vec<&str> = graph
        .manual_rework()
        .expect("the template declares manual rework")
        .targets()
        .iter()
        .map(GraphNodeId::as_str)
        .collect();
    assert_eq!(targets, ["perform"]);
}

#[test]
fn v2aut006_the_minimal_template_is_small_enough_to_review() {
    let characters = SCAFFOLD_TEMPLATE_MINIMAL.chars().count();
    assert!(
        characters < REVIEWABLE_TEMPLATE_MAX_CHARACTERS,
        "a scaffold is a starting point, not a workflow: {characters} characters"
    );
    assert!(characters < SOURCE_PROJECTION_MAX_CHARACTERS);
    assert!(
        SCAFFOLD_TEMPLATE_MINIMAL.ends_with('\n')
            && !SCAFFOLD_TEMPLATE_MINIMAL.ends_with("\n\n")
            && !SCAFFOLD_TEMPLATE_MINIMAL.contains('\r'),
        "canonical authoring form is LF with exactly one trailing newline"
    );
}

#[test]
fn v2aut006_the_template_vocabulary_is_closed_and_the_lookup_is_constant() {
    assert_eq!(ScaffoldTemplate::NAMES, ["minimal"]);
    assert_eq!(ScaffoldTemplate::Minimal.name(), "minimal");
    assert_eq!(
        ScaffoldTemplate::from_name("minimal"),
        Some(ScaffoldTemplate::Minimal)
    );
    assert_eq!(ScaffoldTemplate::from_name("Minimal"), None);
    assert_eq!(ScaffoldTemplate::from_name(""), None);
    assert_eq!(ScaffoldTemplate::from_name("standard"), None);
    for name in ScaffoldTemplate::NAMES {
        assert_eq!(
            ScaffoldTemplate::from_name(name).map(ScaffoldTemplate::name),
            Some(name),
            "every offered name must round-trip through the enum"
        );
    }

    // Determinism is the return type: a `&'static str` is chosen at compile time, so there is no
    // rendering step two calls could disagree about.
    let first = scaffold_procedure_v2(ScaffoldTemplate::Minimal);
    let second = scaffold_procedure_v2(ScaffoldTemplate::Minimal);
    assert_eq!(first, second);
    assert_eq!(first, SCAFFOLD_TEMPLATE_MINIMAL);
}
