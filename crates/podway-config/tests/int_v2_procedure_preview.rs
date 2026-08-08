//! V2GRF-007 read-only Procedure v2 preview aggregation.

use podway_config::{FormatRequest, ProcedureDocumentFormat, preview_procedure_v2};
use podway_core::{AuthoringSeverity, MAX_AUTHORING_DIAGNOSTICS};

const CLEAN_YAML: &str = r#"schema: podway.procedure/v2
id: preview-clean
version: "1"
name: Preview clean
purpose: Exercise the complete preview summary without lint findings.
node_definitions:
  gather:
    type: action
    title: Gather the inputs
    intent: Collect every input the review needs.
    items:
      - id: notes
        type: text
        prompt: Record the gathered notes.
        required: true
  review:
    type: decision
    title: Review the work
    objective: Decide whether the gathered work is complete.
    prompt: Is the gathered work complete?
    evidence_guidance:
      - Read the gathered notes before deciding.
    options:
      - id: complete
        label: Work is complete
        criteria: Every gathered input is present and correct.
      - id: incomplete
        label: Work is incomplete
        criteria: Some gathered input is missing or wrong.
    reason:
      required: true
      prompt: Explain why the work is or is not complete.
  publish:
    type: action
    title: Publish the result
    intent: Record the published outcome.
graph:
  entry: gather-inputs
  nodes:
    - id: gather-inputs
      use: gather
      next: review-work
    - id: review-work
      use: review
      evidence_from:
        - node: gather-inputs
          required: true
          items:
            - notes
      routes:
        complete:
          to: publish-result
          effect: advance
        incomplete:
          to: gather-inputs
          effect: rework
    - id: publish-result
      use: publish
      terminal: true
manual_rework:
  allowed_targets:
    - gather-inputs
"#;

const MINIMAL_YAML: &str = r#"schema: podway.procedure/v2
id: minimal
version: "1"
name: Minimal
purpose: The smallest legal Procedure v2 document.
node_definitions:
  work:
    type: action
    title: Work
    intent: Do the work.
graph:
  entry: only
  nodes:
    - id: only
      use: work
      terminal: true
"#;

const GOAL_YAML: &str = r#"schema: podway.procedure/v2
id: goal-preview
version: "2"
name: Goal preview
purpose: Preview a session-goal assessment before recording the result.
goal_tracking: true
node_definitions:
  prepare:
    type: action
    title: Prepare evidence
    intent: Gather the evidence needed for assessment.
    items:
      - id: notes
        type: text
        prompt: Record the evidence.
        required: true
  assess:
    type: decision
    title: Assess the goal
    objective: Decide the session-goal outcome.
    prompt: What is the session-goal outcome?
    evidence_guidance:
      - Read the prepared evidence before deciding.
    options:
      - id: achieved
        label: Goal achieved
        criteria: Every criterion is satisfied.
      - id: not-achieved
        label: Goal not achieved
        criteria: At least one criterion remains unmet.
      - id: superseded
        label: Goal superseded
        criteria: The goal no longer describes the work.
    reason:
      required: true
      prompt: Explain the assessment outcome.
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  finish:
    type: action
    title: Record the result
    intent: Persist the assessed result.
graph:
  entry: prepare
  nodes:
    - id: prepare
      use: prepare
      next: assess-goal
    - id: assess-goal
      use: assess
      evidence_from:
        - node: prepare
          required: true
          items:
            - notes
      routes:
        achieved:
          to: done
          effect: advance
        not-achieved:
          to: prepare
          effect: rework
        superseded:
          to: done
          effect: advance
    - id: done
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - prepare
"#;

fn preview<'a>(
    source: &'a str,
    source_path: &'a str,
    format: ProcedureDocumentFormat,
) -> podway_config::ProcedurePreviewReportV2 {
    preview_procedure_v2(FormatRequest {
        source,
        source_path,
        format,
    })
}

