//! V2GRF-008 cross-format projection determinism and safety.

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, ProcedureGraphModelV2,
    ValidatedProcedureV2, parse_procedure_document, project_procedure_v2_dot,
    project_procedure_v2_graph, project_procedure_v2_mermaid, project_procedure_v2_plantuml,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::SOURCE_PROJECTION_MAX_CHARACTERS;
use serde_json::Value;

const CROSS_FORMAT_YAML: &str = r#"schema: podway.procedure/v2
id: cross-format
version: "1"
name: Cross-format conformance
purpose: Prove every graph projection carries the same topology.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Implement safely
    intent: Never emit runtime-attempt-secret.
    instructions:
      - Never emit actor-secret or artifact-location-secret.
    items:
      - id: private-evidence-sentinel
        type: text
        prompt: Never emit evidence-prompt-secret.
        required: true
  choose:
    type: decision
    title: Choose route
    objective: Never emit decision-objective-secret.
    prompt: Never emit decision-prompt-secret.
    options:
      - id: passed
        label: Continue
      - id: retry
        label: Retry
    reason:
      required: true
  assess:
    type: decision
    title: Assess goal
    objective: Assess the session goal.
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
  close:
    type: decision
    title: Select terminal
    objective: Select the terminal placement.
    prompt: Which terminal?
    options:
      - id: complete
        label: Complete
      - id: archive
        label: Archive
    reason:
      required: true
  finish:
    type: action
    title: Finish
    intent: Finish the procedure.
graph:
  entry: start-work
  nodes:
    - id: start-work
      use: work
      skip:
        allowed: true
        reason_required: true
      next: choose-route
    - id: choose-route
      use: choose
      routes:
        retry:
          to: start-work
          effect: rework
        passed:
          to: assess-goal
          effect: advance
    - id: assess-goal
      use: assess
      evidence_from:
        - node: start-work
          required: false
          items:
            - private-evidence-sentinel
      routes:
        superseded:
          to: close-route
          effect: advance
        not-achieved:
          to: start-work
          effect: rework
        achieved:
          to: close-route
          effect: advance
    - id: close-route
      use: close
      routes:
        archive:
          to: archived
          effect: advance
        complete:
          to: done
          effect: advance
    - id: done
      use: finish
      terminal: true
    - id: archived
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - start-work
    - choose-route
"#;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Transition {
    from: String,
    to: String,
    option: Option<String>,
    effect: String,
}

#[derive(Debug, Eq, PartialEq)]
struct ExtractedGraph {
    nodes: Vec<String>,
    transitions: Vec<Transition>,
    manual_rework_targets: Vec<String>,
}

struct Projections {
    graph: ProcedureGraphModelV2,
    json: String,
    json_digest: String,
    mermaid: String,
    mermaid_digest: String,
    plantuml: String,
    plantuml_digest: String,
    dot: String,
    dot_digest: String,
}

fn admit(source: &str, format: ProcedureDocumentFormat) -> ValidatedProcedureV2 {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source.as_bytes(), format).expect("fixture must parse");
    let validated = validate_procedure_v2(parsed).expect("fixture must validate");
    let context = AuthoringContext::new("cross-format.yaml", source, format);
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "a projection fixture must be vetted: {diagnostics:#?}"
    );
    validated
}

fn render(source: &str, format: ProcedureDocumentFormat) -> Projections {
    let validated = admit(source, format);
    let graph_projection =
        project_procedure_v2_graph(&validated).expect("graph JSON projection must fit");
    let graph = graph_projection.graph().clone();
    let mermaid = project_procedure_v2_mermaid(&graph).expect("Mermaid projection must fit");
    let plantuml = project_procedure_v2_plantuml(&graph).expect("PlantUML projection must fit");
    let dot = project_procedure_v2_dot(&graph).expect("DOT projection must fit");

    Projections {
        graph,
        json: graph_projection.projection().to_owned(),
        json_digest: graph_projection.projection_digest().as_str().to_owned(),
        mermaid: mermaid.projection().to_owned(),
        mermaid_digest: mermaid.projection_digest().as_str().to_owned(),
        plantuml: plantuml.projection().to_owned(),
        plantuml_digest: plantuml.projection_digest().as_str().to_owned(),
        dot: dot.projection().to_owned(),
        dot_digest: dot.projection_digest().as_str().to_owned(),
    }
}

