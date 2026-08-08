//! V2GRF-003 canonical JSON graph projection.

use podway_config::{
    AuthoringContext, ConfigError, GraphProjectionNodeTypeV2, ParsedProcedure,
    ProcedureDocumentFormat, ValidatedProcedureV2, parse_procedure_document,
    project_procedure_v2_graph, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{SOURCE_PROJECTION_MAX_CHARACTERS, verify_canonical_json_v1};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

const BASE_YAML: &str = r#"schema: podway.procedure/v2
id: graph-projection
version: "1"
name: Projection fixture
purpose: Prove the graph-only projection contract.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Sensitive action title
    intent: Sensitive action intent
    instructions:
      - Sensitive instruction text
    items:
      - id: private-note
        type: text
        prompt: Sensitive item prompt
        required: true
  assess:
    type: decision
    title: Sensitive decision title
    objective: Sensitive decision objective
    prompt: Sensitive decision prompt
    evidence_guidance:
      - Sensitive evidence guidance
    options:
      - id: achieved
        label: Sensitive achieved label
      - id: not-achieved
        label: Sensitive not-achieved label
      - id: superseded
        label: Sensitive superseded label
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
    title: Sensitive finish title
    intent: Sensitive finish intent
graph:
  entry: start
  nodes:
    - id: start
      use: work
      skip:
        allowed: true
        reason_required: true
      next: assess-goal
    - id: assess-goal
      use: assess
      evidence_from:
        - node: start
          required: false
          items:
            - private-note
      routes:
        superseded:
          to: done
          effect: advance
        achieved:
          to: done
          effect: advance
        not-achieved:
          to: start
          effect: rework
    - id: done
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - start
"#;

fn admit(source: &str, format: ProcedureDocumentFormat) -> ValidatedProcedureV2 {
    let parsed =
        match parse_procedure_document(source.as_bytes(), format).expect("fixture must parse") {
            ParsedProcedure::V2(parsed) => parsed,
            ParsedProcedure::V1(_) => panic!("expected Procedure v2"),
        };
    let validated = validate_procedure_v2(parsed).expect("fixture must validate");
    let context = AuthoringContext::new("projection.yaml", source, format);
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "a published projection fixture must be vetted: {diagnostics:#?}"
    );
    validated
}

fn projection(
    source: &str,
    format: ProcedureDocumentFormat,
) -> podway_config::ProcedureGraphProjectionV2 {
    project_procedure_v2_graph(&admit(source, format)).expect("projection must fit")
}

fn oversized_graph_projection_source() -> String {
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
        "schema: podway.procedure/v2\nid: projection-cap\nversion: \"1\"\nname: Projection cap\npurpose: Prove graph projections are independently bounded.\nnode_definitions:\n  choose:\n    type: decision\n    title: Choose\n    objective: Select a route.\n    prompt: Which route?\n    options:\n{options}    reason:\n      required: true\n  finish:\n    type: action\n    title: Finish\n    intent: Finish.\ngraph:\n  entry: {}\n  nodes:\n{nodes}    - id: {terminal_id}\n      use: finish\n      terminal: true\n",
        node_ids[0]
    )
}

