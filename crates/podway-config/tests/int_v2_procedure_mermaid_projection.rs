//! V2GRF-004 deterministic Mermaid review projection.

use podway_config::{
    AuthoringContext, ConfigError, ParsedProcedure, ProcedureDocumentFormat, ValidatedProcedureV2,
    normalize_procedure_v2_graph, parse_procedure_document, project_procedure_v2_graph,
    project_procedure_v2_mermaid, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::SOURCE_PROJECTION_MAX_CHARACTERS;
use sha2::{Digest as _, Sha256};

const MERMAID_YAML: &str = r#"schema: podway.procedure/v2
id: mermaid-review
version: "1"
name: Mermaid review
purpose: Exercise every Mermaid review convention.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: 'Implement "safe" <change> & verify'
    intent: Perform the work.
  choose:
    type: decision
    title: Choose route
    objective: Select the next route.
    prompt: Which route?
    options:
      - id: passed
        label: Passed
      - id: retry
        label: Retry
    reason:
      required: true
  assess:
    type: decision
    title: Assess the session goal
    objective: Assess the goal.
    prompt: What is the outcome?
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
    title: Record closeout
    intent: Complete the procedure.
graph:
  entry: implement-change
  nodes:
    - id: implement-change
      use: work
      skip:
        allowed: true
        reason_required: true
      next: choose-route
    - id: choose-route
      use: choose
      routes:
        retry:
          to: implement-change
          effect: rework
        passed:
          to: assess-session-goal
          effect: advance
    - id: assess-session-goal
      use: assess
      evidence_from:
        - node: implement-change
          required: false
      routes:
        superseded:
          to: record-closeout
          effect: advance
        achieved:
          to: record-closeout
          effect: advance
        not-achieved:
          to: implement-change
          effect: rework
    - id: record-closeout
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - implement-change
    - choose-route
"#;

fn admit(source: &str, format: ProcedureDocumentFormat) -> ValidatedProcedureV2 {
    let parsed =
        match parse_procedure_document(source.as_bytes(), format).expect("fixture must parse") {
            ParsedProcedure::V2(parsed) => parsed,
        };
    let validated = validate_procedure_v2(parsed).expect("fixture must validate");
    let context = AuthoringContext::new("mermaid.yaml", source, format);
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "a published projection fixture must be vetted: {diagnostics:#?}"
    );
    validated
}

fn render(source: &str, format: ProcedureDocumentFormat) -> (ValidatedProcedureV2, String, String) {
    let validated = admit(source, format);
    let graph = project_procedure_v2_graph(&validated).expect("graph projection must fit");
    let mermaid = project_procedure_v2_mermaid(graph.graph()).expect("Mermaid projection must fit");
    (
        validated,
        mermaid.projection().to_owned(),
        mermaid.projection_digest().as_str().to_owned(),
    )
}

fn json_over_mermaid_under_source() -> String {
    let option_ids = (0..8)
        .map(|index| format!("option-{index:02}-{}", "x".repeat(54)))
        .collect::<Vec<_>>();
    let node_ids = (0..63)
        .map(|index| format!("node-{index:02}-{}", "x".repeat(56)))
        .collect::<Vec<_>>();
    let terminal_id = format!("terminal-{}", "x".repeat(55));
    let options = option_ids
        .iter()
        .map(|id| format!("      - id: {id}\n        label: Choice\n"))
        .collect::<String>();
    let nodes = node_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let target = node_ids.get(index + 1).unwrap_or(&terminal_id);
            let routes = option_ids
                .iter()
                .map(|option| {
                    format!(
                        "        {option}:\n          to: {target}\n          effect: advance\n"
                    )
                })
                .collect::<String>();
            format!("    - id: {id}\n      use: choose\n      routes:\n{routes}")
        })
        .collect::<String>();
    format!(
        "schema: podway.procedure/v2\nid: independent-cap\nversion: \"1\"\nname: Independent cap\npurpose: Prove renderer budgets are independent.\nnode_definitions:\n  choose:\n    type: decision\n    title: Choose\n    objective: Select a route.\n    prompt: Which route?\n    options:\n{options}    reason:\n      required: true\n  finish:\n    type: action\n    title: Finish\n    intent: Finish.\ngraph:\n  entry: {}\n  nodes:\n{nodes}    - id: {terminal_id}\n      use: finish\n      terminal: true\n",
        node_ids[0]
    )
}

