//! V2GRF-001: structural Procedure v2 graph vetting.

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, ValidatedProcedureV2,
    parse_procedure_document, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{AuthoringDiagnostic, AuthoringDiagnosticCode};

const SOURCE_PATH: &str = "workflow.yaml";

const BASE: &str = r#"schema: podway.procedure/v2
id: diagnostics
version: "1"
name: Diagnostics
purpose: Exercise structural graph vetting end to end.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Work
    intent: Do the work this node owns.
    items:
      - id: note
        type: text
        prompt: Record what happened.
        required: true
  assess:
    type: decision
    title: Assess
    objective: Assess the session goal.
    prompt: What is the outcome of this session?
    options:
      - id: achieved
        label: Achieved
      - id: not-achieved
        label: Not achieved
      - id: superseded
        label: Superseded
    reason:
      required: true
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  finish:
    type: action
    title: Finish
    intent: Close the session out.
graph:
  entry: start
  nodes:
    - id: start
      use: work
      next: review
    - id: review
      use: assess
      evidence_from:
        - node: start
          items:
            - note
      routes:
        achieved:
          to: done
          effect: advance
        not-achieved:
          to: start
          effect: rework
        superseded:
          to: done
          effect: advance
    - id: done
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - start
"#;

fn mutate(from: &str, to: &str) -> String {
    assert!(BASE.contains(from), "base document has no {from:?}");
    BASE.replacen(from, to, 1)
}

fn admit(source: &str) -> ValidatedProcedureV2 {
    match parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml) {
        Ok(ParsedProcedure::V2(parsed)) => {
            validate_procedure_v2(parsed).expect("the fixture must pass closed validation")
        }
        Err(error) => panic!("fixture must parse: {error}\n{source}"),
    }
}

fn vet(source: &str) -> Vec<AuthoringDiagnostic> {
    let validated = admit(source);
    let context = AuthoringContext::new(SOURCE_PATH, source, ProcedureDocumentFormat::Yaml);
    vet_procedure_v2(&validated, &context)
}

fn codes(source: &str) -> Vec<&'static str> {
    vet(source)
        .iter()
        .map(|diagnostic| diagnostic.code().as_str())
        .collect()
}

fn assert_has(source: &str, expected: AuthoringDiagnosticCode) {
    let findings = vet(source);
    assert!(
        findings
            .iter()
            .any(|diagnostic| diagnostic.code() == expected),
        "expected {}, got {:?}",
        expected.as_str(),
        findings
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect::<Vec<_>>()
    );
}

fn oversized_static_document() -> String {
    let instructions = (0..16)
        .map(|_| format!("      - {}\n", "i".repeat(1_000)))
        .collect::<String>();
    let items = (0..64)
        .map(|index| {
            format!(
                "      - id: item-{index}-identifier\n        type: text\n        prompt: {}\n        required: true\n        max_length: 1\n",
                "p".repeat(300),
            )
        })
        .collect::<String>();
    format!(
        "schema: podway.procedure/v2\nid: static-budget\nversion: \"1\"\nname: Static budget\npurpose: Exercise the complete procedure-static next accounting.\nnode_definitions:\n  work:\n    type: action\n    title: {}\n    intent: {}\n    description: {}\n    instructions:\n{instructions}    items:\n{items}graph:\n  entry: work\n  nodes:\n    - id: work\n      use: work\n      terminal: true\n",
        "t".repeat(120),
        "n".repeat(300),
        "d".repeat(1_000),
    )
}

/// The charges for the `consumer` placement of a vetted document.
fn placement_charges(source: &str) -> podway_config::ProcedurePlacementBudgetV2 {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml)
            .expect("the fixture must parse");
    let validated = validate_procedure_v2(parsed).expect("the fixture must validate");
    podway_config::procedure_placement_budget_v2(
        &validated,
        &podway_core::GraphNodeId::new("consumer").unwrap(),
    )
    .expect("the fixture declares a consumer placement")
}

