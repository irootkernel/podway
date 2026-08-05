use podway_config::{
    ConfigError, ParsedProcedure, ParsedProcedureV2, parse_procedure_v1, parse_procedure_yaml,
};
use podway_core::TransitionEffectV2;

fn v2(yaml: &str) -> Result<ParsedProcedureV2, ConfigError> {
    match parse_procedure_yaml(yaml.as_bytes()) {
        Ok(ParsedProcedure::V2(parsed)) => Ok(parsed),
        Ok(ParsedProcedure::V1(_)) => panic!("expected v2 dispatch, got v1"),
        Err(error) => Err(error),
    }
}

fn err(yaml: &str) -> ConfigError {
    v2(yaml).expect_err("expected a closed failure")
}

const ACTION_A: &str = "  a:\n    type: action\n    title: A\n    intent: I\n";
const TERMINAL_N: &str = "    - id: n\n      use: a\n      terminal: true\n";

fn v2_doc(node_defs: &str, graph_nodes: &str) -> String {
    v2_doc_extra("", node_defs, graph_nodes)
}

fn v2_doc_extra(extra: &str, node_defs: &str, graph_nodes: &str) -> String {
    format!(
        "schema: podway.procedure/v2\nid: p\nversion: \"1\"\nname: P\npurpose: P.\n{extra}node_definitions:\n{node_defs}graph:\n  entry: n\n  nodes:\n{graph_nodes}",
    )
}

fn minimal_v2() -> &'static str {
    concat!(
        "schema: podway.procedure/v2\n",
        "id: proc\n",
        "version: \"1\"\n",
        "name: Proc\n",
        "purpose: Do the work.\n",
        "node_definitions:\n",
        "  act:\n",
        "    type: action\n",
        "    title: Act\n",
        "    intent: Produce the result.\n",
        "    items:\n",
        "      - id: note\n",
        "        type: text\n",
        "        prompt: Record the result.\n",
        "        required: true\n",
        "graph:\n",
        "  entry: do\n",
        "  nodes:\n",
        "    - id: do\n",
        "      use: act\n",
        "      terminal: true\n",
    )
}

const V1_DOC: &str = concat!(
    "schema: podway.procedure/v1\n",
    "id: release\n",
    "version: \"1\"\n",
    "name: Release\n",
    "stages:\n",
    "  - id: prepare\n",
    "    title: Prepare\n",
    "    items:\n",
    "      - id: approval\n",
    "        type: confirm\n",
    "        prompt: Approved\n",
    "        required: true\n",
    "rework:\n",
    "  allow_return_to: [prepare]\n",
);

#[test]
fn dispatch_routes_v1_unchanged_and_v2_only_by_exact_schema() {
    let dispatched_v1 = match parse_procedure_yaml(V1_DOC.as_bytes()) {
        Ok(ParsedProcedure::V1(validated)) => validated,
        other => panic!("expected v1 dispatch, got {other:?}"),
    };
    assert_eq!(
        dispatched_v1,
        parse_procedure_v1(V1_DOC, podway_config::ProcedureFormatV1::Yaml).expect("v1 parses"),
    );

    v2(minimal_v2()).expect("exact v2 schema dispatches to v2");

    let v3 = minimal_v2().replacen("podway.procedure/v2", "podway.procedure/v3", 1);
    assert!(matches!(
        err(&v3),
        ConfigError::InvalidSchema { expected, .. }
            if expected == "podway.procedure/v1 or podway.procedure/v2"
    ));
    let wrong_case = minimal_v2().replacen("podway.procedure/v2", "Podway.Procedure/V2", 1);
    assert!(matches!(
        err(&wrong_case),
        ConfigError::InvalidSchema { .. }
    ));
}

#[test]
fn v2_rejects_top_level_json_form_but_keeps_nested_flow_collections() {
    let json = "{\"schema\":\"podway.procedure/v2\",\"id\":\"p\",\"version\":\"1\",\"name\":\"P\",\
                \"purpose\":\"P.\",\"node_definitions\":{\"a\":{\"type\":\"action\",\"title\":\"A\",\
                \"intent\":\"I\"}},\"graph\":{\"entry\":\"n\",\"nodes\":[{\"id\":\"n\",\"use\":\"a\",\
                \"terminal\":true}]}}";
    assert!(matches!(
        parse_procedure_yaml(json.as_bytes()),
        Err(ConfigError::InvalidDocument { reason }) if reason.contains("JSON-form")
    ));

    let with_nested_flow = v2_doc(
        "  a:\n    type: action\n    title: A\n    intent: I\n    items:\n      - { id: c, type: confirm, prompt: C?, required: true }\n",
        TERMINAL_N,
    );
    v2(&with_nested_flow).expect("nested flow collections inside a block document parse");
}