#[test]
fn v2grf007_rich_preview_has_exact_summary_graph_and_confirmed_start_argv() {
    let report = preview(CLEAN_YAML, "workflow.yaml", ProcedureDocumentFormat::Yaml);
    assert!(report.admissible());
    assert_eq!(
        (
            report.checks().validate(),
            report.checks().vet(),
            report.checks().lint()
        ),
        (true, true, true)
    );
    assert!(report.diagnostics().is_empty());

    let details = report.details().expect("admissible details");
    assert_eq!(details.procedure_schema(), "podway.procedure/v2");
    assert_eq!(details.procedure_id(), "preview-clean");
    assert_eq!(details.procedure_version(), "1");
    assert_eq!(
        details.purpose(),
        "Exercise the complete preview summary without lint findings."
    );
    assert!(!details.goal_tracking());
    assert!(details.goal_assessment_graph_node_ids().is_empty());
    let summary = details.summary();
    assert_eq!(
        (
            summary.definition_count(),
            summary.graph_node_count(),
            summary.action_node_count(),
            summary.decision_node_count(),
            summary.route_count(),
            summary.cycle_count(),
            summary.evidence_reference_count(),
            summary.skippable_node_count(),
            summary.manual_rework_target_count()
        ),
        (3, 3, 2, 1, 2, 1, 1, 0, 1),
    );
    assert_eq!(details.graph().entry_graph_node_id(), "gather-inputs");
    assert_eq!(
        details.graph().terminal_graph_node_ids(),
        ["publish-result"]
    );
    assert_eq!(
        details
            .graph()
            .nodes()
            .iter()
            .map(|node| node.graph_node_id())
            .collect::<Vec<_>>(),
        ["gather-inputs", "review-work", "publish-result"]
    );
    assert_eq!(
        details.graph().edges().len(),
        3,
        "one action next plus two decision routes"
    );
    assert_eq!(details.graph().edges()[0].option_id(), None);
    assert_eq!(details.graph().edges()[1].option_id(), Some("complete"));
    assert_eq!(details.graph().edges()[2].option_id(), Some("incomplete"));
    assert!(
        details
            .mermaid()
            .starts_with("%% podway.procedure/v2\n%% procedure-digest: ")
    );
    assert!(!details.mermaid().ends_with('\n'));
    assert_eq!(details.start_suggestion().command(), "session.start");
    assert_eq!(
        details.start_suggestion().argv(),
        [
            "podway",
            "start",
            "--procedure",
            "workflow.yaml",
            "--expect-procedure-digest",
            details.procedure_digest().as_str(),
            "--task",
            "<title>"
        ]
    );
}

#[test]
fn v2grf007_goal_tracking_and_assessment_placement_are_exposed() {
    let report = preview(GOAL_YAML, "goal.yaml", ProcedureDocumentFormat::Yaml);
    assert!(report.admissible(), "{:?}", report.diagnostics());
    let details = report.details().expect("admissible details");
    assert!(details.goal_tracking());
    assert_eq!(details.goal_assessment_graph_node_ids(), ["assess-goal"]);
    assert_eq!(details.summary().route_count(), 3);
    assert_eq!(details.summary().evidence_reference_count(), 1);
}

#[test]
fn v2grf007_lint_warning_is_retained_without_blocking_admissibility() {
    let report = preview(MINIMAL_YAML, "minimal.yaml", ProcedureDocumentFormat::Yaml);
    assert!(report.admissible());
    assert_eq!(
        (
            report.checks().validate(),
            report.checks().vet(),
            report.checks().lint()
        ),
        (true, true, false)
    );
    assert_eq!(report.diagnostics().len(), 1);
    assert_eq!(
        report.diagnostics()[0].code().as_str(),
        "NO_REACTIVATION_PATH"
    );
    assert_eq!(
        report.diagnostics()[0].severity(),
        AuthoringSeverity::Warning
    );
    assert!(report.details().is_some());
}