/// `sources` selector-less references to definitions of `items` confirm items each.
///
/// Confirm items are the cheapest metadata there is, so this shape separates the record ceiling
/// from the byte allocation: it can pass one and fail the other.
fn narrow_selectorless_document(sources: usize, items: usize) -> String {
    let mut definitions = String::new();
    let mut nodes = String::new();
    let mut evidence = String::new();
    for source in 0..sources {
        let declared = (0..items)
            .map(|item| {
                format!(
                    "      - id: item-{source}-{item:03}\n        type: confirm\n        prompt: Confirm it.\n        required: false\n"
                )
            })
            .collect::<String>();
        definitions.push_str(&format!(
            "  source-{source}:\n    type: action\n    title: Source {source}\n    intent: Record values.\n    items:\n{declared}"
        ));
        let next = if source + 1 == sources {
            "consumer".to_owned()
        } else {
            format!("source-{}", source + 1)
        };
        nodes.push_str(&format!(
            "    - id: source-{source}\n      use: source-{source}\n      next: {next}\n"
        ));
        evidence.push_str(&format!(
            "        - node: source-{source}\n          required: true\n"
        ));
    }
    format!(
        "schema: podway.procedure/v2\nid: narrow-selectorless\nversion: \"1\"\nname: Narrow selectorless\npurpose: Separate the record ceiling from the byte allocation.\nnode_definitions:\n{definitions}  consumer:\n    type: action\n    title: Consumer\n    intent: Read every recorded value.\ngraph:\n  entry: source-0\n  nodes:\n{nodes}    - id: consumer\n      use: consumer\n      evidence_from:\n{evidence}      terminal: true\n"
    )
}

/// A consumer reading back from `sources` selector-less references to 128-item definitions.
///
/// An omitted selector means every item of the source, so this is the widest metadata a schema
/// valid document can demand — far wider than the 8x16 explicit-selector case.
fn selectorless_evidence_document(sources: usize) -> String {
    let mut definitions = String::new();
    let mut nodes = String::new();
    let mut evidence = String::new();
    for source in 0..sources {
        let items = (0..128)
            .map(|item| {
                format!(
                    "      - id: item-{source}-{item:03}\n        type: artifact\n        prompt: Attach the artifact.\n        required: false\n"
                )
            })
            .collect::<String>();
        definitions.push_str(&format!(
            "  source-{source}:\n    type: action\n    title: Source {source}\n    intent: Record values.\n    items:\n{items}"
        ));
        let next = if source + 1 == sources {
            "consumer".to_owned()
        } else {
            format!("source-{}", source + 1)
        };
        nodes.push_str(&format!(
            "    - id: source-{source}\n      use: source-{source}\n      next: {next}\n"
        ));
        evidence.push_str(&format!(
            "        - node: source-{source}\n          required: true\n"
        ));
    }
    format!(
        "schema: podway.procedure/v2\nid: selectorless-evidence\nversion: \"1\"\nname: Selectorless evidence\npurpose: Charge the widest selector-less evidence declaration.\nnode_definitions:\n{definitions}  consumer:\n    type: action\n    title: Consumer\n    intent: Read every recorded value.\ngraph:\n  entry: source-0\n  nodes:\n{nodes}    - id: consumer\n      use: consumer\n      evidence_from:\n{evidence}      terminal: true\n"
    )
}

/// The widest evidence declaration the schema admits: eight references of sixteen items each.
fn widest_evidence_document() -> String {
    let mut definitions = String::new();
    let mut nodes = String::new();
    let mut evidence = String::new();
    for source in 0..8 {
        let items = (0..16)
            .map(|item| {
                format!(
                    "      - id: item-{source}-{item:02}\n        type: artifact\n        prompt: Attach the artifact.\n        required: false\n"
                )
            })
            .collect::<String>();
        definitions.push_str(&format!(
            "  source-{source}:\n    type: action\n    title: Source {source}\n    intent: Record values.\n    items:\n{items}"
        ));
        let next = if source == 7 {
            "consumer".to_owned()
        } else {
            format!("source-{}", source + 1)
        };
        nodes.push_str(&format!(
            "    - id: source-{source}\n      use: source-{source}\n      next: {next}\n"
        ));
        let selected = (0..16)
            .map(|item| format!("            - item-{source}-{item:02}\n"))
            .collect::<String>();
        evidence.push_str(&format!(
            "        - node: source-{source}\n          required: true\n          items:\n{selected}"
        ));
    }
    format!(
        "schema: podway.procedure/v2\nid: widest-evidence\nversion: \"1\"\nname: Widest evidence\npurpose: Charge the widest evidence declaration the schema admits.\nnode_definitions:\n{definitions}  consumer:\n    type: action\n    title: Consumer\n    intent: Read every selected value.\ngraph:\n  entry: source-0\n  nodes:\n{nodes}    - id: consumer\n      use: consumer\n      evidence_from:\n{evidence}      terminal: true\n"
    )
}