fn expected_graph(graph: &ProcedureGraphModelV2) -> ExtractedGraph {
    ExtractedGraph {
        nodes: graph
            .nodes()
            .iter()
            .map(|node| node.graph_node_id().to_owned())
            .collect(),
        transitions: graph
            .edges()
            .iter()
            .map(|edge| Transition {
                from: edge.from_graph_node_id().to_owned(),
                to: edge.to_graph_node_id().to_owned(),
                option: edge.option_id().map(str::to_owned),
                effect: edge.effect().to_owned(),
            })
            .collect(),
        manual_rework_targets: graph
            .nodes()
            .iter()
            .filter(|node| node.manual_rework_target())
            .map(|node| node.graph_node_id().to_owned())
            .collect(),
    }
}

fn cross_format_oracle() -> ExtractedGraph {
    ExtractedGraph {
        nodes: [
            "start-work",
            "choose-route",
            "assess-goal",
            "close-route",
            "done",
            "archived",
        ]
        .map(str::to_owned)
        .to_vec(),
        transitions: [
            ("start-work", "choose-route", None, "advance"),
            ("choose-route", "assess-goal", Some("passed"), "advance"),
            ("choose-route", "start-work", Some("retry"), "rework"),
            ("assess-goal", "close-route", Some("achieved"), "advance"),
            ("assess-goal", "start-work", Some("not-achieved"), "rework"),
            ("assess-goal", "close-route", Some("superseded"), "advance"),
            ("close-route", "done", Some("complete"), "advance"),
            ("close-route", "archived", Some("archive"), "advance"),
        ]
        .map(|(from, to, option, effect)| Transition {
            from: from.to_owned(),
            to: to.to_owned(),
            option: option.map(str::to_owned),
            effect: effect.to_owned(),
        })
        .to_vec(),
        manual_rework_targets: ["start-work", "choose-route"].map(str::to_owned).to_vec(),
    }
}

fn extract_json(projection: &str) -> ExtractedGraph {
    let value: Value = serde_json::from_str(projection).expect("graph projection must be JSON");
    let nodes = value["nodes"]
        .as_array()
        .expect("nodes must be an array")
        .iter()
        .map(|node| node["graph_node_id"].as_str().expect("node ID").to_owned())
        .collect();
    let transitions = value["edges"]
        .as_array()
        .expect("edges must be an array")
        .iter()
        .map(|edge| Transition {
            from: edge["from_graph_node_id"]
                .as_str()
                .expect("edge source")
                .to_owned(),
            to: edge["to_graph_node_id"]
                .as_str()
                .expect("edge target")
                .to_owned(),
            option: edge.get("option_id").map(|option| {
                option
                    .as_str()
                    .expect("option ID must be a string")
                    .to_owned()
            }),
            effect: edge["effect"].as_str().expect("edge effect").to_owned(),
        })
        .collect();
    let manual_rework_targets = value["nodes"]
        .as_array()
        .expect("nodes must be an array")
        .iter()
        .filter(|node| node["manual_rework_target"] == Value::Bool(true))
        .map(|node| node["graph_node_id"].as_str().expect("node ID").to_owned())
        .collect();

    ExtractedGraph {
        nodes,
        transitions,
        manual_rework_targets,
    }
}

fn graph_id_from_alias(alias: &str, prefix: &str) -> String {
    alias
        .strip_prefix(prefix)
        .unwrap_or(alias)
        .replace('_', "-")
}

fn transition(from: &str, to_and_label: &str, alias_prefix: &str) -> Transition {
    let (to, label) = to_and_label
        .split_once(" : ")
        .map_or((to_and_label, None), |(to, label)| (to, Some(label)));
    let (option, effect) = label.map_or((None, "advance".to_owned()), |label| {
        let (option, effect) = label.split_once(" · ").expect("decision edge label");
        (Some(option.to_owned()), effect.to_owned())
    });
    Transition {
        from: graph_id_from_alias(from, alias_prefix),
        to: graph_id_from_alias(to, alias_prefix),
        option,
        effect,
    }
}

