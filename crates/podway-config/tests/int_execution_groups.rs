use podway_config::execution_groups::*;
use podway_config::{
    AuthoringContext, ConfigError, FormatRequest, ProcedureDocumentFormat as Format,
};
use serde_json::{Value, json};

#[test]
fn successor_graph_renderings_preserve_the_replan_boundary() {
    let base = accept(&base());
    let group = accept(&group());
    for format in [
        SuccessorGraphFormat::Mermaid,
        SuccessorGraphFormat::Dot,
        SuccessorGraphFormat::PlantUml,
    ] {
        let (base_projection, base_digest) = render_successor_graph(&base, format).unwrap();
        let (group_projection, group_digest) = render_successor_graph(&group, format).unwrap();
        assert!(base_projection.contains("n_design"));
        assert!(base_projection.contains("replan"));
        assert!(group_projection.contains("Return to base design anchor"));
        assert!(group_projection.contains("repair · rework"));
        assert_ne!(base_digest, group_digest);
        assert_eq!(
            render_successor_graph(&group, format).unwrap(),
            (group_projection, group_digest),
        );
    }
}

#[test]
fn successor_authoring_reports_canonical_drift_at_the_source() {
    let document = accept(&group());
    let canonical = project_successor_source(&document, Format::Yaml).unwrap();
    let drifted = canonical.replacen("schema:", "schema: ", 1);
    let context = AuthoringContext::new("group.yaml", &drifted, Format::Yaml);
    let formatted = format_successor_definition(FormatRequest {
        source: &drifted,
        source_path: "group.yaml",
        format: Format::Yaml,
    })
    .unwrap();
    assert!(formatted.changed());
    assert_eq!(formatted.document(), canonical);
    assert_eq!(formatted.digest(), document.digest());
    assert_eq!(
        formatted.drift_diagnostic(&context).unwrap().code(),
        podway_core::AuthoringDiagnosticCode::FormatNotCanonical,
    );
}

fn reject_controller(value: &Value) {
    match admit(value) {
        Err(ConfigError::V2ShapeMismatch { field, reason }) => {
            assert_eq!(field, "node_definitions.items.required_when.item");
            assert!(reason.contains("earlier unconditionally required"));
        }
        Err(other) => panic!("wrong controller error: {other}"),
        Ok(_) => panic!("selected controller contract was admitted"),
    }
}
#[test]
fn frame002_settlement_preserves_first_arrival_and_same_region_controls() {
    accept(&base());
    let mut value = crossing_base(false);
    value["graph"]["nodes"][3]["routes"]["escape"] = json!({"effect":"rework","to":"ret"});
    accept(&value);
    let mut value = crossing_base(true);
    value["graph"]["nodes"][5]["next"] = json!("finish");
    accept(&value);
    let value = conditional_port();
    let b = accept(&value);
    let mut g = group();
    g["plan_inputs"] = value["node_definitions"]["design"]["items"].clone();
    g["graph"]["nodes"][0]["evidence_from"][0]["items"] = json!(["mode", "details"]);
    g["plan_inputs"][1]["prompt"] = json!("Presentation only");
    validate_group_interface(&b, &accept(&g)).unwrap();
}
#[test]
fn frame002_settlement_finite_exit_and_route_targets_are_enforced() {
    let mut value = chain(1);
    value["node_definitions"] = json!({"start":decision(vec![json!({"id":"finish","label":"Finish"}),json!({"id":"trap","label":"Trap"})]),"work":action(vec![]),"again":decision(vec![json!({"id":"again","label":"Again"})])});
    value["graph"] = json!({"entry":"start","nodes":[{"id":"start","use":"start","routes":{"finish":{"effect":"advance","to":"finish"},"trap":{"effect":"advance","to":"trap"}}},{"id":"trap","use":"work","next":"stuck"},{"id":"stuck","use":"again","routes":{"again":{"effect":"rework","to":"trap"}}},{"id":"finish","use":"work","terminal":true}]});
    reject_field(&value, "graph", "finite terminal or replan exit");
    value["graph"]["nodes"][2]["routes"]["again"] = json!({"effect":"advance","to":"finish"});
    accept(&value);
    for (option, with_target) in [("replan", true), ("success", false), ("repair", false)] {
        let mut value = group();
        if with_target {
            value["graph"]["nodes"][2]["routes"][option]["to"] = json!("work");
        } else {
            remove(&mut value, &format!("/graph/nodes/2/routes/{option}"), "to");
        }
        reject_field(&value, "graph.nodes.routes", "effect and target");
    }
}
#[test]
fn frame002_settlement_closed_categories_have_typed_rejections() {
    let mut value = base();
    value.as_object_mut().unwrap().remove("execution_group");
    reject_field(&value, "execution_group", "exactly one boundary");
    let mut value = group();
    value.as_object_mut().unwrap().remove("plan_inputs");
    reject_field(&value, "plan_inputs", "plan interface");
    let mut value = base();
    value["plan_inputs"] = json!([]);
    reject_field(&value, "plan_inputs", "only to group");
    let mut value = base();
    value["graph"]["nodes"][0]["evidence_from"] = json!([{"scope":"plan","items":["plan"]}]);
    reject_field(&value, "evidence.scope", "only to a group");
    let mut value = crossing_base(false);
    value["graph"]["nodes"][3]["routes"]["escape"] = json!({"effect":"rework","to":"ret"});
    value["execution_group"]["replan_decisions"] = json!(["gate"]);
    reject_field(
        &value,
        "execution_group.replan_decisions",
        "follow the group return",
    );
    let mut value = group();
    remove(&mut value, "/graph/nodes/2/routes", "repair");
    reject_field(&value, "graph.nodes.routes", "exactly cover");
    let mut value = group();
    value["graph"]["nodes"][2]["routes"]["extra"] = json!({"effect":"advance","to":"finish"});
    reject_field(&value, "graph.nodes.routes", "exactly cover");
    let mut value = group();
    value["schema"] = json!("podway.execution-group/v999");
    assert!(
        matches!(admit(&value),Err(ConfigError::InvalidSchema{actual,..}) if actual=="podway.execution-group/v999")
    );
    let mut value = group();
    value["graph"]["nodes"][2]["next"] = json!("finish");
    reject_field(&value, "graph.nodes", "only routes");
    let mut value = group();
    remove(
        &mut value,
        "/node_definitions/choose/options/2/guards/0/evidence",
        "item",
    );
    reject_field(&value, "guards.evidence.item", "select one item");
    let b = accept(&base());
    let g = accept(&group());
    for (left, right) in [(&b, &b), (&g, &g), (&g, &b)] {
        assert!(matches!(
            validate_group_interface(left, right),
            Err(ConfigError::InvalidValue {
                field: "execution_group",
                ..
            })
        ));
    }
}
const AUTHORED_BASE: &str = r#"schema: podway.procedure/v3
id: authored-base
version: "1"
name: Authored base
purpose: Keep authored order
node_definitions:
  z-work:
    type: action
    title: Design
    intent: Design the work
  a-review:
    type: decision
    title: Review
    objective: Review current work
    prompt: Choose
    reason:
      required: true
    options:
      - id: a
        label: First option
      - id: z
        label: Second option
  m-finish:
    type: action
    title: Finish
    intent: Finish the work
graph:
  entry: design
  nodes:
    - id: design
      use: z-work
      next: review
    - id: review
      use: a-review
      routes:
        z:
          effect: advance
          to: finish
        a:
          effect: advance
          to: finish
    - id: finish
      use: m-finish
      terminal: true
execution_group:
  anchor: design
  success_return: review
  plan_inputs: []
  result_items: []
  replan_decisions: []
"#;
const AUTHORED_GROUP: &str = r#"schema: podway.execution-group/v1
id: authored-group
version: "1"
name: Authored group
purpose: Preserve a real YAML source
node_definitions:
  z-work:
    type: action
    title: Work
    intent: Perform the work
  a-finish:
    type: action
    title: Finish
    intent: Return to base