/// A consumer reading back from `sources` decision nodes, each carrying a goal assessment.
///
/// One complete goal-assessment record nearly fills its allocation, so this is the shape that
/// makes the decision-record allocation bind.
fn decision_readback_document(sources: usize) -> String {
    let mut definitions = String::new();
    let mut nodes = String::new();
    let mut evidence = String::new();
    for index in 0..sources {
        definitions.push_str(&format!(
            "  review-{index}:\n    type: decision\n    title: Review {index}\n    objective: Judge the work.\n    prompt: What is the outcome?\n    options:\n      - id: achieved\n        label: Achieved\n      - id: not-achieved\n        label: Not achieved\n      - id: superseded\n        label: Superseded\n    reason:\n      required: true\n    assessment:\n      target: session_goal\n      outcomes:\n        achieved: achieved\n        not-achieved: not_achieved\n        superseded: superseded\n"
        ));
        let next = if index + 1 == sources {
            "consumer".to_owned()
        } else {
            format!("review-{}", index + 1)
        };
        nodes.push_str(&format!(
            "    - id: review-{index}\n      use: review-{index}\n      routes:\n        achieved:\n          to: {next}\n          effect: advance\n        not-achieved:\n          to: {next}\n          effect: advance\n        superseded:\n          to: {next}\n          effect: advance\n"
        ));
        evidence.push_str(&format!(
            "        - node: review-{index}\n          required: true\n"
        ));
    }
    format!(
        "schema: podway.procedure/v2\nid: decision-readback\nversion: \"1\"\nname: Decision readback\npurpose: Exercise the decision-record allocation.\ngoal_tracking: true\nnode_definitions:\n{definitions}  consumer:\n    type: action\n    title: Consumer\n    intent: Read the decision records.\ngraph:\n  entry: review-0\n  nodes:\n{nodes}    - id: consumer\n      use: consumer\n      evidence_from:\n{evidence}      terminal: true\n"
    )
}

fn readback_document(selector: Option<&str>, required: bool, unreachable_consumer: bool) -> String {
    let evidence = match selector {
        Some(item) => format!(
            "        - node: source\n          required: {required}\n          items:\n            - {item}\n"
        ),
        None => format!("        - node: source\n          required: {required}\n"),
    };
    let source_outcome = if unreachable_consumer {
        "      terminal: true\n"
    } else {
        "      next: consumer\n"
    };
    format!(
        "schema: podway.procedure/v2\nid: readback-budget\nversion: \"1\"\nname: Readback budget\npurpose: Exercise worst-case selected item read-back accounting.\nnode_definitions:\n  producer:\n    type: action\n    title: Producer\n    intent: Record bounded source values.\n    items:\n      - id: huge-list\n        type: list\n        prompt: Record the list.\n        required: false\n        max_items: 200\n        max_item_length: 1000\n      - id: small-confirm\n        type: confirm\n        prompt: Confirm the result.\n        required: false\n  consumer:\n    type: action\n    title: Consumer\n    intent: Read the selected source values.\ngraph:\n  entry: source\n  nodes:\n    - id: source\n      use: producer\n{source_outcome}    - id: consumer\n      use: consumer\n      evidence_from:\n{evidence}      terminal: true\n"
    )
}

#[test]
fn v2grf001_accepts_a_dominating_assessment_and_an_unbounded_declared_rework_cycle() {
    assert_eq!(vet(BASE), Vec::new());

    let self_rework = mutate(
        "        not-achieved:\n          to: start\n          effect: rework\n",
        "        not-achieved:\n          to: review\n          effect: rework\n",
    );
    assert_eq!(
        vet(&self_rework),
        Vec::new(),
        "a rework self-loop contains a declared rework edge and its target dominates itself"
    );
}

#[test]
fn v2grf001_reports_unreachable_nodes_without_using_them_as_terminal_paths() {
    let source = mutate(
        "    - id: done\n",
        "    - id: orphan\n      use: finish\n      terminal: true\n    - id: done\n",
    );
    let findings = vet(&source);
    let unreachable = findings
        .iter()
        .find(|finding| finding.code() == AuthoringDiagnosticCode::UnreachableGraphNode)
        .expect("orphan must be unreachable");
    assert_eq!(unreachable.graph_node_id(), Some("orphan"));
    assert_eq!(unreachable.field(), "graph.nodes[orphan]");
    assert!(!codes(&source).contains(&"NO_TERMINAL_PATH"));
}

