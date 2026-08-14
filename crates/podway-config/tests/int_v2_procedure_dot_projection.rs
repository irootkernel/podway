//! V2GRF-006 deterministic Graphviz DOT projection.

use podway_config::{
    AuthoringContext, ConfigError, ParsedProcedure, ProcedureDocumentFormat, ValidatedProcedureV2,
    normalize_procedure_v2_graph, parse_procedure_document, project_procedure_v2_dot,
    project_procedure_v2_graph, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::SOURCE_PROJECTION_MAX_CHARACTERS;
use sha2::{Digest as _, Sha256};

const DOT_YAML: &str = r#"schema: podway.procedure/v2
id: dot-review
version: "1"
name: DOT review
purpose: Exercise every DOT review convention.
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
    let context = AuthoringContext::new("dot.yaml", source, format);
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "a published projection fixture must be vetted: {diagnostics:#?}"
    );
    validated
}

fn render(source: &str, format: ProcedureDocumentFormat) -> (ValidatedProcedureV2, String, String) {
    let validated = admit(source, format);
    let graph = normalize_procedure_v2_graph(&validated).expect("normalization must succeed");
    let dot = project_procedure_v2_dot(&graph).expect("DOT projection must fit");
    (
        validated,
        dot.projection().to_owned(),
        dot.projection_digest().as_str().to_owned(),
    )
}

fn json_over_dot_under_source() -> String {
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
fn v2grf006_dot_matches_the_complete_graph_golden() {
    let (validated, projection, digest) = render(DOT_YAML, ProcedureDocumentFormat::Yaml);
    let expected = format!(
        concat!(
            "digraph podway {{\n",
            "    // podway.procedure/v2\n",
            "    // procedure-digest: {}\n",
            "    rankdir=TB;\n\n",
            "    \"implement-change\" [label=\"Implement <U+0022>safe<U+0022> <U+003C>change<U+003E> <U+0026> verify · entry · skippable · manual rework target\", shape=box, style=dashed];\n",
            "    \"choose-route\" [label=\"Choose route · decision · manual rework target\", shape=diamond, style=dashed];\n",
            "    \"assess-session-goal\" [label=\"Assess the session goal · goal assessment\", shape=hexagon];\n",
            "    \"record-closeout\" [label=\"Record closeout · terminal\", shape=box];\n\n",
            "    \"implement-change\" -> \"choose-route\";\n",
            "    \"choose-route\" -> \"assess-session-goal\" [label=\"passed · advance\"];\n",
            "    \"choose-route\" -> \"implement-change\" [label=\"retry · rework\"];\n",
            "    \"assess-session-goal\" -> \"record-closeout\" [label=\"achieved · advance\"];\n",
            "    \"assess-session-goal\" -> \"implement-change\" [label=\"not-achieved · rework\"];\n",
            "    \"assess-session-goal\" -> \"record-closeout\" [label=\"superseded · advance\"];\n\n",
            "}}"
        ),
        validated.digest().as_str()
    );

    assert_eq!(projection, expected);
    assert_eq!(
        digest,
        "sha256:e747f70dcfa60806a4b271156bbe633e3af3285b0a05760c71b04b30420fd5a0"
    );
    assert!(!projection.ends_with('\n'));
    assert!(!projection.contains("evidence_from"));
}

#[test]
fn v2grf006_projection_digest_hashes_the_exact_dot_bytes() {
    let (_, projection, digest) = render(DOT_YAML, ProcedureDocumentFormat::Yaml);
    assert_eq!(
        digest,
        format!("sha256:{:x}", Sha256::digest(projection.as_bytes()))
    );
}

#[test]
fn v2grf006_titles_cannot_inject_dot_statements_or_attributes() {
    let source = DOT_YAML.replace(
        "title: Choose route",
        "title: 'Choose\"] hacked -> target [URL=<bad> & _-!'",
    );
    let (_, projection, _) = render(&source, ProcedureDocumentFormat::Yaml);

    assert!(projection.contains(concat!(
        "\"choose-route\" [label=\"Choose<U+0022><U+005D> hacked <U+002D><U+003E> target ",
        "<U+005B>URL<U+003D><U+003C>bad<U+003E> <U+0026> <U+005F><U+002D><U+0021> · decision · manual rework target\", ",
        "shape=diamond, style=dashed];"
    )));
    assert!(!projection.contains("hacked -> target [URL=<bad>"));
}

#[test]
fn v2grf006_equivalent_yaml_and_json_render_identically() {
    let yaml = admit(DOT_YAML, ProcedureDocumentFormat::Yaml);
    let yaml_graph = normalize_procedure_v2_graph(&yaml).expect("normalization must succeed");
    let from_yaml = project_procedure_v2_dot(&yaml_graph).expect("DOT must fit");

    let json = admit(
        yaml.canonical_json().as_str(),
        ProcedureDocumentFormat::Json,
    );
    let json_graph = normalize_procedure_v2_graph(&json).expect("normalization must succeed");
    let from_json = project_procedure_v2_dot(&json_graph).expect("DOT must fit");

    assert_eq!(from_yaml, from_json);
}

#[test]
fn v2grf006_single_terminal_omits_the_empty_edge_block() {
    let source = r#"schema: podway.procedure/v2
id: one-node
version: "1"
name: One node
purpose: Verify an edge-free DOT graph.
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
            concat!(
                "digraph podway {{\n",
                "    // podway.procedure/v2\n",
                "    // procedure-digest: {}\n",
                "    rankdir=TB;\n\n",
                "    \"finish-now\" [label=\"Finish · entry · terminal\", shape=box];\n\n",
                "}}"
            ),
            validated.digest().as_str()
        )
    );
    assert!(!projection.contains(" -> "));
}

#[test]
fn v2grf006_parallel_edges_are_preserved_in_author_order() {
    let (_, projection, _) = render(DOT_YAML, ProcedureDocumentFormat::Yaml);
    let first = projection
        .find("\"assess-session-goal\" -> \"record-closeout\" [label=\"achieved")
        .expect("first parallel edge must exist");
    let second = projection
        .find("\"assess-session-goal\" -> \"record-closeout\" [label=\"superseded")
        .expect("second parallel edge must exist");

    assert!(first < second);
    assert_eq!(
        projection
            .matches("\"assess-session-goal\" -> \"record-closeout\"")
            .count(),
        2
    );
}

#[test]
fn v2grf006_dot_budget_does_not_depend_on_json_projection_results() {
    let source = json_over_dot_under_source();
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
    let dot = project_procedure_v2_dot(&graph).expect("DOT has its own fitting bytes");
    assert!(dot.projection().chars().count() <= SOURCE_PROJECTION_MAX_CHARACTERS);
}

#[test]
fn v2grf006_dot_rejects_its_own_complete_projection_over_budget() {
    let source = json_over_dot_under_source().replace(
        "    title: Choose\n",
        &format!("    title: '{}'\n", "<".repeat(120)),
    );
    let validated = admit(&source, ProcedureDocumentFormat::Yaml);
    let graph = normalize_procedure_v2_graph(&validated).expect("normalization has no format cap");

    assert!(matches!(
        project_procedure_v2_dot(&graph),
        Err(ConfigError::OutOfBounds {
            field: "graph projection",
            min: 1,
            max: SOURCE_PROJECTION_MAX_CHARACTERS,
            actual,
        }) if actual > SOURCE_PROJECTION_MAX_CHARACTERS
    ));
}