graph:
  entry: work
  nodes:
    - id: work
      use: z-work
      # Continue the declared local path.
      next: finish
    - id: finish
      use: a-finish
      terminal: true
plan_inputs: []
"#;
fn yaml_admit(source: &str) -> ValidatedSuccessorDefinition {
    validate_successor_definition(
        parse_successor_document(source.as_bytes(), Format::Yaml).unwrap(),
    )
    .unwrap()
}
#[test]
fn frame002_settlement_authored_maps_options_and_yaml_comments_are_discriminating() {
    let b = yaml_admit(AUTHORED_BASE);
    let g = yaml_admit(AUTHORED_GROUP);
    validate_group_interface(&b, &g).unwrap();
    let rendered = project_successor_source(&b, Format::Json).unwrap();
    assert!(rendered.find("\"z-work\":").unwrap() < rendered.find("\"a-review\":").unwrap());
    assert!(rendered.find("\"a-review\":").unwrap() < rendered.find("\"m-finish\":").unwrap());
    assert!(rendered.find("\"z\": {").unwrap() < rendered.find("\"a\": {").unwrap());
    let projection: Value =
        serde_json::from_str(project_successor_graph(&b).unwrap().projection()).unwrap();
    let options = projection["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["from"] == "review")
        .map(|v| v["option"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(options, ["a", "z"]);
    let normalized = accept(&serde_json::from_str(b.canonical_json().as_str()).unwrap());
    assert_eq!(normalized.digest(), b.digest());
    let normalized_source = project_successor_source(&normalized, Format::Json).unwrap();
    assert!(
        normalized_source.find("\"a-review\":").unwrap()
            < normalized_source.find("\"z-work\":").unwrap()
    );
    for source in [AUTHORED_BASE, AUTHORED_GROUP] {
        let original = yaml_admit(source);
        let formatted = format_successor_definition(FormatRequest {
            source,
            source_path: "authored.yaml",
            format: Format::Yaml,
        })
        .unwrap();
        assert_eq!(yaml_admit(formatted.document()).digest(), original.digest());
        let again = format_successor_definition(FormatRequest {
            source: formatted.document(),
            source_path: "authored.yaml",
            format: Format::Yaml,
        })
        .unwrap();
        assert!(!again.changed());
        if source == AUTHORED_GROUP {
            assert!(
                formatted
                    .document()
                    .contains("# Continue the declared local path.")
            );
        }
    }
}
#[test]
fn frame002_settlement_successor_decoder_seams_fail_before_typed_mapping() {
    yaml_admit(AUTHORED_GROUP);
    let mut value = group();
    value["description"] = Value::Null;
    assert!(
        matches!(admit(&value),Err(ConfigError::InvalidDocument{reason}) if reason.contains("null"))
    );
    let anchored = AUTHORED_GROUP.replace("id: authored-group", "id: &group authored-group");
    assert!(matches!(
        parse_successor_document(anchored.as_bytes(), Format::Yaml),
        Err(ConfigError::UnsupportedYamlFeature { .. })
    ));
    let multiple = format!("{AUTHORED_GROUP}---\n{AUTHORED_GROUP}");
    assert!(
        matches!(parse_successor_document(multiple.as_bytes(),Format::Yaml),Err(ConfigError::InvalidDocument{reason}) if reason.contains("exactly one YAML document"))
    );
    let duplicate = AUTHORED_GROUP.replace("version: \"1\"", "version: \"1\"\nversion: \"1\"");
    assert!(
        matches!(parse_successor_document(duplicate.as_bytes(),Format::Yaml),Err(ConfigError::DuplicateKey{key}) if key=="version")
    );
    let mut nested = json!(0);
    for _ in 0..=podway_config::MAX_PROCEDURE_DOCUMENT_DEPTH {
        nested = json!([nested]);
    }
    let mut value = group();
    value["unexpected"] = nested;
    assert!(
        matches!(admit(&value),Err(ConfigError::InputTooDeep{maximum,..}) if maximum==podway_config::MAX_PROCEDURE_DOCUMENT_DEPTH)
    );
    let mut value = group();
    value["unexpected"] = json!(vec![0; podway_config::MAX_PROCEDURE_DOCUMENT_NODES + 1]);
    let source = serde_json::to_vec(&value).unwrap();
    assert!(source.len() < podway_config::MAX_PROCEDURE_DOCUMENT_BYTES);
    assert!(
        matches!(parse_successor_document(&source,Format::Json),Err(ConfigError::InputTooComplex{maximum,..}) if maximum==podway_config::MAX_PROCEDURE_DOCUMENT_NODES)
    );
}
fn dense_group(long_ids: bool) -> Value {
    let count = podway_core::execution_groups::MAX_GROUP_PLACEMENTS_V1;
    let id = |n: usize| {
        if long_ids {
            format!("n{n:04}{}", "x".repeat(59))
        } else {
            format!("n{n}")
        }
    };
    let options = (0..8)
        .map(|n| json!({"id":format!("r{n}"),"label":"Route"}))
        .collect();
    let mut value = chain(1);
    value["node_definitions"] = json!({"choose":decision(options),"finish":action(vec![])});
    value["graph"]["entry"] = json!(id(0));
    value["graph"]["nodes"] = json!(
        (0..count)
            .map(|n| {
                if n + 1 == count {
                    return json!({"id":id(n),"use":"finish","terminal":true});
                }
                let routes = (0..8)
                    .map(|r| {
                        (
                            format!("r{r}"),
                            if r == 0 {
                                json!({"effect":"advance","to":id(n+1)})
                            } else {
                                json!({"effect":"rework","to":id(0)})
                            },
                        )
                    })
                    .collect::<serde_json::Map<_, _>>();
                json!({"id":id(n),"use":"choose","routes":routes})
            })
            .collect::<Vec<_>>()
    );
    value
}
#[test]
fn frame002_settlement_attainable_edge_composition_is_admitted_and_projected() {
    let value = accept(&dense_group(false));
    let projected: Value =
        serde_json::from_str(project_successor_graph(&value).unwrap().projection()).unwrap();
    assert_eq!(projected["nodes"].as_array().unwrap().len(), 1024);
    let edge_count = projected["edges"].as_array().unwrap().len();
    assert_eq!(edge_count, 1023 * 8);
    assert_eq!(edge_count, 8184);
    assert!(edge_count < podway_core::execution_groups::MAX_GROUP_EDGES_V1);
}
fn overflow<T>(result: Result<T, ConfigError>, expected: &str) -> usize {
    match result {
        Err(ConfigError::OutOfBounds {
            field, max, actual, ..
        }) => {
            assert_eq!(field, expected);
            assert_eq!(max, podway_core::SOURCE_PROJECTION_MAX_CHARACTERS);
            assert!(actual > max);
            actual
        }
        Err(other) => panic!("wrong overflow: {other}"),
        Ok(_) => panic!("expected public API overflow: {expected}"),
    }
}
#[test]
fn frame002_settlement_public_source_and_projection_caps_are_wired() {
    let mut padded = serde_json::to_vec(&chain(1)).unwrap();
    padded.resize(podway_config::MAX_PROCEDURE_DOCUMENT_BYTES, b' ');
    validate_successor_definition(parse_successor_document(&padded, Format::Json).unwrap())
        .unwrap();
    padded.push(b' ');
    assert!(
        matches!(parse_successor_document(&padded,Format::Json),Err(ConfigError::InputTooLarge{maximum,actual}) if maximum==podway_config::MAX_PROCEDURE_DOCUMENT_BYTES&&actual==maximum+1)
    );
    let source = serde_json::to_string(&dense_group(true)).unwrap();
    assert!(source.len() < podway_config::MAX_PROCEDURE_DOCUMENT_BYTES);
    let value = validate_successor_definition(
        parse_successor_document(source.as_bytes(), Format::Json).unwrap(),
    )
    .unwrap();
    assert!(value.canonical_json().as_bytes().len() < podway_config::MAX_PROCEDURE_DOCUMENT_BYTES);
    overflow(
        project_successor_graph(&value),
        "successor source projection characters",
    );
    let pretty = overflow(
        project_successor_source(&value, Format::Json),
        "successor formatted source characters",
    );
    let formatted = overflow(
        format_successor_definition(FormatRequest {
            source: &source,
            source_path: "dense.json",
            format: Format::Json,
        }),
        "successor formatted source characters",
    );
    assert_eq!(pretty, formatted);
    let mut expanded = chain(1);
    for n in 0..80 {
        expanded["node_definitions"][format!("unused-{n}")]=action((0..128).map(|i|json!({"id":format!("item-{i}"),"type":"text","prompt":"x","required":true})).collect());
    }
    let source = serde_json::to_vec(&expanded).unwrap();
    assert!(source.len() < podway_config::MAX_PROCEDURE_DOCUMENT_BYTES);
    let parsed = parse_successor_document(&source, Format::Json).unwrap();
    overflow(
        validate_successor_definition(parsed),
        "successor source projection characters",
    );
}

fn crossing_base(action_escape: bool) -> Value {
    let mut value = json!({"schema":"podway.procedure/v3","id":"crossing","version":"1","name":"Crossing","purpose":"Verify the one anchor boundary",
        "node_definitions":{
            "gate":decision(vec![json!({"id":"shared","label":"Prefix"}),json!({"id":"design","label":"Design"})]),
            "shared":decision(vec![json!({"id":"rewind","label":"Return to prefix gate"})]),
            "design":action(vec![]),"ret":decision(vec![json!({"id":"finish","label":"Finish"}),json!({"id":"escape","label":"Escape"})]),"finish":action(vec![])},
        "graph":{"entry":"gate","nodes":[
            {"id":"gate","use":"gate","routes":{"shared":{"effect":"advance","to":"shared"},"design":{"effect":"advance","to":"design"}}},
            {"id":"shared","use":"shared","routes":{"rewind":{"effect":"rework","to":"gate"}}},
            {"id":"design","use":"design","next":"ret"},
            {"id":"ret","use":"ret","routes":{"finish":{"effect":"advance","to":"finish"},"escape":{"effect":"advance","to":"shared"}}},
            {"id":"finish","use":"finish","terminal":true}]},
        "execution_group":{"anchor":"design","success_return":"ret","plan_inputs":[],"result_items":[],"replan_decisions":[]}});
    if action_escape {
        value["node_definitions"]["escape"] = action(vec![]);
        value["graph"]["nodes"][3]["routes"]["escape"]["to"] = json!("escape");
        value["graph"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"escape","use":"escape","next":"shared"}));
    }
    value
}
fn conditional_port() -> Value {
    let mut value = base();
    value["node_definitions"]["design"]["items"] = json!([
        {"id":"mode","type":"choice","prompt":"Mode","required":true,"choices":["simple","complex"]},
        {"id":"details","type":"text","prompt":"Details","required":false,"required_when":[{"item":"mode","equals":"complex"}]}]);
    value["execution_group"]["plan_inputs"] = json!(["mode", "details"]);
    value
}
fn reject_field(value: &Value, expected_field: &str, expected_reason: &str) {
    match admit(value) {
        Err(ConfigError::InvalidValue { field, reason }) => {
            assert_eq!(field, expected_field);
            assert!(reason.contains(expected_reason), "{reason}");
        }
        Err(other) => panic!("wrong error category: {other}"),
        Ok(_) => panic!("expected {expected_field}: {expected_reason}; document was admitted"),
    }
}
#[test]
fn frame002_settlement_regression_decision_advance_cannot_escape_returned_scope() {
    reject_field(
        &crossing_base(false),
        "graph.nodes.routes",
        "anchor boundary",
    );
}
#[test]
fn frame002_settlement_regression_action_next_cannot_escape_returned_scope() {
    reject_field(&crossing_base(true), "graph.nodes.next", "anchor boundary");
}
#[test]
fn frame002_settlement_regression_selected_plan_cannot_omit_controller() {
    let mut value = conditional_port();
    value["execution_group"]["plan_inputs"] = json!(["details"]);
    reject_controller(&value);
}
#[test]
fn frame002_settlement_regression_selected_plan_cannot_reverse_controller_order() {
    let mut value = conditional_port();
    value["execution_group"]["plan_inputs"] = json!(["details", "mode"]);
    reject_controller(&value);
}

fn tracked_base() -> Value {
    let mut value = base();
    value["goal_tracking"] = json!(true);
    let mut goal = decision(
        ["achieved", "not-achieved", "superseded"]
            .iter()
            .map(|id| json!({"id":id,"label":id}))
            .collect(),
    );
    goal["assessment"] = json!({"target":"session_goal","outcomes":{"achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"}});
    value["node_definitions"]["goal"] = goal;
    value["graph"]["nodes"][2]["routes"]["success"]["to"] = json!("goal");
    value["graph"]["nodes"].as_array_mut().unwrap().push(json!({"id":"goal","use":"goal","routes":{"achieved":{"effect":"advance","to":"finish"},"not-achieved":{"effect":"advance","to":"finish"},"superseded":{"effect":"advance","to":"finish"}}}));
    value
}

#[test]
fn frame002_goal_assessment_belongs_to_base_with_complete_coverage_and_dominance() {
    let mut value = tracked_base();
    schema_accepts(&accept(&value));
    let mut missing = value.clone();
    missing["node_definitions"]["goal"]["options"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"unknown","label":"Unknown"}));
    missing["graph"]["nodes"][4]["routes"]["unknown"] = json!({"effect":"advance","to":"finish"});
    reject(&missing, "cover every option and outcome");
    let mut bypass = value.clone();
    bypass["node_definitions"]["review"]["options"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"bypass","label":"Bypass"}));
    bypass["graph"]["nodes"][2]["routes"]["bypass"] = json!({"effect":"advance","to":"finish"});
    reject_field(
        &bypass,
        "assessment",
        "each base terminal requires a dominating session-goal assessment at or after group return",
    );
    value.as_object_mut().unwrap().remove("goal_tracking");
    reject(&value, "goal_tracking");
}