#[test]
fn v2_maps_every_action_decision_route_evidence_and_goal_field() {
    let yaml = concat!(
        "schema: podway.procedure/v2\n",
        "id: software-change\n",
        "version: \"2\"\n",
        "name: Verified change\n",
        "purpose: Deliver a reviewed change.\n",
        "description: Full-feature known answer.\n",
        "goal_tracking: true\n",
        "node_definitions:\n",
        "  build:\n",
        "    type: action\n",
        "    title: Build\n",
        "    intent: Build the change.\n",
        "    instructions:\n",
        "      - Build outside Podway.\n",
        "    items:\n",
        "      - id: confirm-done\n",
        "        type: confirm\n",
        "        prompt: Done?\n",
        "        required: true\n",
        "      - id: summary\n",
        "        type: text\n",
        "        prompt: Summary?\n",
        "        required: true\n",
        "        min_length: 1\n",
        "        max_length: 400\n",
        "      - id: channel\n",
        "        type: choice\n",
        "        prompt: Which channel?\n",
        "        required: false\n",
        "        choices: [stable, beta]\n",
        "      - id: count\n",
        "        type: integer\n",
        "        prompt: How many?\n",
        "        required: false\n",
        "        minimum: 0\n",
        "        maximum: 10\n",
        "      - id: findings\n",
        "        type: list\n",
        "        prompt: Findings?\n",
        "        required: false\n",
        "        max_items: 5\n",
        "      - id: attachment\n",
        "        type: artifact\n",
        "        prompt: Attach the log.\n",
        "        required: false\n",
        "        allowed_media_types: [text/plain]\n",
        "  assess:\n",
        "    type: decision\n",
        "    title: Assess\n",
        "    objective: Only acceptable evidence proceeds.\n",
        "    prompt: Acceptable?\n",
        "    evidence_guidance:\n",
        "      - Read the summary.\n",
        "    options:\n",
        "      - id: passed\n",
        "        label: Passed\n",
        "        criteria: Evidence is acceptable.\n",
        "      - id: failed\n",
        "        label: Failed\n",
        "      - id: superseded\n",
        "        label: Superseded\n",
        "    reason:\n",
        "      required: true\n",
        "      prompt: Explain.\n",
        "    assessment:\n",
        "      target: session_goal\n",
        "      outcomes:\n",
        "        passed: achieved\n",
        "        failed: not_achieved\n",
        "        superseded: superseded\n",
        "graph:\n",
        "  entry: build-node\n",
        "  nodes:\n",
        "    - id: build-node\n",
        "      use: build\n",
        "      skip:\n",
        "        allowed: true\n",
        "        reason_required: true\n",
        "      next: assess-node\n",
        "    - id: assess-node\n",
        "      use: assess\n",
        "      evidence_from:\n",
        "        - node: build-node\n",
        "          required: true\n",
        "          items: [summary]\n",
        "        - node: build-node\n",
        "          required: false\n",
        "      routes:\n",
        "        passed:\n",
        "          to: done\n",
        "          effect: advance\n",
        "        failed:\n",
        "          to: build-node\n",
        "          effect: rework\n",
        "        superseded:\n",
        "          to: done\n",
        "          effect: advance\n",
        "    - id: done\n",
        "      use: build\n",
        "      terminal: true\n",
        "manual_rework:\n",
        "  allowed_targets: [build-node]\n",
    );

    let parsed = v2(yaml).expect("full-feature v2 maps");
    assert_eq!(parsed.id(), "software-change");
    assert_eq!(parsed.version(), "2");
    assert_eq!(parsed.name(), "Verified change");
    assert_eq!(parsed.purpose(), "Deliver a reviewed change.");
    assert_eq!(parsed.description(), Some("Full-feature known answer."));
    assert!(parsed.goal_tracking().is_some_and(|g| g.is_enabled()));

    let defs = parsed.node_definitions();
    assert_eq!(defs.len(), 2);
    let build = match &defs[0] {
        podway_config::ParsedNodeDefinition::Action(a) => a,
        _ => panic!("first definition is an action"),
    };
    assert_eq!(build.id().as_str(), "build");
    assert_eq!(build.items().len(), 6);
    let assess = match &defs[1] {
        podway_config::ParsedNodeDefinition::Decision(d) => d,
        _ => panic!("second definition is a decision"),
    };
    assert_eq!(assess.options().len(), 3);
    assert_eq!(
        assess
            .assessment()
            .expect("assessment present")
            .outcomes()
            .len(),
        3
    );

    let graph = parsed.graph();
    assert_eq!(graph.entry().as_str(), "build-node");
    assert_eq!(graph.node_count(), 3);
    let assess_placement =
        match graph.placement(&podway_core::GraphNodeId::new("assess-node").unwrap()) {
            Some(podway_core::GraphPlacementV2::Decision(d)) => d,
            _ => panic!("assess-node is a decision placement"),
        };
    let routes = assess_placement.routes().entries();
    assert_eq!(routes.len(), 3);
    assert_eq!(routes[0].option_id().as_str(), "passed");
    assert_eq!(routes[0].route().effect(), TransitionEffectV2::Advance);
    assert_eq!(routes[1].route().effect(), TransitionEffectV2::Rework);
    let evidence = assess_placement
        .evidence_from()
        .expect("evidence present")
        .entries();
    assert_eq!(evidence.len(), 2);
    assert!(evidence[0].required());
    assert_eq!(
        evidence[0].selected_items().expect("selected items"),
        &[podway_core::ItemId::new("summary").unwrap()],
    );
    assert!(!evidence[1].required());
    assert!(matches!(
        graph.placement(&podway_core::GraphNodeId::new("build-node").unwrap()),
        Some(podway_core::GraphPlacementV2::Action(_))
    ));
    assert_eq!(
        graph.manual_rework().expect("manual rework").targets(),
        &[podway_core::GraphNodeId::new("build-node").unwrap()],
    );
}