#[test]
fn v2grf004_mermaid_matches_the_complete_review_golden() {
    let (validated, projection, digest) = render(MERMAID_YAML, ProcedureDocumentFormat::Yaml);
    let expected = format!(
        concat!(
            "%% podway.procedure/v2\n",
            "%% procedure-digest: {}\n\n",
            "flowchart TD\n",
            "    implement_change[\"Implement &quot;safe&quot; &lt;change&gt; &amp; verify · entry · skippable\"]\n",
            "    choose_route{{\"Choose route\"}}\n",
            "    assess_session_goal{{{{\"Assess the session goal\"}}}}\n",
            "    record_closeout[\"Record closeout · terminal\"]\n\n",
            "    implement_change --> choose_route\n",
            "    choose_route -->|passed · advance| assess_session_goal\n",
            "    choose_route -->|retry · rework| implement_change\n",
            "    assess_session_goal -->|achieved · advance| record_closeout\n",
            "    assess_session_goal -->|not-achieved · rework| implement_change\n",
            "    assess_session_goal -->|superseded · advance| record_closeout\n\n",
            "    classDef manual_rework_target stroke-dasharray:4 3\n",
            "    class implement_change,choose_route manual_rework_target"
        ),
        validated.digest().as_str()
    );

    assert_eq!(projection, expected);
    assert_eq!(
        digest,
        "sha256:f7400481345fded2137686d34fb46c0472a7a349751fe2f7a7376e6126f61176"
    );
    assert!(!projection.ends_with('\n'));
    assert!(!projection.contains("evidence_from"));
}

#[test]
fn v2grf004_mermaid_is_not_blocked_by_the_json_projection_budget() {
    let source = json_over_mermaid_under_source();
    let validated = admit(&source, ProcedureDocumentFormat::Yaml);
    assert!(matches!(
        project_procedure_v2_graph(&validated),
        Err(ConfigError::OutOfBounds {
            field: "graph projection",
            min: 1,
            max: SOURCE_PROJECTION_MAX_CHARACTERS,
            actual,
        }) if actual > SOURCE_PROJECTION_MAX_CHARACTERS
    ));

    let graph = normalize_procedure_v2_graph(&validated).expect("normalization has no format cap");
    let mermaid = project_procedure_v2_mermaid(&graph).expect("Mermaid bytes fit their own cap");
    assert!(mermaid.projection().chars().count() <= SOURCE_PROJECTION_MAX_CHARACTERS);
}

#[test]
fn v2grf004_mermaid_rejects_its_own_complete_projection_over_budget() {
    let source = json_over_mermaid_under_source().replace(
        "    title: Choose\n",
        &format!("    title: '{}'\n", "&".repeat(120)),
    );
    let validated = admit(&source, ProcedureDocumentFormat::Yaml);
    let graph = normalize_procedure_v2_graph(&validated).expect("normalization has no format cap");

    assert!(matches!(
        project_procedure_v2_mermaid(&graph),
        Err(ConfigError::OutOfBounds {
            field: "graph projection",
            min: 1,
            max: SOURCE_PROJECTION_MAX_CHARACTERS,
            actual,
        }) if actual > SOURCE_PROJECTION_MAX_CHARACTERS
    ));
}

#[test]
fn v2grf004_projection_digest_hashes_the_exact_mermaid_bytes() {
    let (_, projection, digest) = render(MERMAID_YAML, ProcedureDocumentFormat::Yaml);
    assert_eq!(
        digest,
        format!("sha256:{:x}", Sha256::digest(projection.as_bytes()))
    );
}