#[test]
fn v2grf001_reports_every_reachable_node_without_a_finite_terminal_path() {
    let source = mutate("      terminal: true\n", "      next: done\n");
    let findings = vet(&source);
    let no_terminal: Vec<&str> = findings
        .iter()
        .filter(|finding| finding.code() == AuthoringDiagnosticCode::NoTerminalPath)
        .filter_map(AuthoringDiagnostic::graph_node_id)
        .collect();
    assert_eq!(no_terminal, ["start", "review", "done"]);
    assert_has(&source, AuthoringDiagnosticCode::GraphCycleInvalid);
}

#[test]
fn v2grf001_rejects_an_advance_only_subcycle_inside_the_complete_graph() {
    let source = mutate(
        "        achieved:\n          to: done\n",
        "        achieved:\n          to: start\n",
    );
    let findings = vet(&source);
    let cycle = findings
        .iter()
        .find(|finding| finding.code() == AuthoringDiagnosticCode::GraphCycleInvalid)
        .expect("advance-only cycle must be rejected");
    assert_eq!(cycle.graph_node_id(), Some("start"));
    assert_eq!(cycle.related_graph_node_ids(), ["start", "review"]);
}

#[test]
fn v2grf001_uses_all_edges_for_declared_rework_dominance() {
    let source = mutate(
        "        not-achieved:\n          to: start\n          effect: rework\n",
        "        not-achieved:\n          to: done\n          effect: rework\n",
    );
    let finding = vet(&source)
        .into_iter()
        .find(|finding| finding.code() == AuthoringDiagnosticCode::ReworkTargetNotDominating)
        .expect("a downstream target cannot dominate its routing decision");
    assert_eq!(finding.graph_node_id(), Some("review"));
    assert_eq!(finding.related_graph_node_ids(), ["done"]);
    assert_eq!(
        finding.field(),
        "graph.nodes[review].routes[not-achieved].to"
    );
}

#[test]
fn v2grf001_required_evidence_must_strictly_dominate_and_must_not_be_skippable() {
    let downstream = mutate(
        "        - node: start\n          items:\n            - note\n",
        "        - node: done\n",
    );
    assert_has(
        &downstream,
        AuthoringDiagnosticCode::EvidenceSourceDoesNotDominateConsumer,
    );

    let skippable = mutate(
        "      use: work\n      next: review\n",
        "      use: work\n      skip:\n        allowed: true\n        reason_required: true\n      next: review\n",
    );
    assert_has(&skippable, AuthoringDiagnosticCode::SkippableEvidenceSource);

    let optional_downstream = downstream.replace(
        "        - node: done\n",
        "        - node: done\n          required: false\n",
    );
    assert!(
        !codes(&optional_downstream).contains(&"EVIDENCE_SOURCE_DOES_NOT_DOMINATE_CONSUMER"),
        "branch-specific optional evidence is permitted"
    );
}

#[test]
fn v2grf001_goal_assessment_must_dominate_each_reachable_terminal() {
    let source = mutate("      next: review\n", "      next: done\n");
    let finding = vet(&source)
        .into_iter()
        .find(|finding| {
            finding.code() == AuthoringDiagnosticCode::GoalAssessmentNotDominatingTerminal
        })
        .expect("the assessment is bypassed");
    assert_eq!(finding.graph_node_id(), Some("done"));
    assert_eq!(finding.related_graph_node_ids(), ["review"]);
}

#[test]
fn v2grf001_assessment_coverage_reports_each_unmapped_option_and_missing_outcome() {
    let unmapped = mutate(
        "      - id: superseded\n        label: Superseded\n",
        "      - id: superseded\n        label: Superseded\n      - id: deferred\n        label: Deferred\n",
    )
    .replacen(
        "        superseded:\n          to: done\n          effect: advance\n",
        "        superseded:\n          to: done\n          effect: advance\n        deferred:\n          to: done\n          effect: advance\n",
        1,
    );
    let finding = vet(&unmapped)
        .into_iter()
        .find(|finding| finding.code() == AuthoringDiagnosticCode::GoalAssessmentOptionUnmapped)
        .expect("deferred must be unmapped");
    assert_eq!(finding.node_definition_id(), Some("assess"));
    assert_eq!(
        finding.field(),
        "node_definitions[assess].options[deferred]"
    );

    let missing = mutate(
        "        not-achieved: not_achieved\n        superseded: superseded\n",
        "        not-achieved: achieved\n        superseded: achieved\n",
    );
    let missing_outcomes = vet(&missing)
        .iter()
        .filter(|finding| {
            finding.code() == AuthoringDiagnosticCode::GoalAssessmentOutcomeUnreachable
        })
        .count();
    assert_eq!(missing_outcomes, 2);
}