#[test]
fn frame_audit_config_rejects_replan_on_session_goal_assessment() {
    let mut value = tracked_base();
    value["execution_group"]["replan_decisions"] = json!(["review", "goal"]);
    value["graph"]["nodes"][4]["evidence_from"] =
        json!([{"scope": "execution_result", "items": ["verdict"]}]);
    value["node_definitions"]["goal"]["options"][1]["guards"] = json!([{
        "evidence": {"scope": "execution_result", "item": "verdict"},
        "equals": "failure"
    }]);
    // Full outcome coverage, policy membership and the guard already pass. The ordinary
    // guarded replan on review remains available.
    schema_accepts(&accept(&value));
    value["graph"]["nodes"][4]["routes"]["not-achieved"] = json!({"effect": "replan"});
    reject_field(
        &value,
        "graph.nodes.routes",
        "session-goal assessment cannot use a replan route",
    );
}

fn tracked_base_with_prefix_assessment() -> Value {
    let mut prefix_and_suffix = tracked_base();
    let mut prefix_goal = prefix_and_suffix["graph"]["nodes"][4].clone();
    prefix_goal["id"] = json!("prefix-goal");
    for route in prefix_goal["routes"].as_object_mut().unwrap().values_mut() {
        route["to"] = json!("prefix");
    }
    prefix_and_suffix["graph"]["entry"] = json!("prefix-goal");
    prefix_and_suffix["graph"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(prefix_goal);
    prefix_and_suffix
}

#[test]
fn frame_audit_config_prefix_assessment_cannot_qualify_completion() {
    let mut prefix_only = tracked_base_with_prefix_assessment();
    schema_accepts(&accept(&prefix_only));
    prefix_only["graph"]["nodes"][2]["routes"]["success"]["to"] = json!("finish");
    prefix_only["graph"]["nodes"]
        .as_array_mut()
        .unwrap()
        .remove(4);
    reject_field(
        &prefix_only,
        "assessment",
        "each base terminal requires a dominating session-goal assessment at or after group return",
    );
}

#[test]
fn frame_audit_config_rejects_one_branch_that_bypasses_the_post_return_assessment() {
    let mut prefix_and_suffix = tracked_base_with_prefix_assessment();
    schema_accepts(&accept(&prefix_and_suffix));
    prefix_and_suffix["node_definitions"]["review"]["options"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "bypass", "label": "Bypass"}));
    prefix_and_suffix["graph"]["nodes"][2]["routes"]["bypass"] =
        json!({"effect": "advance", "to": "finish"});
    reject_field(
        &prefix_and_suffix,
        "assessment",
        "each base terminal requires a dominating session-goal assessment at or after group return",
    );
}

