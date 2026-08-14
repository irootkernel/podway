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

fn exact_readback_boundary_document(last_item_max_length: u32) -> String {
    let maxima = [16_384, 16_384, 16_384, 16_384, 16_384, last_item_max_length];
    let items = maxima
        .iter()
        .enumerate()
        .map(|(index, maximum)| {
            format!(
                "      - id: text-{index}\n        type: text\n        prompt: Value {index}.\n        required: false\n        max_length: {maximum}\n"
            )
        })
        .collect::<String>();
    format!(
        "schema: podway.procedure/v2\nid: readback-boundary\nversion: \"1\"\nname: Readback boundary\npurpose: Pin exact per-placement read-back accounting at the accepted limit.\nnode_definitions:\n  producer:\n    type: action\n    title: Producer\n    intent: Record six bounded text values.\n    items:\n{items}  consumer:\n    type: action\n    title: Consumer\n    intent: Read every recorded source value.\ngraph:\n  entry: source\n  nodes:\n    - id: source\n      use: producer\n      next: consumer\n    - id: consumer\n      use: consumer\n      evidence_from:\n        - node: source\n      terminal: true\n"
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
fn v2grf002_readback_charges_all_items_without_a_selector_and_only_selected_items_with_one() {
    let all_items = readback_document(None, true, false);
    let finding = vet(&all_items)
        .into_iter()
        .find(|finding| finding.code() == AuthoringDiagnosticCode::ReadbackBudgetExceeded)
        .expect("a 200 by 1000 list cannot fit in read-back");
    assert_eq!(finding.graph_node_id(), Some("consumer"));
    assert_eq!(finding.node_definition_id(), Some("consumer"));
    assert_eq!(finding.field(), "graph.nodes[consumer].evidence_from");
    assert_eq!(finding.related_graph_node_ids(), ["source"]);

    let selected = readback_document(Some("small-confirm"), true, false);
    assert!(
        !codes(&selected).contains(&"READBACK_BUDGET_EXCEEDED"),
        "a selector excludes the unselected maximal list value"
    );
}

#[test]
fn v2grf002_accepts_an_exact_readback_budget_and_rejects_the_next_authored_scalar() {
    // Required array/metadata fields charge 4,604 bytes. Each text item charges 608 bytes plus
    // six times max_length. Six maxima summing to 86,006 therefore charge exactly 524,288.
    let exact = exact_readback_boundary_document(4_086);
    assert!(
        !codes(&exact).contains(&"READBACK_BUDGET_EXCEEDED"),
        "equality with READBACK_BUDGET must be accepted"
    );

    let over = exact_readback_boundary_document(4_087);
    assert_has(&over, AuthoringDiagnosticCode::ReadbackBudgetExceeded);
}

#[test]
fn v2grf002_optional_and_unreachable_readback_is_still_charged_at_its_worst_case() {
    let optional = readback_document(None, false, false);
    assert_has(&optional, AuthoringDiagnosticCode::ReadbackBudgetExceeded);

    let unreachable = readback_document(None, false, true);
    let findings = vet(&unreachable);
    assert!(findings.iter().any(|finding| {
        finding.code() == AuthoringDiagnosticCode::UnreachableGraphNode
            && finding.graph_node_id() == Some("consumer")
    }));
    assert!(findings.iter().any(|finding| {
        finding.code() == AuthoringDiagnosticCode::ReadbackBudgetExceeded
            && finding.graph_node_id() == Some("consumer")
    }));
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