#[test]
fn v2grf001_findings_are_byte_stable_and_sorted_by_source_position() {
    let source = mutate("      terminal: true\n", "      next: done\n");
    let first = vet(&source);
    let expected = serde_json::to_vec(&first).expect("findings serialize");
    for _ in 0..100 {
        assert_eq!(
            serde_json::to_vec(&vet(&source)).expect("findings serialize"),
            expected
        );
    }
    assert!(first.windows(2).all(|pair| {
        let left = &pair[0];
        let right = &pair[1];
        (
            left.location().line(),
            left.location().column(),
            left.code().as_str(),
            left.field(),
        ) <= (
            right.location().line(),
            right.location().column(),
            right.code().as_str(),
            right.field(),
        )
    }));
}

#[test]
fn v2grf002_rejects_a_placement_whose_complete_static_next_content_is_too_large() {
    let source = oversized_static_document();
    let finding = vet(&source)
        .into_iter()
        .find(|finding| finding.code() == AuthoringDiagnosticCode::NextStaticBudgetExceeded)
        .expect("the combined static fields and suggestions must exceed the placement budget");
    assert_eq!(finding.graph_node_id(), Some("work"));
    assert_eq!(finding.node_definition_id(), Some("work"));
    assert_eq!(finding.field(), "graph.nodes[work]");
}

#[test]
fn v2scl005_a_selector_narrows_the_evidence_allocations_it_charges() {
    // ADR-0024 charges metadata per selected item and a preview slot only for an explicit
    // selector, so a selector must lower both charges. Neither can overflow here: both
    // allocations are sized to hold the schema maximum, which the unreachability test proves.
    let all_items = placement_charges(&readback_document(None, true, false));
    let selected = placement_charges(&readback_document(Some("small-confirm"), true, false));
    assert!(
        selected.readback() < all_items.readback(),
        "a selector must charge less metadata than reading every item"
    );
    assert_eq!(
        all_items.evidence_preview(),
        0,
        "an omitted selector requests no preview"
    );
    assert!(
        selected.evidence_preview() > 0 && selected.page_tokens() > 0,
        "an explicit selector spends a preview slot and publishes a continuation token"
    );
}

#[test]
fn v2scl005_the_preview_and_token_allocations_hold_every_slot_a_selector_can_ask_for() {
    // A selector caps previews at 32 slots and one token per slot however wide the selection is,
    // so these two allocations hold the maximum by construction. Metadata does not: see the
    // selector-less case below.
    let widest = placement_charges(&widest_evidence_document());
    assert!(widest.evidence_preview() <= podway_config::EVIDENCE_PREVIEW_BUDGET);
    assert!(widest.page_tokens() <= podway_config::PAGE_TOKEN_BUDGET);
    let codes = codes(&widest_evidence_document());
    assert!(
        widest.charged_total()
            <= podway_config::NEXT_FRAME_BUDGET - podway_config::NEXT_SERIALIZATION_RESERVE
    );
    for unreachable in [
        "EVIDENCE_PREVIEW_BUDGET_EXCEEDED",
        "PAGE_TOKEN_BUDGET_EXCEEDED",
        "READBACK_BUDGET_EXCEEDED",
        "NEXT_FRAME_BUDGET_EXCEEDED",
    ] {
        assert!(
            !codes.contains(&unreachable),
            "{unreachable} fired on the widest explicit selection"
        );
    }
}