#[test]
fn v2_preserves_author_order_for_maps_and_arrays() {
    let yaml = concat!(
        "schema: podway.procedure/v2\n",
        "id: ordered\n",
        "version: \"1\"\n",
        "name: Ordered\n",
        "purpose: Preserve author order.\n",
        "node_definitions:\n",
        "  zeta:\n",
        "    type: action\n",
        "    title: Zeta\n",
        "    intent: First authored.\n",
        "  alpha:\n",
        "    type: decision\n",
        "    title: Alpha\n",
        "    objective: Decide.\n",
        "    prompt: Which?\n",
        "    options:\n",
        "      - id: zoo\n",
        "        label: Zoo\n",
        "      - id: apple\n",
        "        label: Apple\n",
        "    reason:\n",
        "      required: true\n",
        "graph:\n",
        "  entry: z\n",
        "  nodes:\n",
        "    - id: z\n",
        "      use: zeta\n",
        "      terminal: true\n",
        "    - id: a\n",
        "      use: alpha\n",
        "      routes:\n",
        "        zoo:\n",
        "          to: z\n",
        "          effect: advance\n",
        "        apple:\n",
        "          to: z\n",
        "          effect: advance\n",
    );
    let parsed = v2(yaml).expect("ordered v2 maps");

    let ids: Vec<_> = parsed
        .node_definitions()
        .iter()
        .map(|d| d.id().as_str())
        .collect();
    assert_eq!(ids, vec!["zeta", "alpha"]);

    let alpha = match &parsed.node_definitions()[1] {
        podway_config::ParsedNodeDefinition::Decision(d) => d,
        _ => panic!("alpha is a decision"),
    };
    let option_order: Vec<_> = alpha.options().iter().map(|o| o.id().as_str()).collect();
    assert_eq!(option_order, vec!["zoo", "apple"]);

    let alpha_placement = match parsed
        .graph()
        .placement(&podway_core::GraphNodeId::new("a").unwrap())
    {
        Some(podway_core::GraphPlacementV2::Decision(d)) => d,
        _ => panic!("a is a decision placement"),
    };
    let route_order: Vec<_> = alpha_placement
        .routes()
        .entries()
        .iter()
        .map(|e| e.option_id().as_str())
        .collect();
    assert_eq!(route_order, vec!["zoo", "apple"]);

    let node_order: Vec<_> = parsed
        .graph()
        .placements()
        .iter()
        .map(|p| p.id().as_str())
        .collect();
    assert_eq!(node_order, vec!["z", "a"]);
}