fn extract_mermaid(projection: &str) -> ExtractedGraph {
    let mut nodes = Vec::new();
    let mut transitions = Vec::new();
    let mut manual_rework_targets = Vec::new();
    for line in projection.lines().map(str::trim) {
        if let Some((from, remainder)) = line.split_once(" -->") {
            let (label, to) = if let Some(label_and_to) = remainder.strip_prefix('|') {
                let (label, to) = label_and_to.split_once("| ").expect("Mermaid edge label");
                (Some(label), to)
            } else {
                (None, remainder.trim())
            };
            let (option, effect) = label.map_or((None, "advance".to_owned()), |label| {
                let (option, effect) = label.split_once(" · ").expect("decision edge label");
                (Some(option.to_owned()), effect.to_owned())
            });
            transitions.push(Transition {
                from: graph_id_from_alias(from, "n__"),
                to: graph_id_from_alias(to, "n__"),
                option,
                effect,
            });
        } else if let Some(targets) = line
            .strip_prefix("class ")
            .and_then(|line| line.strip_suffix(" manual_rework_target"))
        {
            manual_rework_targets = targets
                .split(',')
                .map(|target| graph_id_from_alias(target, "n__"))
                .collect();
        } else if !line.is_empty()
            && !line.starts_with('%')
            && !line.starts_with("flowchart ")
            && !line.starts_with("classDef ")
        {
            let end = line.find(['[', '{']).expect("Mermaid node declaration");
            nodes.push(graph_id_from_alias(&line[..end], "n__"));
        }
    }
    ExtractedGraph {
        nodes,
        transitions,
        manual_rework_targets,
    }
}

fn extract_plantuml(projection: &str) -> ExtractedGraph {
    let mut nodes = Vec::new();
    let mut transitions = Vec::new();
    let mut manual_rework_targets = Vec::new();
    for line in projection.lines() {
        if let Some(declaration) = line.strip_prefix("state \"") {
            let (label, alias) = declaration
                .rsplit_once("\" as ")
                .expect("state declaration");
            let alias = alias.split_whitespace().next().expect("state alias");
            let graph_id = graph_id_from_alias(alias, "n_");
            if label.ends_with(" · manual rework target") {
                manual_rework_targets.push(graph_id.clone());
            }
            nodes.push(graph_id);
        } else if let Some((from, to_and_label)) = line.split_once(" --> ") {
            transitions.push(transition(from, to_and_label, "n_"));
        }
    }
    ExtractedGraph {
        nodes,
        transitions,
        manual_rework_targets,
    }
}

fn extract_dot(projection: &str) -> ExtractedGraph {
    let mut nodes = Vec::new();
    let mut transitions = Vec::new();
    let mut manual_rework_targets = Vec::new();
    for line in projection.lines().map(str::trim) {
        if let Some(edge) = line.strip_prefix('"') {
            if let Some((from, remainder)) = edge.split_once("\" -> \"") {
                let (to, attributes) = remainder
                    .split_once('"')
                    .expect("DOT edge target must be quoted");
                let label = attributes
                    .strip_prefix(" [label=\"")
                    .and_then(|attributes| attributes.strip_suffix("\"];"));
                let (option, effect) = label.map_or((None, "advance".to_owned()), |label| {
                    let (option, effect) = label.split_once(" · ").expect("DOT edge label");
                    (Some(option.to_owned()), effect.to_owned())
                });
                transitions.push(Transition {
                    from: from.to_owned(),
                    to: to.to_owned(),
                    option,
                    effect,
                });
            } else if let Some((node, attributes)) = edge.split_once("\" [label=\"") {
                nodes.push(node.to_owned());
                if attributes.ends_with(", style=dashed];") {
                    manual_rework_targets.push(node.to_owned());
                }
            }
        }
    }
    ExtractedGraph {
        nodes,
        transitions,
        manual_rework_targets,
    }
}