#[test]
fn v2grf007_parse_and_validation_failures_stop_with_one_bounded_diagnostic() {
    for source in [
        "not: [valid",
        &MINIMAL_YAML.replace("use: work", "use: missing"),
    ] {
        let report = preview(source, "bad.yaml", ProcedureDocumentFormat::Yaml);
        assert!(!report.admissible());
        assert_eq!(
            (
                report.checks().validate(),
                report.checks().vet(),
                report.checks().lint()
            ),
            (false, false, false)
        );
        assert!(report.details().is_none());
        assert_eq!(report.diagnostics().len(), 1);
        assert_eq!(report.diagnostics_total(), 1);
        assert!(!report.diagnostics_truncated());
    }
}

#[test]
fn v2grf007_vet_failure_still_runs_lint_and_withholds_details() {
    let source = MINIMAL_YAML.replace(
        "    - id: only\n",
        "    - id: stranded\n      use: work\n      terminal: true\n    - id: only\n",
    );
    let report = preview(&source, "bad-graph.yaml", ProcedureDocumentFormat::Yaml);
    assert!(!report.admissible());
    assert_eq!(
        (
            report.checks().validate(),
            report.checks().vet(),
            report.checks().lint()
        ),
        (true, false, false)
    );
    let codes = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"UNREACHABLE_GRAPH_NODE"));
    assert!(
        codes.contains(&"NO_REACTIVATION_PATH"),
        "lint must still run: {codes:?}"
    );
    assert!(report.diagnostics().len() <= MAX_AUTHORING_DIAGNOSTICS);
    let first_warning = report
        .diagnostics()
        .iter()
        .position(|diagnostic| diagnostic.severity() == AuthoringSeverity::Warning)
        .expect("lint warning");
    assert!(
        report.diagnostics()[..first_warning]
            .iter()
            .all(|diagnostic| diagnostic.severity() == AuthoringSeverity::Error)
    );
}

#[test]
fn v2grf007_yaml_and_json_are_semantically_equivalent_and_repeatable() {
    let yaml_value: serde_yaml::Value = serde_yaml::from_str(CLEAN_YAML).expect("yaml fixture");
    let json = serde_json::to_string(&yaml_value).expect("json equivalent");
    let source = CLEAN_YAML.to_owned();
    let source_before = source.clone();
    let yaml = preview(&source, "workflow.yaml", ProcedureDocumentFormat::Yaml);
    let json = preview(&json, "workflow.yaml", ProcedureDocumentFormat::Json);
    assert_eq!(yaml, json);
    for _ in 0..20 {
        assert_eq!(
            preview(&source, "workflow.yaml", ProcedureDocumentFormat::Yaml),
            yaml
        );
    }
    assert_eq!(source, source_before, "preview must not mutate its source");
}

#[test]
fn v2grf007_cycle_count_is_cyclic_scc_regions_including_self_loops() {
    let source = CLEAN_YAML
        .replace(
            "  publish:\n    type: action\n    title: Publish the result\n    intent: Record the published outcome.\n",
            "  publish:\n    type: decision\n    title: Publish the result\n    objective: Decide whether to finish.\n    prompt: Finish?\n    options:\n      - id: finish\n        label: Finish now\n        criteria: The result is ready.\n      - id: revisit\n        label: Revisit publication\n        criteria: Publication needs another pass.\n    reason:\n      required: true\n      prompt: Explain the publication choice.\n  done:\n    type: action\n    title: Finish the result\n    intent: Record the terminal outcome.\n",
        )
        .replace(
            "    - id: publish-result\n      use: publish\n      terminal: true\n",
            "    - id: publish-result\n      use: publish\n      routes:\n        finish:\n          to: done\n          effect: advance\n        revisit:\n          to: publish-result\n          effect: rework\n    - id: done\n      use: done\n      terminal: true\n",
        );
    let report = preview(&source, "cycles.yaml", ProcedureDocumentFormat::Yaml);
    assert!(report.admissible(), "{:?}", report.diagnostics());
    assert_eq!(
        report.details().expect("details").summary().cycle_count(),
        2
    );
}