#[test]
fn v2_applies_documented_defaults() {
    let yaml = v2_doc(
        "  act:\n    type: action\n    title: Act\n    intent: Do.\n    items:\n      - id: t\n        type: text\n        prompt: T?\n        required: false\n      - id: l\n        type: list\n        prompt: L?\n        required: false\n",
        "    - id: n\n      use: act\n      terminal: true\n    - id: ref\n      use: act\n      evidence_from:\n        - node: n\n      next: n\n",
    );
    let parsed = v2(&yaml).expect("defaults v2");
    let action = match &parsed.node_definitions()[0] {
        podway_config::ParsedNodeDefinition::Action(a) => a,
        _ => panic!("act is an action"),
    };
    let text = match &action.items()[0] {
        podway_core::ItemSpecV2::Text(t) => t,
        _ => panic!("t is a text item"),
    };
    assert_eq!(text.max_length(), 4_000);
    assert!(text.multiline());
    let list = match &action.items()[1] {
        podway_core::ItemSpecV2::List(l) => l,
        _ => panic!("l is a list item"),
    };
    assert_eq!(list.max_items(), 50);
    assert_eq!(list.max_item_length(), 500);
    assert!(list.unique());

    let reference = parsed
        .graph()
        .placement(&podway_core::GraphNodeId::new("ref").unwrap())
        .and_then(|p| match p {
            podway_core::GraphPlacementV2::Action(a) => a.evidence_from(),
            _ => None,
        })
        .expect("evidence present")
        .entries()
        .first()
        .expect("first reference");
    assert!(reference.required());
}

#[test]
fn v2_rejects_explicit_empty_optional_collections_but_accepts_omission() {
    v2(&v2_doc(ACTION_A, TERMINAL_N)).expect("omission of instructions/items is accepted");

    for field in ["instructions", "items"] {
        let doc = v2_doc(&format!("{ACTION_A}    {field}: []\n"), TERMINAL_N);
        assert!(
            matches!(err(&doc), ConfigError::InvalidValue { .. }),
            "{field}: [] should be rejected"
        );
    }

    let decision = "  d:\n    type: decision\n    title: D\n    objective: O\n    prompt: P\n    options:\n      - { id: a, label: A }\n    reason: { required: true }\n    evidence_guidance: []\n";
    let graph = "    - id: n\n      use: d\n      routes:\n        a: { to: n, effect: advance }\n";
    assert!(matches!(
        err(&v2_doc(decision, graph)),
        ConfigError::InvalidValue { .. }
    ));

    let artifact_empty = "  a:\n    type: action\n    title: A\n    intent: I\n    items:\n      - id: art\n        type: artifact\n        prompt: Attach?\n        required: false\n        allowed_media_types: []\n";
    assert!(
        matches!(
            err(&v2_doc(artifact_empty, TERMINAL_N)),
            ConfigError::InvalidValue { .. }
        ),
        "allowed_media_types: [] should be rejected"
    );
    let artifact_omitted = "  a:\n    type: action\n    title: A\n    intent: I\n    items:\n      - id: art\n        type: artifact\n        prompt: Attach?\n        required: false\n";
    v2(&v2_doc(artifact_omitted, TERMINAL_N)).expect("omitted allowed_media_types is accepted");
}

#[test]
fn shared_yaml_hazards_fail_closed_through_v2_dispatch() {
    type HazardCase = (&'static str, &'static str, fn(&ConfigError) -> bool);
    let cases: &[HazardCase] = &[
        (
            "duplicate key",
            "schema: podway.procedure/v2\nschema: podway.procedure/v2\n",
            |e| matches!(e, ConfigError::DuplicateKey { key } if key.as_str() == "schema"),
        ),
        (
            "alias",
            "schema: &s podway.procedure/v2\nid: *s\n",
            |e| matches!(e, ConfigError::UnsupportedYamlFeature { feature } if *feature == "anchor"),
        ),
        (
            "tag",
            "schema: !t podway.procedure/v2\n",
            |e| matches!(e, ConfigError::UnsupportedYamlFeature { feature } if *feature == "tag"),
        ),
        (
            "explicit null",
            "schema: podway.procedure/v2\nid: null\n",
            |e| matches!(e, ConfigError::InvalidDocument { .. }),
        ),
        (
            "multiple documents",
            "schema: podway.procedure/v2\n---\nschema: podway.procedure/v2\n",
            |e| matches!(e, ConfigError::InvalidDocument { .. }),
        ),
        (
            "non-canonical number",
            "schema: podway.procedure/v2\nid: proc\nversion: \"1\"\nname: P\npurpose: P.\nmanual_rework:\n  allowed_targets: [1.0]\nnode_definitions:\n  a:\n    type: action\n    title: A\n    intent: I\ngraph:\n  entry: n\n  nodes:\n    - id: n\n      use: a\n      terminal: true\n",
            |e| matches!(e, ConfigError::NonCanonicalNumber),
        ),
    ];
    for (name, source, matches) in cases {
        assert!(matches(&err(source)), "{name}: failed hazard gate");
    }
}