#[test]
fn v2grf003_canonical_graph_json_matches_the_golden_shape_and_model() {
    let validated = admit(BASE_YAML, ProcedureDocumentFormat::Yaml);
    let result = project_procedure_v2_graph(&validated).expect("projection must fit");
    let expected = format!(
        concat!(
            "{{\"edges\":[",
            "{{\"effect\":\"advance\",\"from_graph_node_id\":\"start\",\"to_graph_node_id\":\"assess-goal\"}},",
            "{{\"effect\":\"advance\",\"from_graph_node_id\":\"assess-goal\",\"option_id\":\"achieved\",\"to_graph_node_id\":\"done\"}},",
            "{{\"effect\":\"rework\",\"from_graph_node_id\":\"assess-goal\",\"option_id\":\"not-achieved\",\"to_graph_node_id\":\"start\"}},",
            "{{\"effect\":\"advance\",\"from_graph_node_id\":\"assess-goal\",\"option_id\":\"superseded\",\"to_graph_node_id\":\"done\"}}],",
            "\"entry_graph_node_id\":\"start\",",
            "\"nodes\":[",
            "{{\"entry\":true,\"goal_assessment\":false,\"graph_node_id\":\"start\",\"manual_rework_target\":true,\"node_definition_id\":\"work\",\"node_type\":\"action\",\"skippable\":true,\"terminal\":false}},",
            "{{\"entry\":false,\"goal_assessment\":true,\"graph_node_id\":\"assess-goal\",\"manual_rework_target\":false,\"node_definition_id\":\"assess\",\"node_type\":\"decision\",\"skippable\":false,\"terminal\":false}},",
            "{{\"entry\":false,\"goal_assessment\":false,\"graph_node_id\":\"done\",\"manual_rework_target\":false,\"node_definition_id\":\"finish\",\"node_type\":\"action\",\"skippable\":false,\"terminal\":true}}],",
            "\"procedure_digest\":\"{}\",",
            "\"procedure_schema\":\"podway.procedure/v2\",",
            "\"terminal_graph_node_ids\":[\"done\"]}}"
        ),
        validated.digest().as_str()
    );

    assert_eq!(result.projection(), expected);
    assert_eq!(
        result.projection_digest().as_str(),
        "sha256:ba81914ef9399626a7e2db0e0af6f268904d2258ff267430c2094dd0f5048cf9"
    );
    verify_canonical_json_v1(result.projection().as_bytes())
        .expect("projection is Canonical JSON v1");
    assert!(!result.projection().ends_with('\n'));

    let graph = result.graph();
    assert_eq!(graph.procedure_digest(), validated.digest());
    assert_eq!(graph.entry_graph_node_id(), "start");
    assert_eq!(graph.terminal_graph_node_ids(), ["done"]);
    assert_eq!(graph.nodes()[0].title(), "Sensitive action title");
    assert_eq!(
        graph.nodes()[1].node_type(),
        GraphProjectionNodeTypeV2::Decision
    );
    assert!(graph.nodes()[1].goal_assessment());
    assert_eq!(graph.edges()[1].option_id(), Some("achieved"));
}

#[test]
fn v2grf003_node_and_edge_arrays_follow_semantic_author_order() {
    let result = projection(BASE_YAML, ProcedureDocumentFormat::Yaml);
    let node_ids = result
        .graph()
        .nodes()
        .iter()
        .map(|node| node.graph_node_id())
        .collect::<Vec<_>>();
    let edge_options = result
        .graph()
        .edges()
        .iter()
        .map(|edge| edge.option_id())
        .collect::<Vec<_>>();

    assert_eq!(node_ids, ["start", "assess-goal", "done"]);
    assert_eq!(
        edge_options,
        [
            None,
            Some("achieved"),
            Some("not-achieved"),
            Some("superseded")
        ]
    );

    let reordered_routes = BASE_YAML.replace(
        concat!(
            "        superseded:\n          to: done\n          effect: advance\n",
            "        achieved:\n          to: done\n          effect: advance\n",
            "        not-achieved:\n          to: start\n          effect: rework\n"
        ),
        concat!(
            "        not-achieved:\n          to: start\n          effect: rework\n",
            "        achieved:\n          to: done\n          effect: advance\n",
            "        superseded:\n          to: done\n          effect: advance\n"
        ),
    );
    assert_ne!(reordered_routes, BASE_YAML);
    let reordered = projection(&reordered_routes, ProcedureDocumentFormat::Yaml);
    assert_eq!(result.projection(), reordered.projection());
    assert_eq!(result.projection_digest(), reordered.projection_digest());
}