#[test]
fn v2grf004_unicode_line_separators_cannot_inject_mermaid_statements() {
    let source = MERMAID_YAML.replace(
        "'Implement \"safe\" <change> & verify'",
        "'Before\u{2028}flowchart LR\u{2029}After'",
    );
    let (_, projection, _) = render(&source, ProcedureDocumentFormat::Yaml);

    assert!(projection.contains("Before&#8232;flowchart LR&#8233;After"));
    assert!(!projection.contains('\u{2028}'));
    assert!(!projection.contains('\u{2029}'));
}

#[test]
fn v2grf004_equivalent_yaml_and_json_render_identically() {
    let yaml = admit(MERMAID_YAML, ProcedureDocumentFormat::Yaml);
    let graph_from_yaml = project_procedure_v2_graph(&yaml).expect("graph must fit");
    let from_yaml =
        project_procedure_v2_mermaid(graph_from_yaml.graph()).expect("Mermaid must fit");

    let json = admit(
        yaml.canonical_json().as_str(),
        ProcedureDocumentFormat::Json,
    );
    let graph_from_json = project_procedure_v2_graph(&json).expect("graph must fit");
    let from_json =
        project_procedure_v2_mermaid(graph_from_json.graph()).expect("Mermaid must fit");

    assert_eq!(from_yaml, from_json);
}

#[test]
fn v2grf004_descriptive_title_edits_change_only_labels_and_bound_metadata() {
    let (_, original, original_digest) = render(MERMAID_YAML, ProcedureDocumentFormat::Yaml);
    let changed_source = MERMAID_YAML.replace("Choose route", "Choose a safe route");
    let (_, changed, changed_digest) = render(&changed_source, ProcedureDocumentFormat::Yaml);

    assert_ne!(original, changed);
    assert_ne!(original_digest, changed_digest);
    assert!(changed.contains("choose_route{\"Choose a safe route\"}"));
    assert!(changed.contains("choose_route -->|passed · advance| assess_session_goal"));
}

#[test]
fn v2grf004_single_terminal_omits_empty_edge_and_style_sections() {
    let source = r#"schema: podway.procedure/v2
id: one-node
version: "1"
name: One node
purpose: Verify an edge-free Mermaid graph.
node_definitions:
  finish:
    type: action
    title: Finish
    intent: Finish.
graph:
  entry: finish-now
  nodes:
    - id: finish-now
      use: finish
      terminal: true
"#;
    let (validated, projection, _) = render(source, ProcedureDocumentFormat::Yaml);

    assert_eq!(
        projection,
        format!(
            "%% podway.procedure/v2\n%% procedure-digest: {}\n\nflowchart TD\n    finish_now[\"Finish · entry · terminal\"]",
            validated.digest().as_str()
        )
    );
    assert!(!projection.contains("-->"));
    assert!(!projection.contains("classDef"));
}

#[test]
fn v2grf004_flowchart_keywords_are_escaped_in_nodes_edges_and_classes() {
    let source = r#"schema: podway.procedure/v2
id: reserved-node-ids
version: "1"
name: Reserved node IDs
purpose: Keep valid Procedure identifiers valid in Mermaid.
node_definitions:
  step:
    type: action
    title: Step
    intent: Continue.
graph:
  entry: graph
  nodes:
    - id: graph
      use: step
      next: call
    - id: call
      use: step
      next: end
    - id: end
      use: step
      terminal: true
manual_rework:
  allowed_targets:
    - call
    - end
"#;
    let (validated, projection, _) = render(source, ProcedureDocumentFormat::Yaml);

    assert_eq!(
        projection,
        format!(
            concat!(
                "%% podway.procedure/v2\n",
                "%% procedure-digest: {}\n\n",
                "flowchart TD\n",
                "    n__graph[\"Step · entry\"]\n",
                "    n__call[\"Step\"]\n",
                "    n__end[\"Step · terminal\"]\n\n",
                "    n__graph --> n__call\n",
                "    n__call --> n__end\n\n",
                "    classDef manual_rework_target stroke-dasharray:4 3\n",
                "    class n__call,n__end manual_rework_target"
            ),
            validated.digest().as_str()
        )
    );
}