#[test]
fn v2grf008_all_formats_preserve_ordered_identities_transitions_and_rework_metadata() {
    let projections = render(CROSS_FORMAT_YAML, ProcedureDocumentFormat::Yaml);
    let expected = cross_format_oracle();

    assert_eq!(expected_graph(&projections.graph), expected);
    assert_eq!(extract_json(&projections.json), expected);
    assert_eq!(extract_mermaid(&projections.mermaid), expected);
    assert_eq!(extract_plantuml(&projections.plantuml), expected);
    assert_eq!(extract_dot(&projections.dot), expected);
    assert_eq!(
        expected.manual_rework_targets,
        ["start-work", "choose-route"]
    );
    assert_eq!(expected.transitions.len(), 8, "manual rework adds no edge");
    assert!(
        !expected
            .transitions
            .iter()
            .any(|edge| edge.from == "start-work" && edge.to == "assess-goal"),
        "evidence_from must not become a transition"
    );
}

#[test]
fn v2grf008_equivalent_source_forms_produce_identical_bytes_and_digests() {
    let yaml = render(CROSS_FORMAT_YAML, ProcedureDocumentFormat::Yaml);
    let canonical_json = admit(CROSS_FORMAT_YAML, ProcedureDocumentFormat::Yaml)
        .canonical_json()
        .as_str()
        .to_owned();
    let json = render(&canonical_json, ProcedureDocumentFormat::Json);
    let reordered_commented = CROSS_FORMAT_YAML
        .replacen(
            concat!(
                "schema: podway.procedure/v2\n",
                "id: cross-format\n",
                "version: \"1\"\n",
                "name: Cross-format conformance\n",
                "purpose: Prove every graph projection carries the same topology.\n"
            ),
            concat!(
                "# Equivalent root key order and comments are nonsemantic.\n",
                "purpose: Prove every graph projection carries the same topology.\n",
                "name: Cross-format conformance\n",
                "version: \"1\"\n",
                "id: cross-format\n",
                "schema: podway.procedure/v2\n"
            ),
            1,
        )
        .replacen(
            concat!(
                "        retry:\n          to: start-work\n          effect: rework\n",
                "        passed:\n          to: assess-goal\n          effect: advance\n"
            ),
            concat!(
                "        passed:\n          effect: advance\n          to: assess-goal\n",
                "        # Route map order follows option authority.\n",
                "        retry:\n          effect: rework\n          to: start-work\n"
            ),
            1,
        );
    let alternate = render(&reordered_commented, ProcedureDocumentFormat::Yaml);

    for candidate in [&json, &alternate] {
        assert_eq!(candidate.json, yaml.json);
        assert_eq!(candidate.json_digest, yaml.json_digest);
        assert_eq!(candidate.mermaid, yaml.mermaid);
        assert_eq!(candidate.mermaid_digest, yaml.mermaid_digest);
        assert_eq!(candidate.plantuml, yaml.plantuml);
        assert_eq!(candidate.plantuml_digest, yaml.plantuml_digest);
        assert_eq!(candidate.dot, yaml.dot);
        assert_eq!(candidate.dot_digest, yaml.dot_digest);
    }
}

#[test]
fn v2grf008_all_formats_exclude_evidence_runtime_and_sensitive_sentinels() {
    let projections = render(CROSS_FORMAT_YAML, ProcedureDocumentFormat::Yaml);
    for (format, projection) in [
        ("json", projections.json.as_str()),
        ("mermaid", projections.mermaid.as_str()),
        ("plantuml", projections.plantuml.as_str()),
        ("dot", projections.dot.as_str()),
    ] {
        for forbidden in [
            "runtime-attempt-secret",
            "actor-secret",
            "artifact-location-secret",
            "private-evidence-sentinel",
            "evidence-prompt-secret",
            "decision-objective-secret",
            "decision-prompt-secret",
            "evidence_from",
        ] {
            assert!(
                !projection.contains(forbidden),
                "{format} projection leaked {forbidden:?}"
            );
        }
        assert!(projection.chars().count() <= SOURCE_PROJECTION_MAX_CHARACTERS);
    }
}