#[test]
fn v2grf003_projection_digest_hashes_the_exact_canonical_bytes() {
    let result = projection(BASE_YAML, ProcedureDocumentFormat::Yaml);
    let expected = format!(
        "sha256:{:x}",
        Sha256::digest(result.projection().as_bytes())
    );

    assert_eq!(result.projection_digest().as_str(), expected);

    let changed = BASE_YAML
        .replace("    - id: done\n", "    - id: completed\n")
        .replace("          to: done\n", "          to: completed\n");
    let changed_result = projection(&changed, ProcedureDocumentFormat::Yaml);
    assert_ne!(
        result.projection_digest(),
        changed_result.projection_digest()
    );
}

#[test]
fn v2grf003_projection_excludes_definition_and_attempt_sensitive_state() {
    let result = projection(BASE_YAML, ProcedureDocumentFormat::Yaml);
    let text = result.projection();

    for forbidden in [
        "Sensitive",
        "evidence_from",
        "private-note",
        "attempt",
        "session",
        "actor",
        "artifact",
        "instructions",
        "purpose",
        "title",
    ] {
        assert!(!text.contains(forbidden), "projection leaked {forbidden:?}");
    }
}

#[test]
fn v2grf003_descriptive_edits_affect_only_the_embedded_procedure_digest() {
    let original = projection(BASE_YAML, ProcedureDocumentFormat::Yaml);
    let edited_source = BASE_YAML
        .replace("Sensitive action title", "Changed private title")
        .replace("Sensitive instruction text", "Changed private instruction")
        .replace("Sensitive item prompt", "Changed private item prompt")
        .replace("Sensitive evidence guidance", "Changed private guidance");
    let edited = projection(&edited_source, ProcedureDocumentFormat::Yaml);

    assert_ne!(original.projection(), edited.projection());
    assert_ne!(original.projection_digest(), edited.projection_digest());
    let mut original_value: Value =
        serde_json::from_str(original.projection()).expect("projection must be JSON");
    let mut edited_value: Value =
        serde_json::from_str(edited.projection()).expect("projection must be JSON");
    let original_digest = original_value
        .as_object_mut()
        .expect("projection must be an object")
        .remove("procedure_digest");
    let edited_digest = edited_value
        .as_object_mut()
        .expect("projection must be an object")
        .remove("procedure_digest");
    assert_ne!(original_digest, edited_digest);
    assert_eq!(original_value, edited_value);
    for forbidden in [
        "Changed private title",
        "Changed private instruction",
        "Changed private item prompt",
        "Changed private guidance",
    ] {
        assert!(!edited.projection().contains(forbidden));
    }
}

#[test]
fn v2grf003_equivalent_yaml_and_json_have_identical_projection_bytes() {
    let yaml = admit(BASE_YAML, ProcedureDocumentFormat::Yaml);
    let json_source = yaml.canonical_json().as_str();
    let from_yaml = project_procedure_v2_graph(&yaml).expect("YAML projection must fit");
    let from_json = projection(json_source, ProcedureDocumentFormat::Json);

    assert_eq!(from_yaml.projection(), from_json.projection());
    assert_eq!(from_yaml.projection_digest(), from_json.projection_digest());
}

#[test]
fn v2grf003_rejects_a_vetted_graph_projection_over_the_character_budget() {
    let source = oversized_graph_projection_source();
    let validated = admit(&source, ProcedureDocumentFormat::Yaml);
    let error = project_procedure_v2_graph(&validated).expect_err("projection must exceed its cap");

    assert!(
        validated.canonical_json().as_str().chars().count() <= SOURCE_PROJECTION_MAX_CHARACTERS
    );
    match error {
        ConfigError::OutOfBounds {
            field,
            min,
            max,
            actual,
        } => {
            assert_eq!(field, "graph projection");
            assert_eq!(min, 1);
            assert_eq!(max, SOURCE_PROJECTION_MAX_CHARACTERS);
            assert!(actual > max);
        }
        other => panic!("unexpected projection error: {other:?}"),
    }
}