#[test]
fn v2scl005_a_wide_selection_cannot_exceed_the_published_record_ceiling() {
    // Metadata costs the same for every item kind, so many small items charge few bytes while
    // still publishing more records than `next-result/v3` admits. The byte allocation cannot
    // catch that, which is why the record ceiling is enforced on its own.
    let inside = placement_charges(&narrow_selectorless_document(3, 42));
    assert_eq!(inside.readback_records(), 126);
    assert!(inside.readback() <= podway_config::READBACK_BUDGET);
    assert!(!codes(&narrow_selectorless_document(3, 42)).contains(&"READBACK_BUDGET_EXCEEDED"));

    let over = placement_charges(&narrow_selectorless_document(3, 43));
    assert_eq!(over.readback_records(), 129);
    assert!(
        over.readback() <= podway_config::READBACK_BUDGET,
        "the byte allocation must not be what rejects this: {} bytes",
        over.readback()
    );
    let findings = vet(&narrow_selectorless_document(3, 43));
    let finding = findings
        .iter()
        .find(|finding| finding.code() == AuthoringDiagnosticCode::ReadbackBudgetExceeded)
        .expect("129 metadata records exceed what one response may carry");
    assert!(finding.message().contains("129"));
    assert_eq!(finding.field(), "graph.nodes[consumer].evidence_from");
}

#[test]
fn v2scl005_a_selectorless_reference_can_exceed_the_metadata_allocation() {
    // An omitted selector means every item of the source, so a reference to a 128-item definition
    // charges far more metadata than the eight-by-sixteen explicit maximum. One such reference
    // fits its allocation and two do not, which is what makes READBACK_BUDGET_EXCEEDED a live
    // diagnostic rather than fail-closed defense.
    let one = placement_charges(&selectorless_evidence_document(1));
    assert!(
        one.readback() <= podway_config::READBACK_BUDGET,
        "one selector-less reference charges {} bytes of metadata",
        one.readback()
    );
    assert!(!codes(&selectorless_evidence_document(1)).contains(&"READBACK_BUDGET_EXCEEDED"));

    let two = placement_charges(&selectorless_evidence_document(2));
    assert_eq!(two.readback(), 367_736);
    assert!(two.readback() > podway_config::READBACK_BUDGET);
    let findings = vet(&selectorless_evidence_document(2));
    let finding = findings
        .iter()
        .find(|finding| finding.code() == AuthoringDiagnosticCode::ReadbackBudgetExceeded)
        .expect("two selector-less 128-item references cannot fit the metadata allocation");
    assert_eq!(finding.graph_node_id(), Some("consumer"));
    assert_eq!(finding.field(), "graph.nodes[consumer].evidence_from");
}

#[test]
fn v2scl005_a_second_decision_source_exceeds_the_decision_record_allocation() {
    // One complete goal-assessment record nearly fills its allocation, so reading back from a
    // second decision source is the evidence declaration that actually fails.
    assert!(
        !codes(&decision_readback_document(1)).contains(&"DECISION_RECORD_BUDGET_EXCEEDED"),
        "one decision source must fit its allocation"
    );
    let findings = vet(&decision_readback_document(2));
    let finding = findings
        .iter()
        .find(|finding| finding.code() == AuthoringDiagnosticCode::DecisionRecordBudgetExceeded)
        .expect("two complete decision records cannot fit their allocation");
    assert_eq!(finding.graph_node_id(), Some("consumer"));
    assert_eq!(finding.field(), "graph.nodes[consumer].evidence_from");
    assert_eq!(finding.related_graph_node_ids(), ["review-0", "review-1"]);
}

#[test]
fn v2grf002_optional_and_unreachable_readback_is_still_charged_at_its_worst_case() {
    // An optional or unreachable consumer is charged exactly like a required, reachable one: the
    // budget bounds what the response could carry, not what this run happens to reach.
    let required = placement_charges(&readback_document(None, true, false));
    let optional = placement_charges(&readback_document(None, false, false));
    assert_eq!(optional.readback(), required.readback());

    let unreachable = readback_document(None, false, true);
    let findings = vet(&unreachable);
    assert!(findings.iter().any(|finding| {
        finding.code() == AuthoringDiagnosticCode::UnreachableGraphNode
            && finding.graph_node_id() == Some("consumer")
    }));
    assert_eq!(
        placement_charges(&unreachable).readback(),
        required.readback(),
        "an unreachable consumer is still charged at its worst case"
    );
}

#[test]
fn v2grf002_valid_declared_rework_does_not_create_a_cumulative_budget() {
    assert!(
        !codes(BASE).iter().any(|code| {
            matches!(
                *code,
                "NEXT_STATIC_BUDGET_EXCEEDED" | "READBACK_BUDGET_EXCEEDED"
            )
        }),
        "vet charges one immutable placement projection, never a traversal count"
    );
}