#[test]
fn frame_audit_config_allows_goal_assessment_at_group_return() {
    let mut value = tracked_base();
    value["execution_group"]["success_return"] = json!("goal");
    value["graph"]["nodes"][1]["next"] = json!("goal");
    value["graph"]["nodes"][2]["routes"]["success"]["to"] = json!("finish");
    for route in value["graph"]["nodes"][4]["routes"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        route["to"] = json!("review");
    }
    schema_accepts(&accept(&value));
}

#[derive(Clone)]
struct OfflineSchemas(std::collections::BTreeMap<String, Value>);
impl jsonschema::Retrieve for OfflineSchemas {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        self.0.get(uri.as_str()).cloned().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "schema is not enrolled locally",
            )
            .into()
        })
    }
}
fn schema_accepts(value: &ValidatedSuccessorDefinition) {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/schemas");
    let mut resources = std::collections::BTreeMap::new();
    for file in std::fs::read_dir(directory).unwrap() {
        let path = file.unwrap().path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let schema: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        resources.insert(schema["$id"].as_str().unwrap().to_owned(), schema);
    }
    let schema = resources
        .get(match value.definition() {
            SuccessorDefinition::Base(_) => "urn:podway:schema:procedure:v3",
            SuccessorDefinition::Group(_) => "urn:podway:schema:execution-group:v1",
        })
        .unwrap()
        .clone();
    let validator = jsonschema::options()
        .with_retriever(OfflineSchemas(resources))
        .build(&schema)
        .unwrap();
    let canonical: Value = serde_json::from_str(value.canonical_json().as_str()).unwrap();
    let errors = validator
        .iter_errors(&canonical)
        .map(|e| e.to_string())
        .collect::<Vec<_>>();
    assert!(errors.is_empty(), "{errors:?}");
}
fn chain(count: usize) -> Value {
    json!({"schema":"podway.execution-group/v1","id":"bounded","version":"1","name":"Bounded","purpose":"Exercise actual graph limits","plan_inputs":[],
        "node_definitions":{"work":action(vec![])},
        "graph":{"entry":"node-0","nodes":(0..count).map(|n|if n+1==count {json!({"id":format!("node-{n}"),"use":"work","terminal":true})}else{json!({"id":format!("node-{n}"),"use":"work","next":format!("node-{}",n+1)})}).collect::<Vec<_>>()}})
}
#[test]
fn frame002_canonical_schema_agreement_covers_all_shared_item_kinds_and_headers() {
    schema_accepts(&accept(&base()));
    schema_accepts(&accept(&group()));
    let item =
        |id: &str, kind: &str| json!({"id":id,"type":kind,"prompt":"Record","required":true});
    let mut values = vec![
        item("plan", "confirm"),
        item("text", "text"),
        item("choice", "choice"),
        item("integer", "integer"),
        item("list", "list"),
        item("artifact", "artifact"),
        item("check", "check_result"),
    ];
    values[2]["choices"] = json!(["yes", "no"]);
    values[6]["operation_id"] = json!("verify");
    values[6]["operation_digest"] = json!(format!("sha256:{}", "a".repeat(64)));
    values[6]["accepted_outcomes"] = json!(["pass", "fail"]);
    let mut v = group();
    v["plan_inputs"] = json!(values);
    v["version"] = json!(" ");
    v["description"] = json!("");
    let accepted = accept(&v);
    schema_accepts(&accepted);
    let canonical: Value = serde_json::from_str(accepted.canonical_json().as_str()).unwrap();
    assert_eq!(
        canonical["plan_inputs"][4]["max_total_length"],
        podway_core::DEFAULT_LIST_MAX_TOTAL_SCALARS_V2
    );
    for (field, limit) in [
        ("name", 120),
        ("purpose", 500),
        ("description", 1000),
        ("version", 64),
    ] {
        let mut value = group();
        value[field] = json!("한".repeat(limit));
        schema_accepts(&accept(&value));
        value[field] = json!("한".repeat(limit + 1));
        assert!(admit(&value).is_err());
    }
}
#[test]
fn frame002_actual_collection_limits_cover_nodes_definitions_ports_routes_and_manual_targets() {
    let max = podway_core::execution_groups::MAX_GROUP_PLACEMENTS_V1;
    let at = accept(&chain(max));
    schema_accepts(&at);
    assert!(
        project_successor_graph(&at).unwrap().projection().len()
            < podway_core::MAX_PROCEDURE_DOCUMENT_BYTES
    );
    assert!(admit(&chain(max + 1)).is_err());
    for (count, ok) in [
        (podway_core::MAX_NODE_DEFINITIONS_PER_PROCEDURE_V2, true),
        (
            podway_core::MAX_NODE_DEFINITIONS_PER_PROCEDURE_V2 + 1,
            false,
        ),
    ] {
        let mut v = chain(1);
        v["node_definitions"] = Value::Object(
            (0..count)
                .map(|n| {
                    (
                        if n == 0 {
                            "work".into()
                        } else {
                            format!("unused-{n}")
                        },
                        action(vec![]),
                    )
                })
                .collect(),
        );
        assert_eq!(admit(&v).is_ok(), ok);
    }
    for (count, ok) in [(128, true), (129, false)] {
        let mut v = chain(1);
        v["plan_inputs"] = json!(
            (0..count)
                .map(|n| text(&format!("input-{n}")))
                .collect::<Vec<_>>()
        );
        assert_eq!(admit(&v).is_ok(), ok);
    }
    for (count, ok) in [(64, true), (65, false)] {
        let mut v = chain(65);
        v["manual_rework"] =
            json!({"allowed_targets":(0..count).map(|n|format!("node-{n}")).collect::<Vec<_>>()});
        assert_eq!(admit(&v).is_ok(), ok);
    }
    for (count, ok) in [(8, true), (9, false)] {
        let mut v = chain(2);
        v["node_definitions"]["work"] = decision(
            (0..count)
                .map(|n| json!({"id":format!("option-{n}"),"label":"Continue"}))
                .collect(),
        );
        v["node_definitions"]["finish"] = action(vec![]);
        v["graph"]["nodes"][1]["use"] = json!("finish");
        remove(&mut v, "/graph/nodes/0", "next");
        v["graph"]["nodes"][0]["routes"] = Value::Object(
            (0..count)
                .map(|n| {
                    (
                        format!("option-{n}"),
                        json!({"effect":"advance","to":"node-1"}),
                    )
                })
                .collect(),
        );
        assert_eq!(admit(&v).is_ok(), ok);
    }
    // Declared edge bounds remain Core-owned; node/route composition can be tighter with one real success exit.
    assert_eq!(
        podway_core::execution_groups::MAX_GROUP_EDGES_V1,
        podway_core::MAX_GRAPH_EDGES_PER_PROCEDURE_V2
    );
}

