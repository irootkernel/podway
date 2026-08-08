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
        Ok(ParsedProcedure::V1(_)) => panic!("expected Procedure v2"),
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