#[test]
fn v2_rejects_byte_and_depth_limits() {
    let mut huge = String::from(
        "schema: podway.procedure/v2\nid: big\nversion: \"1\"\nname: Big\npurpose: P.\ndescription: \"",
    );
    huge.push_str(&"x".repeat(1_048_560));
    huge.push_str("\"\nnode_definitions:\n  a:\n    type: action\n    title: A\n    intent: I\ngraph:\n  entry: n\n  nodes:\n    - id: n\n      use: a\n      terminal: true\n");
    assert!(matches!(
        parse_procedure_yaml(huge.as_bytes()),
        Err(ConfigError::InputTooLarge { .. })
    ));

    let mut nested = String::from("schema: podway.procedure/v2\n");
    let mut indent = String::new();
    for _ in 0..65 {
        nested.push_str(&indent);
        nested.push_str("a:\n");
        indent.push_str("  ");
    }
    nested.push_str(&indent);
    nested.push_str("b: c\n");
    assert!(matches!(err(&nested), ConfigError::InputTooDeep { .. }));
}

#[test]
fn v2_rejects_unknown_field_wrong_scalar_and_missing_schema() {
    assert!(matches!(
        err(&v2_doc_extra("unknown: true\n", ACTION_A, TERMINAL_N)),
        ConfigError::InvalidDocument { .. }
    ));
    assert!(matches!(
        err(&v2_doc_extra(
            "goal_tracking: \"true\"\n",
            ACTION_A,
            TERMINAL_N
        )),
        ConfigError::InvalidDocument { .. }
    ));
    assert!(matches!(
        parse_procedure_yaml(b"id: p\nversion: \"1\"\n"),
        Err(ConfigError::InvalidDocument { .. })
    ));
}

#[test]
fn constructor_bounds_fail_closed_during_mapping() {
    assert!(matches!(
        err(&v2_doc(
            &format!(
                "  a:\n    type: action\n    title: \"{}\"\n    intent: I\n",
                "t".repeat(121)
            ),
            TERMINAL_N,
        )),
        ConfigError::InvalidValue { .. } | ConfigError::OutOfBounds { .. }
    ));
    assert!(matches!(
        err(&v2_doc(
            "  Bad-ID:\n    type: action\n    title: A\n    intent: I\n",
            "    - id: n\n      use: Bad-ID\n      terminal: true\n",
        )),
        ConfigError::InvalidValue { .. }
    ));
    let nine_options = "  d:\n    type: decision\n    title: D\n    objective: O\n    prompt: P\n    options:\n      - { id: a, label: A }\n      - { id: b, label: B }\n      - { id: c, label: C }\n      - { id: e, label: E }\n      - { id: f, label: F }\n      - { id: g, label: G }\n      - { id: h, label: H }\n      - { id: i, label: I }\n      - { id: j, label: J }\n    reason: { required: true }\n";
    assert!(matches!(
        err(&v2_doc(
            nine_options,
            "    - id: n\n      use: d\n      routes:\n        a: { to: n, effect: advance }\n",
        )),
        ConfigError::InvalidValue { .. }
    ));
    assert!(matches!(
        err(&v2_doc(
            ACTION_A,
            "    - id: n\n      use: a\n      skip: { allowed: false, reason_required: false }\n      terminal: true\n",
        )),
        ConfigError::InvalidValue { .. }
    ));
    assert!(matches!(
        err(&v2_doc(
            ACTION_A,
            "    - id: n\n      use: a\n      next: other\n      terminal: true\n    - id: other\n      use: a\n      terminal: true\n",
        )),
        ConfigError::InvalidValue { .. }
    ));
    assert!(matches!(
        err(&v2_doc_extra(
            "goal_tracking: false\n",
            ACTION_A,
            TERMINAL_N
        )),
        ConfigError::InvalidValue { .. }
    ));
}

#[test]
fn v2_does_not_perform_semantic_or_canonical_validation() {
    let yaml = concat!(
        "schema: podway.procedure/v2\n",
        "id: semantic-deferred\n",
        "version: \"1\"\n",
        "name: Semantic deferred\n",
        "purpose: Parse without semantic checks.\n",
        "node_definitions:\n",
        "  used:\n",
        "    type: action\n",
        "    title: Used\n",
        "    intent: Used.\n",
        "  unused:\n",
        "    type: action\n",
        "    title: Unused\n",
        "    intent: Never placed.\n",
        "graph:\n",
        "  entry: start\n",
        "  nodes:\n",
        "    - id: start\n",
        "      use: used\n",
        "      next: undefined-target\n",
        "    - id: orphan\n",
        "      use: undefined-definition\n",
        "      next: start\n",
    );
    v2(yaml).expect("syntactically valid but semantically invalid still maps");
}