fn text(id: &str) -> Value {
    json!({"id":id,"type":"text","prompt":"Record plan","required":true})
}
fn verdict() -> Value {
    json!({"id":"verdict","type":"choice","prompt":"Record verdict","required":true,"choices":["success","failure"]})
}
fn action(items: Vec<Value>) -> Value {
    let mut v = json!({"type":"action","title":"Work","intent":"Complete the assigned work"});
    if !items.is_empty() {
        v["items"] = json!(items);
    }
    v
}
fn decision(options: Vec<Value>) -> Value {
    json!({"type":"decision","title":"Choose","objective":"Choose the permitted route","prompt":"Choose","reason":{"required":true},"options":options})
}
fn base() -> Value {
    json!({"schema":"podway.procedure/v3","id":"base","version":"1","name":"Base","purpose":"Coordinate one group",
        "node_definitions":{"prefix":action(vec![]),"design":action(vec![text("plan")]),"review":decision(vec![json!({"id":"success","label":"Accept"}),json!({"id":"replan","label":"Replan","guards":[{"evidence":{"scope":"execution_result","item":"verdict"},"equals":"failure"}]})]),"finish":action(vec![])},
        "graph":{"entry":"prefix","nodes":[{"id":"prefix","use":"prefix","next":"design"},{"id":"design","use":"design","next":"review"},{"id":"review","use":"review","evidence_from":[{"scope":"execution_result","items":["verdict"]}],"routes":{"success":{"effect":"advance","to":"finish"},"replan":{"effect":"replan"}}},{"id":"finish","use":"finish","terminal":true}]},
        "manual_rework":{"allowed_targets":["prefix","finish"]},
        "execution_group":{"anchor":"design","success_return":"review","plan_inputs":["plan"],"result_items":[verdict()],"replan_decisions":["review"]}})
}
fn group() -> Value {
    let stage = json!({"id":"stage","type":"choice","prompt":"Record stage","required":true,"choices":["not-scored","lightweight","scored"]});
    let score = json!({"id":"score","type":"integer","prompt":"Record score","required":true,"minimum":0,"maximum":100});
    let explanation = json!({"id":"explanation","type":"text","prompt":"Explain scored judgment","required":false,"required_when":[{"item":"stage","equals":"scored"}]});
    let guard = |item: &str, op: &str, value: Value| {
        let mut v = json!({"evidence":{"scope":"local","node":"assess","item":item}});
        v[op] = value;
        v
    };
    json!({"schema":"podway.execution-group/v1","id":"group","version":"1","name":"Group","purpose":"Perform the current plan","plan_inputs":[text("plan")],
        "node_definitions":{"work":action(vec![]),"assess":action(vec![verdict(),stage,score,explanation]),"choose":decision(vec![json!({"id":"success","label":"Accept"}),json!({"id":"repair","label":"Repair"}),json!({"id":"replan","label":"Replan","guards":[guard("verdict","equals",json!("failure")),guard("stage","equals",json!("scored")),guard("score","at_least",json!(80))]})]),"finish":action(vec![verdict()])},
        "graph":{"entry":"work","nodes":[{"id":"work","use":"work","evidence_from":[{"scope":"plan","items":["plan"]}],"next":"assess"},{"id":"assess","use":"assess","next":"choose"},{"id":"choose","use":"choose","evidence_from":[{"scope":"local","node":"assess","items":["verdict","stage","score"]}],"routes":{"success":{"effect":"advance","to":"finish"},"repair":{"effect":"rework","to":"work"},"replan":{"effect":"replan"}}},{"id":"finish","use":"finish","terminal":true}]},
        "manual_rework":{"allowed_targets":["work"]}})
}
fn admit(value: &Value) -> Result<ValidatedSuccessorDefinition, ConfigError> {
    validate_successor_definition(parse_successor_document(
        &serde_json::to_vec(value).unwrap(),
        Format::Json,
    )?)
}
fn accept(value: &Value) -> ValidatedSuccessorDefinition {
    admit(value).unwrap_or_else(|e| panic!("{e}\n{value}"))
}
fn reject(value: &Value, reason: &str) {
    let error = admit(value).expect_err("mutated document must fail admission");
    assert!(
        error.to_string().contains(reason),
        "{error}, expected {reason}"
    );
}
fn remove(value: &mut Value, pointer: &str, key: &str) {
    value
        .pointer_mut(pointer)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove(key);
}

