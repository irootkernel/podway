//! V2GRF-005 deterministic PlantUML state-diagram projection.

use podway_config::{
    AuthoringContext, ConfigError, ParsedProcedure, ProcedureDocumentFormat, ValidatedProcedureV2,
    normalize_procedure_v2_graph, parse_procedure_document, project_procedure_v2_graph,
    project_procedure_v2_mermaid, project_procedure_v2_plantuml, validate_procedure_v2,
    vet_procedure_v2,
};
use podway_core::SOURCE_PROJECTION_MAX_CHARACTERS;
use sha2::{Digest as _, Sha256};

const PLANTUML_YAML: &str = r#"schema: podway.procedure/v2
id: plantuml-review
version: "1"
name: PlantUML review
purpose: Exercise every PlantUML review convention.
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
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source.as_bytes(), format).expect("fixture must parse");
    let validated = validate_procedure_v2(parsed).expect("fixture must validate");
    let context = AuthoringContext::new("plantuml.yaml", source, format);
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
    let plantuml = project_procedure_v2_plantuml(&graph).expect("PlantUML projection must fit");
    (
        validated,
        plantuml.projection().to_owned(),
        plantuml.projection_digest().as_str().to_owned(),
    )
}

fn json_over_text_under_source() -> String {
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
fn v2grf005_plantuml_matches_the_complete_state_diagram_golden() {
    let (validated, projection, digest) = render(PLANTUML_YAML, ProcedureDocumentFormat::Yaml);
    let expected = format!(
        concat!(
            "@startuml\n",
            "' podway.procedure/v2\n",
            "' procedure-digest: {}\n",
            "hide empty description\n",
            "top to bottom direction\n\n",
            "state \"Implement <U+0022>safe<U+0022> <U+003C>change<U+003E> <U+0026> verify · entry · skippable · manual rework target\" as n_implement_change\n",
            "state \"Choose route · decision · manual rework target\" as n_choose_route <<decision>>\n",
            "state \"Assess the session goal · goal assessment\" as n_assess_session_goal <<goal_assessment>>\n",
            "state \"Record closeout · terminal\" as n_record_closeout\n\n",
            "n_implement_change --> n_choose_route\n",
            "n_choose_route --> n_assess_session_goal : passed · advance\n",
            "n_choose_route --> n_implement_change : retry · rework\n",
            "n_assess_session_goal --> n_record_closeout : achieved · advance\n",
            "n_assess_session_goal --> n_implement_change : not-achieved · rework\n",
            "n_assess_session_goal --> n_record_closeout : superseded · advance\n\n",
            "@enduml"
        ),
        validated.digest().as_str()
    );

    assert_eq!(projection, expected);
    assert_eq!(
        digest,
        "sha256:c0987dd45290fbbaa01d7500b3006e077ee93a88bb823f25cb2b446bb462b519"
    );
    assert!(!projection.ends_with('\n'));
    assert!(!projection.contains("evidence_from"));
    assert!(!projection.contains("[*]"));
}

#[test]
fn v2grf005_projection_digest_hashes_the_exact_plantuml_bytes() {
    let (_, projection, digest) = render(PLANTUML_YAML, ProcedureDocumentFormat::Yaml);
    assert_eq!(
        digest,
        format!("sha256:{:x}", Sha256::digest(projection.as_bytes()))
    );
}

#[test]
fn v2grf005_titles_cannot_inject_plantuml_statements_or_aliases() {
    let source = PLANTUML_YAML.replace(
        "title: Choose route",
        "title: 'Choose\\n@enduml\\nstate hacked as injected <b> & _-!'",
    );
    let (_, projection, _) = render(&source, ProcedureDocumentFormat::Yaml);

    assert!(projection.contains(concat!(
        "state \"Choose<U+005C>n<U+0040>enduml<U+005C>nstate hacked as injected ",
        "<U+003C>b<U+003E> <U+0026> <U+005F><U+002D><U+0021> · decision · manual rework target\" ",
        "as n_choose_route <<decision>>"
    )));
    assert_eq!(projection.matches("@enduml").count(), 1);
    assert!(!projection.contains("state hacked as injected\n"));
}

#[test]
fn v2grf005_equivalent_yaml_and_json_render_identically() {
    let yaml = admit(PLANTUML_YAML, ProcedureDocumentFormat::Yaml);
    let yaml_graph = normalize_procedure_v2_graph(&yaml).expect("normalization must succeed");
    let from_yaml = project_procedure_v2_plantuml(&yaml_graph).expect("PlantUML must fit");

    let json = admit(
        yaml.canonical_json().as_str(),
        ProcedureDocumentFormat::Json,
    );
    let json_graph = normalize_procedure_v2_graph(&json).expect("normalization must succeed");
    let from_json = project_procedure_v2_plantuml(&json_graph).expect("PlantUML must fit");

    assert_eq!(from_yaml, from_json);
}

#[test]
fn v2grf005_single_terminal_omits_edges_but_keeps_state_boundaries() {
    let source = r#"schema: podway.procedure/v2
id: one-node
version: "1"
name: One node
purpose: Verify an edge-free PlantUML graph.
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
                "@startuml\n",
                "' podway.procedure/v2\n",
                "' procedure-digest: {}\n",
                "hide empty description\n",
                "top to bottom direction\n\n",
                "state \"Finish · entry · terminal\" as n_finish_now\n\n",
                "@enduml"
            ),
            validated.digest().as_str()
        )
    );
    assert!(!projection.contains(" --> "));
}

#[test]
fn v2grf005_plantuml_budget_does_not_depend_on_json_or_mermaid_projection_results() {
    let source = json_over_text_under_source();
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
    let mermaid = project_procedure_v2_mermaid(&graph).expect("Mermaid has its own fitting bytes");
    let plantuml =
        project_procedure_v2_plantuml(&graph).expect("PlantUML has its own fitting bytes");
    assert!(mermaid.projection().chars().count() <= SOURCE_PROJECTION_MAX_CHARACTERS);
    assert!(plantuml.projection().chars().count() <= SOURCE_PROJECTION_MAX_CHARACTERS);
}

#[test]
fn v2grf005_plantuml_rejects_its_own_complete_projection_over_budget() {
    let source = json_over_text_under_source().replace(
        "    title: Choose\n",
        &format!("    title: '{}'\n", "<".repeat(120)),
    );
    let validated = admit(&source, ProcedureDocumentFormat::Yaml);
    let graph = normalize_procedure_v2_graph(&validated).expect("normalization has no format cap");

    assert!(matches!(
        project_procedure_v2_plantuml(&graph),
        Err(ConfigError::OutOfBounds {
            field: "graph projection",
            min: 1,
            max: SOURCE_PROJECTION_MAX_CHARACTERS,
            actual,
        }) if actual > SOURCE_PROJECTION_MAX_CHARACTERS
    ));
}