#[test]
fn frame002_admits_base_group_ports_defaults_and_current_public_refusal() {
    let mut delegated = base();
    delegated["graph"]["nodes"][1]["delegation"] = json!({"binding_ref":"native","role_ref":"worker","role_snapshot_digest":format!("sha256:{}","a".repeat(64)),"max_rounds":1,"after_accept":"retain"});
    let delegated = accept(&delegated);
    schema_accepts(&delegated);
    let source = project_successor_source(&delegated, Format::Json).unwrap();
    assert_eq!(
        accept(&serde_json::from_str(&source).unwrap()).digest(),
        delegated.digest()
    );
    let b = accept(&base());
    let g = accept(&group());
    validate_group_interface(&b, &g).unwrap();
    for v in [&b, &g] {
        let canonical: Value = serde_json::from_str(v.canonical_json().as_str()).unwrap();
        assert_eq!(accept(&canonical).digest(), v.digest());
        assert!(
            podway_config::parse_procedure_document(v.canonical_json().as_bytes(), Format::Json)
                .is_err()
        );
        assert!(
            podway_config::sniff_procedure_schema(v.canonical_json().as_bytes(), Format::Json)
                .is_none()
        );
        assert!(podway_core::verify_canonical_json_v1(v.canonical_json().as_bytes()).is_ok());
    }
    let plan: Value = serde_json::from_str(g.canonical_json().as_str()).unwrap();
    assert_eq!(
        plan["plan_inputs"][0]["max_length"],
        podway_core::DEFAULT_TEXT_MAX_SCALARS_V2
    );
    assert_eq!(plan["plan_inputs"][0]["multiline"], true);
    let projection: Value =
        serde_json::from_str(project_successor_graph(&b).unwrap().projection()).unwrap();
    assert!(
        projection["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == "prefix" && n["manual_rework_before_first_activation"] == true)
    );
    assert!(
        projection["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == "finish" && n["manual_rework_before_first_activation"] == false)
    );
    let replan = projection["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["effect"] == "replan")
        .unwrap();
    assert!(replan.get("to").is_none());
}
#[test]
fn frame004_admitted_definitions_drive_core_group_lifecycle() {
    use podway_core::execution_groups::{
        ActivationIdentityInputV1, ActivationIdentityV1, EvidenceSourceV3, ExecutionScopeV1,
        ExecutionSessionV3, GroupExecutionIdV1, GroupExecutionOutcomeV1, ReplanFeedbackV1,
        SelectionClosureCauseV1,
    };
    use podway_core::{
        AttemptId, AttemptLifecycle, CheckResultInputBasisV2, DomainError, ItemId, OptionId,
        ReasonV2, RecordedItemValueV2, Revision, SessionId, UnixMillis,
    };

    fn attempt(number: u8) -> AttemptId {
        AttemptId::new(format!("00000000-0000-4000-8000-{number:012}")).unwrap()
    }

    fn record(session: &mut ExecutionSessionV3, item: &str, value: RecordedItemValueV2) {
        let target = session.active_attempt().unwrap().target().clone();
        session
            .record_item(
                &target,
                &ItemId::new(item).unwrap(),
                Revision::ZERO,
                Some(value),
                UnixMillis::UNIX_EPOCH,
            )
            .unwrap();
    }

    let failure_detail = json!({
        "id": "detail", "type": "text", "prompt": "Explain the failure",
        "required": false, "required_when": [{"item": "verdict", "equals": "failure"}],
    });
    let mut base_source = base();
    base_source["execution_group"]["result_items"] = json!([verdict(), failure_detail]);
    base_source["graph"]["nodes"][2]["evidence_from"][0]["items"] = json!(["verdict", "detail"]);
    let base_definition = accept(&base_source);
    let mut group_source = group();
    group_source["node_definitions"]["finish"]["items"] = json!([verdict(), failure_detail]);
    let group_definition = accept(&group_source);
    validate_group_interface(&base_definition, &group_definition).unwrap();

    group_source["version"] = json!("2");
    let alternate_group = accept(&group_source);
    validate_group_interface(&base_definition, &alternate_group).unwrap();
    assert_ne!(alternate_group.digest(), group_definition.digest());

    let SuccessorDefinition::Base(base) = base_definition.definition() else {
        panic!("expected an admitted base");
    };
    let SuccessorDefinition::Group(group) = group_definition.definition() else {
        panic!("expected an admitted group");
    };
    let time = UnixMillis::UNIX_EPOCH;
    let session_id = SessionId::new("00000000-0000-4000-8000-000000000100").unwrap();
    let execution_id = GroupExecutionIdV1::new("00000000-0000-4000-8000-000000000200").unwrap();
    let mut session = ExecutionSessionV3::begin(
        session_id.clone(),
        base.clone(),
        base_definition.digest().clone(),
        None,
        attempt(1),
        time,
    )
    .unwrap();
    session
        .complete(&session.fence(), Some(attempt(2)), time)
        .unwrap();
    record(
        &mut session,
        "plan",
        RecordedItemValueV2::text("Use the admitted plan").unwrap(),
    );
    let binding = session.plan_binding().unwrap();
    let activation = |definition: &ValidatedSuccessorDefinition| {
        ActivationIdentityV1::new(ActivationIdentityInputV1 {
            execution: execution_id.clone(),
            session: session_id.clone(),
            base_digest: base_definition.digest().clone(),
            definition_digest: definition.digest().clone(),
            anchor: base.boundary().anchor().clone(),
            goal_revision: None,
            plan: binding.clone(),
            predecessor: None,
            working_copy_basis: CheckResultInputBasisV2::new(
                "Fixture source",
                base_definition.digest().clone(),
            )
            .unwrap(),
            prior_work: vec![],
        })
        .unwrap()
    };

    let unchanged = session.clone();
    assert!(matches!(
        session.activate(
            &session.fence(),
            activation(&alternate_group),
            group.clone(),
            group_definition.digest(),
            attempt(3),
            time,
        ),
        Err(DomainError::InvalidState {
            reason: "activation identity does not bind the exact current design"
        })
    ));
    assert_eq!(session, unchanged);
    session
        .activate(
            &session.fence(),
            activation(&group_definition),
            group.clone(),
            group_definition.digest(),
            attempt(3),
            time,
        )
        .unwrap();
    let selected = session.selected().unwrap().identity();
    assert_eq!(selected.base_digest(), base_definition.digest());
    assert_eq!(selected.definition_digest(), group_definition.digest());
    assert_eq!(selected.plan(), &binding);
    let (source, slot) = session
        .current_evidence(&EvidenceSourceV3::Plan, &ItemId::new("plan").unwrap())
        .unwrap();
    assert_eq!(source.attempt(), &attempt(2));
    assert_eq!(
        slot.value().unwrap().as_text(),
        Some("Use the admitted plan")
    );

    session
        .complete(&session.fence(), Some(attempt(4)), time)
        .unwrap();
    record(
        &mut session,
        "verdict",
        RecordedItemValueV2::choice("success").unwrap(),
    );
    record(
        &mut session,
        "stage",
        RecordedItemValueV2::choice("not-scored").unwrap(),
    );
    record(&mut session, "score", RecordedItemValueV2::integer(0));
    session
        .complete(&session.fence(), Some(attempt(5)), time)
        .unwrap();
    session
        .decide(
            &session.fence(),
            OptionId::new("success").unwrap(),
            ReasonV2::new("Work finished").unwrap(),
            None,
            attempt(6),
            time,
        )
        .unwrap();
    record(
        &mut session,
        "verdict",
        RecordedItemValueV2::choice("failure").unwrap(),
    );
    let unchanged = session.clone();
    assert_eq!(
        session.complete(&session.fence(), Some(attempt(7)), time),
        Err(DomainError::RequiredItemsMissing),
    );
    assert_eq!(session, unchanged);
    record(
        &mut session,
        "detail",
        RecordedItemValueV2::text("Plan requires revision").unwrap(),
    );
    session
        .complete(&session.fence(), Some(attempt(7)), time)
        .unwrap();
    let review = session.active_attempt().unwrap().target();
    assert_eq!(review.scope(), &ExecutionScopeV1::Base);
    assert_eq!(review.node(), base.boundary().success_return());
    assert_eq!(
        session.selected().unwrap().terminal_attempt(),
        Some(&attempt(6))
    );
    let (source, result) = session
        .current_evidence(
            &EvidenceSourceV3::ExecutionResult,
            &ItemId::new("detail").unwrap(),
        )
        .unwrap();
    assert_eq!(source.attempt(), &attempt(6));
    assert_eq!(
        result.value().unwrap().as_text(),
        Some("Plan requires revision")
    );

    let feedback = ReplanFeedbackV1::new(
        review.clone(),
        OptionId::new("replan").unwrap(),
        "Current result reports failure".into(),
        session
            .selected()
            .unwrap()
            .identity()
            .working_copy_basis()
            .clone(),
        "Retain the completed execution".into(),
    )
    .unwrap();
    session
        .replan(&session.fence(), feedback, attempt(8), time)
        .unwrap();
    assert_eq!(
        session.active_attempt().unwrap().target().node(),
        base.boundary().anchor()
    );
    assert!(session.selected().is_none());
    let closed = &session.group_history()[0];
    assert_eq!(closed.outcome(), GroupExecutionOutcomeV1::Completed);
    assert_eq!(
        closed.closure().unwrap().cause,
        SelectionClosureCauseV1::Replan
    );
    assert_eq!(
        session
            .attempt_history()
            .iter()
            .find(|row| row.target().attempt() == &attempt(6))
            .unwrap()
            .lifecycle(),
        AttemptLifecycle::Completed,
    );
}

#[test]
fn frame002_rejects_anchor_bypass_kind_skip_and_crossing_routes() {
    let mut bypass = base();
    bypass["node_definitions"]["prefix"] = decision(vec![
        json!({"id":"design","label":"Design"}),
        json!({"id":"bypass","label":"Bypass"}),
    ]);
    remove(&mut bypass, "/graph/nodes/0", "next");
    bypass["graph"]["nodes"][0]["routes"] = json!({"design":{"effect":"advance","to":"design"},"bypass":{"effect":"advance","to":"finish"}});
    reject(&bypass, "dominate every terminal");
    let original = base();
    let mut v = original.clone();
    v["graph"]["nodes"][1]["next"] = json!("finish");
    reject(&v, "advancing to success_return");
    let mut v = original.clone();
    remove(&mut v, "/graph/nodes/1", "next");
    v["graph"]["nodes"][1]["terminal"] = json!(true);
    reject(&v, "advancing to success_return");
    let mut v = original.clone();
    v["graph"]["nodes"][1]["skip"] = json!({"allowed":true,"reason_required":true});
    reject(&v, "unskippable");
    let mut v = original.clone();
    v["graph"]["nodes"][0]["next"] = json!("finish");
    reject(&v, "reachable");
    let mut v = original.clone();
    v["node_definitions"]["review"]["options"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"cross","label":"Cross"}));
    v["graph"]["nodes"][2]["routes"]["cross"] = json!({"effect":"rework","to":"design"});
    reject(&v, "anchor boundary");
    let mut v = original.clone();
    v["node_definitions"]["review"]["options"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"cross","label":"Cross"}));
    v["graph"]["nodes"][2]["routes"]["cross"] = json!({"effect":"rework","to":"prefix"});
    reject(&v, "anchor boundary");
    let mut v = original.clone();
    v["manual_rework"]["allowed_targets"] = json!(["design"]);
    reject(&v, "cannot target the anchor");
    let mut v = original.clone();
    v["execution_group"]["replan_decisions"] = json!(["prefix"]);
    reject(&v, "must name decisions");
    let mut v = original.clone();
    v["execution_group"]["replan_decisions"] = json!([]);
    reject(&v, "anchor-return policy");
    let mut v = original.clone();
    v["graph"]["nodes"][3]["use"] = json!("review");
    reject(&v, "kinds disagree");
}
#[test]
fn frame002_rejects_unknown_cross_scope_and_incompatible_references() {
    let original = group();
    let cases = [
        ("/graph/nodes/0/use", json!("unknown"), "unknown definition"),
        (
            "/graph/nodes/0/next",
            json!("outside"),
            "unknown local placement",
        ),
        (
            "/graph/nodes/0/evidence_from/0/items",
            json!(["missing"]),
            "unknown item",
        ),
        (
            "/graph/nodes/0/evidence_from/0/scope",
            json!("execution_result"),
            "only to the base",
        ),
        (
            "/graph/nodes/2/evidence_from/0/node",
            json!("outside"),
            "unknown local placement",
        ),
        (
            "/graph/nodes/2/evidence_from/0/node",
            json!("choose"),
            "own consumer",
        ),
    ];
    for (pointer, value, reason) in cases {
        let mut v = original.clone();
        *v.pointer_mut(pointer).unwrap() = value;
        reject(&v, reason);
    }
    let mut v = original.clone();
    v["graph"]["nodes"][2]["routes"]["repair"]["to"] = json!("other-group-design");
    reject_field(&v, "graph.nodes", "unknown local placement");
    let mut v = original.clone();
    v["plan_inputs"][0]["required"] = json!(false);
    v["plan_inputs"][0]["required_when"] = json!([{"item":"missing","equals":true}]);
    reject(&v, "earlier unconditionally required");
    let mut v = base();
    v["graph"]["nodes"][0]["evidence_from"] =
        json!([{"scope":"execution_result","items":["verdict"]}]);
    reject(&v, "only after group return");
    let mut v = base();
    v["execution_group"]["plan_inputs"] = json!(["missing"]);
    reject(&v, "anchor item");
}
#[test]
fn frame002_guard_eligibility_distinguishes_required_reference_and_optional_item() {
    let original = group();
    let mut v = original.clone();
    remove(&mut v, "/node_definitions/choose/options/2", "guards");
    reject(&v, "replan requires");
    let mut v = original.clone();
    v["graph"]["nodes"][2]["evidence_from"][0]["required"] = json!(false);
    reject(&v, "required explicitly selected");
    let mut v = original.clone();
    remove(&mut v, "/graph/nodes/2/evidence_from/0", "items");
    reject(&v, "required explicitly selected");
    let mut v = original.clone();
    v["node_definitions"]["assess"]["items"][2]["required"] = json!(false);
    accept(&v);
    let mut v = original.clone();
    v["node_definitions"]["choose"]["options"][2]["guards"][2]["at_least"] = json!("eighty");
    assert!(
        matches!(admit(&v),Err(ConfigError::InvalidDocument{reason}) if reason.contains("eighty"))
    );
    let mut v = original.clone();
    v["node_definitions"]["choose"]["options"][2]["guards"][0]["equals"] = json!(true);
    reject(&v, "operator and literal");
    let mut v = original.clone();
    v["node_definitions"]["choose"]["options"][2]["guards"][0]["evidence"]["scope"] =
        json!("historical");
    reject_field(&v, "evidence.scope", "closed local");
    // Custom eligible guards need no default assessment record or fixed native80 policy.
    let mut v = original.clone();
    v["node_definitions"]["choose"]["options"][2]["guards"] =
        json!([{"evidence":{"scope":"plan","item":"plan"},"non_empty":true}]);
    v["graph"]["nodes"][2]["evidence_from"] = json!([{"scope":"plan","items":["plan"]}]);
    accept(&v);
}
#[test]
fn frame002_replan_only_exit_keeps_real_success_and_checks_mixed_ordinary_cycles() {
    let mut v = group();
    v["node_definitions"]["choose"]["options"]
        .as_array_mut()
        .unwrap()
        .remove(1);
    v["graph"]["nodes"][2]["routes"]
        .as_object_mut()
        .unwrap()
        .remove("repair");
    let only = decision(vec![
        json!({"id":"replan","label":"Replan","guards":[{"evidence":{"scope":"plan","item":"plan"},"non_empty":true}]}),
    ]);
    v["node_definitions"]["escape"] = only;
    v["graph"]["nodes"].as_array_mut().unwrap().push(json!({"id":"escape","use":"escape","evidence_from":[{"scope":"plan","items":["plan"]}],"routes":{"replan":{"effect":"replan"}}}));
    v["node_definitions"]["choose"]["options"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"escape","label":"Escalate"}));
    v["graph"]["nodes"][2]["routes"]["escape"] = json!({"effect":"advance","to":"escape"});
    accept(&v);
    let mut cycle = v.clone();
    cycle["node_definitions"]["escape"]["options"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"loop","label":"Loop"}));
    cycle["graph"]["nodes"][4]["routes"]["loop"] = json!({"effect":"advance","to":"escape"});
    reject(&cycle, "forward cycles");
    let mut unreachable = v.clone();
    unreachable["graph"]["nodes"][2]["routes"]["success"]["to"] = json!("escape");
    reject(&unreachable, "reachable");
    let mut missing = v.clone();
    remove(&mut missing, "/graph/nodes/3", "terminal");
    missing["graph"]["nodes"][3]["next"] = json!("escape");
    reject(&missing, "success-terminal");
}
#[test]
fn frame002_local_dominance_skippability_rework_and_termination_are_mandatory() {
    let mut diamond = group();
    diamond["node_definitions"]["work"] = decision(vec![
        json!({"id":"assess","label":"Assess"}),
        json!({"id":"bypass","label":"Bypass"}),
    ]);
    remove(&mut diamond, "/graph/nodes/0", "next");
    diamond["graph"]["nodes"][0]["routes"] = json!({"assess":{"effect":"advance","to":"assess"},"bypass":{"effect":"advance","to":"choose"}});
    reject(&diamond, "strictly dominate");
    diamond["graph"]["nodes"][2]["evidence_from"][0]["required"] = json!(false);
    diamond["graph"]["nodes"][2]["evidence_from"]
        .as_array_mut()
        .unwrap()
        .push(json!({"scope":"plan","items":["plan"]}));
    diamond["node_definitions"]["choose"]["options"][2]["guards"] =
        json!([{"evidence":{"scope":"plan","item":"plan"},"non_empty":true}]);
    accept(&diamond);
    let mut v = group();
    v["node_definitions"]["finish"]["items"] = v["node_definitions"]["assess"]["items"].clone();
    v["graph"]["nodes"][2]["evidence_from"][0]["node"] = json!("finish");
    for guard in v["node_definitions"]["choose"]["options"][2]["guards"]
        .as_array_mut()
        .unwrap()
    {
        guard["evidence"]["node"] = json!("finish");
    }
    reject_field(&v, "evidence", "strictly dominate");
    let mut v = group();
    v["graph"]["nodes"][1]["skip"] = json!({"allowed":true,"reason_required":true});
    reject(&v, "skippable");
    let mut v = group();
    v["graph"]["nodes"][2]["routes"]["repair"]["to"] = json!("finish");
    reject(&v, "must dominate");
    let mut v = group();
    v["graph"]["nodes"][0]["next"] = json!("choose");
    reject(&v, "reachable");
    let mut v = group();
    v["graph"]["nodes"][3]["next"] = json!("work");
    remove(&mut v, "/graph/nodes/3", "terminal");
    reject(&v, "forward cycles");
    let mut v = group();
    v["graph"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"other-terminal","use":"finish","terminal":true}));
    v["node_definitions"]["choose"]["options"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"other","label":"Other"}));
    v["graph"]["nodes"][2]["routes"]["other"] = json!({"effect":"advance","to":"other-terminal"});
    reject(&v, "exactly one");
}
#[test]
fn frame002_interface_equality_ignores_only_prompts_and_preserves_declaration_order() {
    let b = accept(&base());
    let mut v = group();
    v["plan_inputs"][0]["prompt"] = json!("Different prompt");
    v["node_definitions"]["finish"]["items"][0]["prompt"] = json!("Different result prompt");
    validate_group_interface(&b, &accept(&v)).unwrap();
    for (pointer, change) in [
        ("/plan_inputs/0/required", json!(false)),
        ("/plan_inputs/0/max_length", json!(10)),
        (
            "/node_definitions/finish/items/0/choices",
            json!(["failure", "success"]),
        ),
    ] {
        let mut v = group();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        v.pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), change);
        assert!(validate_group_interface(&b, &accept(&v)).is_err());
    }
    let mut b = base();
    b["node_definitions"]["design"]["items"]
        .as_array_mut()
        .unwrap()
        .push(text("second"));
    b["execution_group"]["plan_inputs"] = json!(["plan", "second"]);
    let mut g = group();
    g["plan_inputs"]
        .as_array_mut()
        .unwrap()
        .push(text("second"));
    validate_group_interface(&accept(&b), &accept(&g)).unwrap();
    g["plan_inputs"].as_array_mut().unwrap().reverse();
    assert!(validate_group_interface(&accept(&b), &accept(&g)).is_err());
    let mut b = base();
    b["execution_group"]["result_items"]
        .as_array_mut()
        .unwrap()
        .push(text("second"));
    let mut g = group();
    g["node_definitions"]["finish"]["items"]
        .as_array_mut()
        .unwrap()
        .push(text("second"));
    validate_group_interface(&accept(&b), &accept(&g)).unwrap();
    g["node_definitions"]["finish"]["items"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(validate_group_interface(&accept(&b), &accept(&g)).is_err());
}
#[test]
fn frame002_source_format_and_canonical_digests_share_defaults_and_order_rules() {
    let original = accept(&group());
    let mut explicit = group();
    explicit["plan_inputs"][0]["min_length"] = json!(0);
    explicit["plan_inputs"][0]["max_length"] = json!(podway_core::DEFAULT_TEXT_MAX_SCALARS_V2);
    explicit["plan_inputs"][0]["multiline"] = json!(true);
    assert_eq!(accept(&explicit).digest(), original.digest());
    let mut changed = group();
    changed["plan_inputs"][0]["prompt"] = json!("Changed presentation");
    assert_ne!(accept(&changed).digest(), original.digest());
    let mut changed = group();
    changed["graph"]["nodes"].as_array_mut().unwrap().swap(0, 1);
    assert_ne!(accept(&changed).digest(), original.digest());
    let source = project_successor_source(&original, Format::Yaml).unwrap();
    let commented = format!("# Current group\n{source}# Retained tail\n");
    let formatted = format_successor_definition(FormatRequest {
        source: &commented,
        source_path: "group.yaml",
        format: Format::Yaml,
    })
    .unwrap();
    assert!(formatted.document().contains("# Current group"));
    assert!(formatted.document().contains("# Retained tail"));
    assert_eq!(formatted.digest(), original.digest());
    let again = format_successor_definition(FormatRequest {
        source: formatted.document(),
        source_path: "group.yaml",
        format: Format::Yaml,
    })
    .unwrap();
    assert_eq!(again.document(), formatted.document());
    assert!(!again.changed());
    for format in [Format::Yaml, Format::Json] {
        let source = project_successor_source(&original, format).unwrap();
        let admitted = validate_successor_definition(
            parse_successor_document(source.as_bytes(), format).unwrap(),
        )
        .unwrap();
        assert_eq!(admitted.digest(), original.digest());
    }
    let original_source = serde_json::to_string(&group()).unwrap();
    let reordered=original_source.replace("\"repair\":{\"effect\":\"rework\",\"to\":\"work\"},\"replan\":{\"effect\":\"replan\"},\"success\":{\"effect\":\"advance\",\"to\":\"finish\"}","\"success\":{\"effect\":\"advance\",\"to\":\"finish\"},\"replan\":{\"effect\":\"replan\"},\"repair\":{\"effect\":\"rework\",\"to\":\"work\"}");
    assert_ne!(
        reordered, original_source,
        "route-map reorder must actually apply"
    );
    let reordered = validate_successor_definition(
        parse_successor_document(reordered.as_bytes(), Format::Json).unwrap(),
    )
    .unwrap();
    assert_eq!(reordered.digest(), original.digest());
    assert_eq!(
        project_successor_graph(&reordered).unwrap(),
        project_successor_graph(&original).unwrap()
    );
}
#[test]
fn frame002_bounds_closed_shapes_and_conditional_controller_validation() {
    assert!(
        parse_successor_document(&serde_json::to_vec(&group()).unwrap(), Format::Yaml).is_err()
    );
    let mut v = group();
    v["execution_group"] = base()["execution_group"].clone();
    reject(&v, "cannot own");
    let mut v = group();
    v["unexpected"] = json!(true);
    assert!(
        matches!(admit(&v),Err(ConfigError::InvalidDocument{reason}) if reason.contains("unexpected"))
    );
    let mut v = group();
    v["graph"]["nodes"][0]["evidence_from"] = json!([]);
    reject_field(&v, "evidence_from", "nonempty");
    let mut v = group();
    v["node_definitions"]["choose"]["options"][2]["guards"] = json!([]);
    reject_field(&v, "procedure v2", "one to four");
    let mut v = group();
    v["node_definitions"]["choose"]["assessment"] = json!({"target":"session_goal","outcomes":{"success":"achieved","repair":"not_achieved","replan":"superseded"}});
    reject(&v, "goal_tracking");
    for (count, ok) in [(16, true), (17, false)] {
        let mut v = group();
        v["node_definitions"]["assess"]["items"] = json!(
            (0..count)
                .map(|n| text(&format!("item-{n}")))
                .collect::<Vec<_>>()
        );
        v["graph"]["nodes"][2]["evidence_from"][0]["items"] =
            json!((0..count).map(|n| format!("item-{n}")).collect::<Vec<_>>());
        v["node_definitions"]["choose"]["options"][2]["guards"] = json!([{"evidence":{"scope":"local","node":"assess","item":"item-0"},"non_empty":true}]);
        assert_eq!(admit(&v).is_ok(), ok);
    }
    let mut v = group();
    v["node_definitions"]["assess"]["items"][3]["required_when"][0]["item"] = json!("later");
    reject(&v, "earlier unconditionally required");
    let mut v = group();
    v["node_definitions"]["assess"]["items"][3]["required_when"][0]["equals"] = json!(20);
    reject(&v, "operator and literal");
    let oversized = vec![b' '; podway_config::MAX_PROCEDURE_DOCUMENT_BYTES + 1];
    assert!(matches!(
        parse_successor_document(&oversized, Format::Json),
        Err(ConfigError::InputTooLarge { .. })
    ));
    let duplicate = b"{\"schema\":\"podway.procedure/v3\",\"schema\":\"podway.procedure/v3\"}";
    assert!(matches!(
        parse_successor_document(duplicate, Format::Json),
        Err(ConfigError::DuplicateKey { .. })
    ));
}
